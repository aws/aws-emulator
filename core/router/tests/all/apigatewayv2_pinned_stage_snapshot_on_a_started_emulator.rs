// SPDX-License-Identifier: CC0-1.0

//! A non-autoDeploy stage of an HTTP api, pinned to one deployment, served to an UNSIGNED
//! client on an emulator a client STARTED — the position a client of a deployed stage is in.
//!
//! RULE: **what a stage of an HTTP api serves is the configuration the deployment its own
//! `DeploymentId` names froze: a stage whose `AutoDeploy` is off keeps serving that snapshot
//! while the api's integrations are changed under it, a stage whose `AutoDeploy` is on has a
//! new deployment released to it by the service on every such change and converges on the new
//! configuration, and a `CreateDeployment` naming a stage mints a deployment of the api's
//! CURRENT configuration and re-binds that stage to it — every one of those answers being a
//! terminal HTTP status the client reads off the wire.**
//!
//! Two crates have to agree for that to hold — AWS Lambda holds the two functions and RUNS
//! them, API Gateway v2 holds the api, the deployments, the stages and resolves a request on
//! the published `execute-api` host to a stage, its deployment, its route and its integration —
//! and the whole point of running it on a STARTED emulator is that every request here crosses a
//! real socket, carries no credential and is answered by the binary rather than by an
//! in-process router, which is the position the client that filed the issue was in.
//!
//! Nothing here reads an emulator internal: which snapshot a stage served is read off the BODY
//! each backend returned, and each backend answers the marker its own environment names.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::apigatewayv2_published_endpoint_runs_its_lambda_route::announced_parts;
use crate::cfn_deployed_pipeline_on_a_started_emulator::{Started, REGION};
use crate::query_wire::opt_element;

/// The account this emulator serves, which the grants' `SourceArn` carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The stack the two backends are deployed as, and the status the suite waits for.
const STACK: &str = "apigwv2-pinned-stage-snapshot";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The two functions, and the marker each answers with.
const FIRST_FUNCTION: &str = "apigwv2-pinned-snapshot-a";
const SECOND_FUNCTION: &str = "apigwv2-pinned-snapshot-b";
const FIRST_MARKER: &str = "backend-a";
const SECOND_MARKER: &str = "backend-b";

/// The route both deployments carry, and the path within a stage that matches it.
const ROUTE_KEY: &str = "GET /snap";
const ROUTE_PATH: &str = "/snap";

/// The stage pinned to one deployment, beside the reserved stage that serves the api's root.
const PINNED_STAGE: &str = "pinned";
const ROOT_STAGE: &str = "$default";

/// How long one invocation of either function is given, as its own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long to wait for work the emulator owes — a stack settling, a function reaching
/// `Active`, a stage converging on the configuration a release handed it — and how long between
/// two looks. Generous on purpose, so a loaded host never decides the outcome.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The handler both backends run: it answers a `200` whose text body is the marker its own
/// environment names, so which backend answered a request is read off the body alone.
const MARKING_HANDLER: &str = "import os\n\
     def handler(event, context):\n    \
     return {'statusCode': 200, 'headers': {'content-type': 'text/plain'},\n             \
     'body': os.environ['MARKER']}\n";

/// The template the two backends are deployed by: CloudFormation archives the inline source
/// into a deployment package this emulator can run, and each function's environment names its
/// own marker.
fn template() -> String {
    let function = |name: &str, marker: &str| {
        json!({
            "Type": "AWS::Serverless::Function",
            "Properties": {
                "FunctionName": name,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Timeout": FUNCTION_TIMEOUT,
                "InlineCode": MARKING_HANDLER,
                "Environment": { "Variables": { "MARKER": marker } },
            },
        })
    };
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "First": function(FIRST_FUNCTION, FIRST_MARKER),
            "Second": function(SECOND_FUNCTION, SECOND_MARKER),
        },
    })
    .to_string()
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

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what:?}: must carry a non-empty {member:?}, got {body}"))
}

/// Create the stack and wait until it reports `CREATE_COMPLETE`.
///
/// Written here rather than taken from the shared `deployed` of
/// [`crate::cfn_deployed_pipeline_on_a_started_emulator`] because this template declares a
/// macro (`AWS::Serverless-2016-10-31`), which is acknowledged with `CAPABILITY_AUTO_EXPAND`
/// and that helper sends only `CAPABILITY_IAM`; the terminal-status wait below is the same one.
fn deploy(emulator: &Started) {
    let body = template();
    let (status, text) = emulator.cloudformation(
        "CreateStack",
        &[
            ("StackName", STACK),
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
        let (status, body) = emulator.cloudformation("DescribeStacks", &[("StackName", STACK)]);
        assert_eq!(
            status, 200,
            "setup: DescribeStacks must answer 200, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        if reported == CREATE_COMPLETE
            || reported.ends_with("_FAILED")
            || reported.contains("ROLLBACK")
        {
            break;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
    assert_eq!(
        reported, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {reported:?} with {described}; the \
         log:\n{}",
        emulator.log()
    );
}

/// The ARN Lambda reports for `name`, once it reports it `Active` — "the function is ready to
/// be invoked" (`FunctionConfiguration$State`), which is the condition this waits on.
fn active_function_arn(emulator: &Started, name: &str) -> String {
    let deadline = Instant::now() + BUDGET;
    let configuration = loop {
        let (status, text) = emulator.call(
            "lambda",
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &[],
            Vec::new(),
        );
        assert_eq!(
            status, 200,
            "setup: GetFunctionConfiguration({name:?}) must answer 200, got {status} with {text}"
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
        "setup: Lambda must report {name:?} Active, got {configuration}"
    );
    as_str("GetFunctionConfiguration", &configuration, "FunctionArn")
}

/// Grant `apigateway.amazonaws.com` the invocation of `name`, narrowed to every stage, method
/// and path of `api_id` — the grant a client makes beside the api it fronts a route with.
fn grant(emulator: &Started, name: &str, api_id: &str) {
    let source_arn = format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:{api_id}/*/*");
    let (status, text) = emulator.call(
        "lambda",
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        &[("content-type", "application/json")],
        json!({
            "StatementId": "apigatewayv2-may-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": source_arn,
        })
        .to_string()
        .into_bytes(),
    );
    assert_eq!(
        status, 201,
        "setup: AddPermission({name:?}, {source_arn:?}) must answer 201, got {status} with {text}"
    );
}

/// How a client addresses a stage of the api: what it calls this addressing, the address the
/// connection is MADE to, the `Host` the request carries, and the base path ahead of the stage.
struct Addressing {
    case: &'static str,
    /// The authority dialled — for the address the api itself published, the loopback interface
    /// that address names on the port it carries, which is a listener of this emulator's own
    /// and not the address the harness knows it by.
    dialled: String,
    host: String,
    base_path: String,
}

/// Both addressings a client reaches an HTTP api's data plane at:
///
/// * the endpoint the api itself ANNOUNCED (`Api$ApiEndpoint`) — the one value a client of a
///   deployed api is handed, which on an emulator served on a port is the ADDRESS its data
///   plane's socket landed on, with the api's own id behind the reserved data-plane path. The
///   connection is made to that address, which needs no resolver at all, and the published
///   authority is what the request NAMES — the address is this emulator's and the path is the
///   api's;
/// * the api's own documented `execute-api` host
///   (`{api-id}.execute-api.{region}.amazonaws.com`) at this emulator's front door, which is
///   the addressing a client that resolves AWS's own names uses.
fn addressings(emulator: &Started, api_id: &str, announced: &str) -> Vec<Addressing> {
    // The split of a published endpoint into the authority a client dials and the path it
    // carries ahead of the stage is the tree's one reading of what was announced
    // ([`announced_parts`]), so this suite takes it rather than writing a second one.
    let (authority, base_path) = announced_parts(announced);
    let base_path = base_path.trim_end_matches('/').to_string();
    let port = authority
        .rsplit_once(':')
        .map(|(_host, port)| port.to_string())
        .unwrap_or_default();
    vec![
        Addressing {
            case: "the endpoint the api announced, named as published",
            dialled: format!("127.0.0.1:{port}"),
            host: authority,
            base_path,
        },
        Addressing {
            case: "the api's own execute-api host at this emulator's front door",
            dialled: emulator.authority(),
            host: format!("{api_id}.execute-api.{REGION}.amazonaws.com"),
            base_path: String::new(),
        },
    ]
}

/// Assert the endpoint an api announced is an ADDRESS this emulator answers on — the address
/// its data-plane socket landed on, carrying the port the core assigned — with the api's own id
/// behind the reserved data-plane path, which is what makes the first addressing above dialable
/// at all. Read once, before the rows run.
fn assert_announced_is_this_emulators(api_id: &str, announced: &str) {
    let (authority, base_path) = announced_parts(announced);
    let (host, port) = authority.rsplit_once(':').unwrap_or((authority.as_str(), ""));
    assert!(
        announced.starts_with("https://")
            && host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
            && port.parse::<u16>().is_ok()
            && base_path.trim_end_matches('/') == format!("/_aws/execute-api/{api_id}"),
        "setup: an api must announce its own data plane on an address of this emulator's \
         own, got {announced:?}"
    );
}

/// The path at which `stage` serves the route under `addressing`: the stage that serves the
/// api's root is reached with no stage segment at all, a named stage behind its own name.
fn invoke_path(addressing: &Addressing, stage: &str) -> String {
    if stage == ROOT_STAGE {
        format!("{}{ROUTE_PATH}", addressing.base_path)
    } else {
        format!("{}/{stage}{ROUTE_PATH}", addressing.base_path)
    }
}

/// One request a CLIENT of a published stage makes: dialled at the address the addressing
/// names, carrying its `Host` and no credential at all. `Err` carries the transport's own
/// diagnosis, so an address nothing answers on is a case a row can assert rather than a panic.
fn invoked(
    emulator: &Started,
    addressing: &Addressing,
    stage: &str,
) -> Result<(u16, String), String> {
    let path = invoke_path(addressing, stage);
    let url = format!("https://{}{path}", addressing.dialled);
    let answered = emulator
        .client()
        .get(&url)
        .header("host", &addressing.host)
        .send()
        .map_err(|error| format!("nothing answered at {url}: {error}"))?;
    let status = answered.status().as_u16();
    Ok((status, answered.text().unwrap_or_default()))
}

/// The `DeploymentId` `GetStage` reports for `stage`, or `None` for a stage serving none.
fn stage_deployment(emulator: &Started, api_id: &str, stage: &str) -> Option<String> {
    let read = apigatewayv2(
        emulator,
        &format!("GetStage({stage})"),
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage}"),
        None,
        200,
    );
    read.get("deploymentId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Look until `stage` first answers `(200, marker)` at `addressing`, and answer what the last
/// look saw — so a case asserts on the CONDITION rather than on how fast this host runs.
fn served_marker(
    emulator: &Started,
    addressing: &Addressing,
    stage: &str,
    marker: &str,
) -> Result<(u16, String), String> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let answered = invoked(emulator, addressing, stage)?;
        if (answered.0, answered.1.as_str()) == (200, marker) || Instant::now() >= deadline {
            return Ok(answered);
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// Assert every addressing serves `stage` the backend `marker` names, waiting for that
/// condition at each.
fn assert_serves(
    emulator: &Started,
    api_id: &str,
    announced: &str,
    stage: &str,
    marker: &str,
) {
    for addressing in addressings(emulator, api_id, announced) {
        let answered = served_marker(emulator, &addressing, stage, marker);
        let (status, body) = answered.unwrap_or_else(|error| {
            panic!(
                "case {:?}: stage {stage:?} must be served at {}, but {error}; the log:\n{}",
                addressing.case,
                invoke_path(&addressing, stage),
                emulator.log()
            )
        });
        assert_eq!(
            (status, body.as_str()),
            (200, marker),
            "case {:?}: stage {stage:?} at {} must serve {marker:?}, got {status} with {body:?}; \
             the log:\n{}",
            addressing.case,
            invoke_path(&addressing, stage),
            emulator.log()
        );
    }
}

/// How many samples a hold window takes of a stage that must not move: the recorded scenario
/// holds the pinned stage over several samples while the api changes under it, which is what
/// tells a stage that holds its snapshot from one that happens to answer it once.
const HOLD_SAMPLES: usize = 6;

#[test]
fn a_started_emulator_serves_a_pinned_stage_its_own_deployments_snapshot() {
    let emulator = Started::start();
    deploy(&emulator);
    let first = active_function_arn(&emulator, FIRST_FUNCTION);
    let second = active_function_arn(&emulator, SECOND_FUNCTION);

    // The api, its integration over the FIRST backend, and the route that fronts it.
    let api = apigatewayv2(
        &emulator,
        "CreateApi",
        "POST",
        "/v2/apis",
        Some(json!({ "name": "pinned-stage-snapshot", "protocolType": "HTTP" })),
        201,
    );
    let api_id = as_str("CreateApi", &api, "apiId");
    let announced = as_str("CreateApi", &api, "apiEndpoint");
    assert_announced_is_this_emulators(&api_id, &announced);
    grant(&emulator, FIRST_FUNCTION, &api_id);
    grant(&emulator, SECOND_FUNCTION, &api_id);
    let integration = apigatewayv2(
        &emulator,
        "CreateIntegration",
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": first,
            "integrationMethod": "POST",
            "payloadFormatVersion": "2.0",
        })),
        201,
    );
    let integration_id = as_str("CreateIntegration", &integration, "integrationId");
    apigatewayv2(
        &emulator,
        "CreateRoute",
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "target": format!("integrations/{integration_id}"),
        })),
        201,
    );

    // The auto-deploying stage that serves the api's root, an explicit deployment, and the
    // stage PINNED to that deployment.
    apigatewayv2(
        &emulator,
        "CreateStage($default)",
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": ROOT_STAGE, "autoDeploy": true })),
        201,
    );
    let released = apigatewayv2(
        &emulator,
        "CreateDeployment",
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({})),
        201,
    );
    let pinned_deployment = as_str("CreateDeployment", &released, "deploymentId");
    apigatewayv2(
        &emulator,
        "CreateStage(pinned)",
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": PINNED_STAGE, "deploymentId": pinned_deployment })),
        201,
    );

    // Both stages serve the first backend, at either addressing: the root stage with no stage
    // segment at all, the pinned stage behind its own name.
    for stage in [ROOT_STAGE, PINNED_STAGE] {
        assert_serves(&emulator, &api_id, &announced, stage, FIRST_MARKER);
    }

    // The api's integration is re-pointed at the SECOND backend.
    apigatewayv2(
        &emulator,
        "UpdateIntegration",
        "PATCH",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        Some(json!({ "integrationUri": second })),
        200,
    );

    // The auto-deploying stage had a new deployment released to it and converges on the new
    // configuration; the pinned stage keeps its own deployment and its snapshot.
    let root_deployment = stage_deployment(&emulator, &api_id, ROOT_STAGE);
    assert!(
        root_deployment
            .as_deref()
            .is_some_and(|released| released != pinned_deployment),
        "the auto-deploying stage must move onto a deployment of its own, got \
         {root_deployment:?}"
    );
    assert_serves(&emulator, &api_id, &announced, ROOT_STAGE, SECOND_MARKER);
    assert_eq!(
        stage_deployment(&emulator, &api_id, PINNED_STAGE).as_deref(),
        Some(pinned_deployment.as_str()),
        "the pinned stage must keep the deployment it was created with"
    );

    // The HOLD WINDOW: every sample of the pinned stage, at either addressing, still answers
    // the snapshot its deployment froze while the api stands changed under it.
    for addressing in addressings(&emulator, &api_id, &announced) {
        for sample in 0..HOLD_SAMPLES {
            let answered = invoked(&emulator, &addressing, PINNED_STAGE);
            let (status, body) = answered.unwrap_or_else(|error| {
                panic!(
                    "case {:?}: sample {sample} of the pinned stage must be answered, but \
                     {error}; the log:\n{}",
                    addressing.case,
                    emulator.log()
                )
            });
            assert_eq!(
                (status, body.as_str()),
                (200, FIRST_MARKER),
                "case {:?}: sample {sample} of the pinned stage must still serve \
                 {FIRST_MARKER:?}, got {status} with {body:?}; the log:\n{}",
                addressing.case,
                emulator.log()
            );
        }
    }

    // Only an explicit deployment for that stage re-binds it.
    let rebound = apigatewayv2(
        &emulator,
        "CreateDeployment(StageName=pinned)",
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "stageName": PINNED_STAGE })),
        201,
    );
    let rebound = as_str("CreateDeployment", &rebound, "deploymentId");
    assert_ne!(
        rebound, pinned_deployment,
        "a new deployment carries an id of its own"
    );
    assert_eq!(
        stage_deployment(&emulator, &api_id, PINNED_STAGE).as_deref(),
        Some(rebound.as_str()),
        "CreateDeployment(StageName=…) must re-bind the stage it names"
    );
    assert_serves(&emulator, &api_id, &announced, PINNED_STAGE, SECOND_MARKER);
}
