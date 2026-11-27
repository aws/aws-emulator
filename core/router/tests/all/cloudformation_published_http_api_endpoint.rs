// SPDX-License-Identifier: CC0-1.0

//! The invoke address a CloudFormation stack PUBLISHES for the HTTP api it deployed, and what
//! a client that dials exactly that address is served.
//!
//! RULE: **an api's `ApiEndpoint` is an address THIS emulator serves that api's data plane at,
//! whichever request minted the api. So the value a stack publishes through
//! `Fn::GetAtt <api>.ApiEndpoint` is the one API Gateway v2 really minted (for an emulator
//! served on a port, the address of the socket it opens for that plane with the api's own id
//! behind the reserved data-plane path; for one served on no port, the api's own documented
//! `execute-api` host, which the front door claims as well), it is what every later reporter of
//! the api reports, and the invoke URL a client builds from it (that value with the stage name
//! appended, the reserved stage that serves the api's root being reached with no stage segment
//! at all) resolves to the route's own integration. An api whose `DisableExecuteApiEndpoint` is
//! set publishes the same address and serves nothing of the api on it.**
//!
//! `Api$ApiEndpoint` is "the URI of the API, of the form
//! `{api-id}.execute-api.{region}.amazonaws.com`. The stage name is typically appended to this
//! URI to form a complete path to a deployed API stage", and `AWS::ApiGatewayV2::Api` publishes
//! that same value as its `ApiEndpoint` attribute ("the default endpoint for an API"). An api a
//! STACK created is minted by a request CloudFormation made to API Gateway v2 under that
//! service's own endpoint host rather than by a client of the emulator's endpoint, which is the
//! case this suite adds: the address published for such an api still has to be one a caller of
//! the stack can dial, or a template's `Outputs` lead a deployment out of the emulator.
//!
//! Two crates have to agree for that to hold — CloudFormation makes the create and surfaces the
//! attribute, API Gateway v2 mints the api, settles the address and serves the requests — so the
//! suite lives here rather than in either of them.
//!
//! The rows span what the rule generalises over: the endpoint the emulator itself is SERVED at
//! (an IPv4 loopback literal with a port, a loopback NAME with a port, a bracketed IPv6 literal
//! with a port, and no port at all), how the api is declared (imported from an inline OpenAPI
//! definition, quick-created from a `Target`, and with its integration, route, named stage and
//! deployment declared one by one), the stage the route is reached through (the reserved stage
//! that serves the api's root, reached with no stage segment, and a named stage appended to the
//! published value), the reporter the address is read from (the stack's own `Fn::GetAtt` output
//! and API Gateway v2's `GetApi`, the latter reached at an endpoint OTHER than the one that
//! minted the api), and whether the api's default endpoint is turned off.
//!
//! Nothing of the wire or of the answers is worded again here: the mounting of the two services,
//! both control-plane callers, the invoke of a published endpoint and the two assertions — a
//! route's own answer, and the answer of an endpoint that serves nothing of the api — are read
//! from the sibling that owns them
//! ([`super::cloudformation_http_api_disabled_default_endpoint`]), and the split of an announced
//! address from the sibling that owns that
//! ([`super::apigatewayv2_published_endpoint_runs_its_lambda_route`]). So WHICH status and
//! sentence a disabled default endpoint answers is that suite's reading, applied here rather
//! than restated: a disabled api is a row for the address it PUBLISHES, and because the route
//! behind it must not be reached on that address.
//!
//! What the owning suite in the apigatewayv2 crate pins and this one does not restate: the
//! address announced for an api a client of the emulator's own endpoint created
//! (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/tests/all/http_api_published_invoke_address.rs`).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

// The split of an announced address into the authority a client dials and the path ahead of the
// stage, from the sibling that owns it, so this suite reads a published value exactly as the one
// about a directly created api does.
use crate::apigatewayv2_published_endpoint_runs_its_lambda_route::announced_parts;
// The wait for a stack to reach a status, which the suite that owns it exports because "every
// suite deploying a stack whose resources their owning services report as still coming up needs
// exactly this".
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
// The emulator as it is assembled, the two control-plane callers, the invoke a client of a
// published endpoint makes, and what a served and an unserved address answer — all from the
// suite that owns the reading of a default endpoint that serves nothing.
use crate::cloudformation_http_api_disabled_default_endpoint::{
    apigatewayv2, apigatewayv2_at, assert_serves_its_route, assert_serves_nothing, cloudformation,
    emulator as assembled_emulator, request, Answered, CREATE_COMPLETE, REGION,
};
use crate::query_wire::outputs_of;

/// The front door's envelope for a request NO service claimed. The defect this suite pins is a
/// published address nothing in this emulator answers, so this is never a legitimate answer.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The route every api of the stack carries, and the path within the stage that matches it.
const ROUTE_KEY: &str = "GET /items";
const ROUTE_PATH: &str = "/items";

/// The reserved stage "that serves an api's root" (`$default`), reached with no stage segment in
/// the path at all, and the named stage the explicitly declared api is deployed to, which is
/// reached by appending its name to the published value.
const ROOT_STAGE: &str = "$default";
const NAMED_STAGE: &str = "prod";

/// The upstream each api's route fronts — one per api, so a served answer names which api
/// answered it.
const IMPORTED_UPSTREAM: &str = "https://upstream.example.test/imported";
const QUICK_UPSTREAM: &str = "https://upstream.example.test/quick";
const STAGED_UPSTREAM: &str = "https://upstream.example.test/staged";
const DISABLED_UPSTREAM: &str = "https://upstream.example.test/disabled";

/// One row: the endpoint the emulator is served at, or `None` for an emulator served on no port
/// at all — which is what decides the address the api is announced on.
struct Row {
    case: &'static str,
    served_base: Option<&'static str>,
}

/// The rows whose emulator IS served on a port, in each of the forms an authority is written in.
const SERVED_ROWS: &[Row] = &[
    Row {
        case: "the recorded case: an emulator served on an IPv4 loopback address and port",
        served_base: Some("https://127.0.0.1:9297"),
    },
    Row {
        case: "an emulator served on a loopback NAME and port",
        served_base: Some("https://localhost:34181"),
    },
    Row {
        case: "an emulator served on a bracketed IPv6 literal and port",
        served_base: Some("https://[::1]:9297"),
    },
];

/// The row whose emulator is served on no port at all, where the api is announced on the host
/// `Api$ApiEndpoint` documents — which this emulator's front door claims host-style.
const UNSERVED_ROW: Row = Row {
    case: "an emulator served on no port at all",
    served_base: None,
};

/// One api the stack declares: the logical id it is declared under, the upstream its route
/// fronts, the stage path a client appends to the published endpoint to reach that route, and
/// whether its default endpoint is turned off.
struct DeclaredApi {
    case: &'static str,
    logical: &'static str,
    upstream: &'static str,
    /// Empty for the stage that serves the api's root, which is reached with no stage segment.
    stage_path: &'static str,
    disabled: bool,
}

const DECLARED: &[DeclaredApi] = &[
    DeclaredApi {
        case: "an api imported from an inline OpenAPI definition, its root stage declared over it",
        logical: "ImportedApi",
        upstream: IMPORTED_UPSTREAM,
        stage_path: "",
        disabled: false,
    },
    DeclaredApi {
        case: "an api quick-created from a Target, served on the stage that serves its root",
        logical: "QuickApi",
        upstream: QUICK_UPSTREAM,
        stage_path: "",
        disabled: false,
    },
    DeclaredApi {
        case: "an api whose integration, route, named stage and deployment the template declares \
               one by one",
        logical: "StagedApi",
        upstream: STAGED_UPSTREAM,
        stage_path: "/prod",
        disabled: false,
    },
    DeclaredApi {
        case: "an api quick-created with DisableExecuteApiEndpoint set",
        logical: "DisabledApi",
        upstream: DISABLED_UPSTREAM,
        stage_path: "",
        disabled: true,
    },
];

/// The OpenAPI definition the imported api is declared with: one path, fronted by the
/// `http_proxy` integration that names its own upstream.
fn inline_definition() -> Value {
    json!({
        "openapi": "3.0.1",
        "info": { "title": "cfn-published-endpoint-imported", "version": "1.0" },
        "paths": {
            ROUTE_PATH: {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": IMPORTED_UPSTREAM,
                        "payloadFormatVersion": "1.0"
                    }
                }
            }
        }
    })
}

/// The template the stack deploys: the four apis of [`DECLARED`], and an output per api carrying
/// its `Ref` and its `ApiEndpoint` attribute.
fn template() -> String {
    let resources = json!({
        // "To import an HTTP API, you must specify a Body or BodyS3Location", and such a
        // declaration states none of the members only `CreateApi` takes — so neither a `Name` nor
        // a `ProtocolType`: the definition carries both.
        "ImportedApi": {
            "Type": "AWS::ApiGatewayV2::Api",
            "Properties": { "Body": inline_definition() }
        },
        "ImportedStage": {
            "Type": "AWS::ApiGatewayV2::Stage",
            "Properties": {
                "ApiId": { "Ref": "ImportedApi" },
                "StageName": ROOT_STAGE,
                "AutoDeploy": true
            }
        },
        "QuickApi": {
            "Type": "AWS::ApiGatewayV2::Api",
            "Properties": {
                "Name": "cfn-published-endpoint-quick",
                "ProtocolType": "HTTP",
                "RouteKey": ROUTE_KEY,
                "Target": QUICK_UPSTREAM
            }
        },
        "DisabledApi": {
            "Type": "AWS::ApiGatewayV2::Api",
            "Properties": {
                "Name": "cfn-published-endpoint-disabled",
                "ProtocolType": "HTTP",
                "RouteKey": ROUTE_KEY,
                "Target": DISABLED_UPSTREAM,
                "DisableExecuteApiEndpoint": true
            }
        },
        "StagedApi": {
            "Type": "AWS::ApiGatewayV2::Api",
            "Properties": {
                "Name": "cfn-published-endpoint-staged",
                "ProtocolType": "HTTP"
            }
        },
        "StagedIntegration": {
            "Type": "AWS::ApiGatewayV2::Integration",
            "Properties": {
                "ApiId": { "Ref": "StagedApi" },
                "IntegrationType": "HTTP_PROXY",
                "IntegrationMethod": "GET",
                "IntegrationUri": STAGED_UPSTREAM,
                "PayloadFormatVersion": "1.0"
            }
        },
        "StagedRoute": {
            "Type": "AWS::ApiGatewayV2::Route",
            "Properties": {
                "ApiId": { "Ref": "StagedApi" },
                "RouteKey": ROUTE_KEY,
                "Target": {
                    "Fn::Join": ["/", ["integrations", { "Ref": "StagedIntegration" }]]
                }
            }
        },
        "StagedStage": {
            "Type": "AWS::ApiGatewayV2::Stage",
            "Properties": {
                "ApiId": { "Ref": "StagedApi" },
                "StageName": NAMED_STAGE,
                "AutoDeploy": true
            }
        },
        "StagedDeployment": {
            "Type": "AWS::ApiGatewayV2::Deployment",
            "DependsOn": ["StagedRoute", "StagedStage"],
            "Properties": {
                "ApiId": { "Ref": "StagedApi" },
                "StageName": { "Ref": "StagedStage" }
            }
        }
    });

    let mut outputs = Map::new();
    for api in DECLARED {
        assert!(
            resources.get(api.logical).is_some(),
            "setup: every declared api of the table must be a resource of the template, {:?} is \
             not",
            api.logical
        );
        outputs.insert(id_output(api), json!({ "Value": { "Ref": api.logical } }));
        outputs.insert(
            endpoint_output(api),
            json!({ "Value": { "Fn::GetAtt": [api.logical, "ApiEndpoint"] } }),
        );
    }

    json!({ "Resources": resources, "Outputs": Value::Object(outputs) }).to_string()
}

/// The `Outputs` keys one api's `Ref` and its `ApiEndpoint` attribute are published under.
fn id_output(api: &DeclaredApi) -> String {
    format!("{}Id", api.logical)
}
fn endpoint_output(api: &DeclaredApi) -> String {
    format!("{}Endpoint", api.logical)
}

/// The assembled emulator the sibling mounts, SERVED at `served_base` when the row names one —
/// which is the one thing this suite varies about it.
fn emulator(served_base: Option<&str>) -> MultiServiceRouter {
    let mut router = assembled_emulator();
    if let Some(base) = served_base {
        router.set_url_base(EndpointUrl::parse(base).unwrap_or_else(|| {
            panic!("setup: {base:?} must be a base URL this emulator can be served at")
        }));
    }
    router
}

/// The authority and base path an api of `api_id` must be published on, for an emulator served
/// at `served_base`: the ADDRESS of the SOCKET this emulator publishes an api's data plane on,
/// whose port this suite cannot predict, so the authority is left for
/// [`assert_published_host`], with the api's own id behind the reserved data-plane path; and,
/// for an emulator served on no port at all, the api's own documented `execute-api` host in the
/// zone AWS publishes it in, carrying no port and no path.
fn expected_published_parts(served_base: Option<&str>, api_id: &str) -> (Option<String>, String) {
    match served_base {
        Some(_base) => (None, format!("/_aws/execute-api/{api_id}")),
        None => (
            Some(format!("{api_id}.execute-api.{REGION}.amazonaws.com")),
            String::new(),
        ),
    }
}

/// Assert `authority` is the address this emulator publishes an api's data plane at: the
/// address literal the core's listener landed on, on the loopback interface its listeners are
/// bound on, carrying a port a client can dial.
///
/// The PORT is the core's to choose, so it is not predicted here; that the address is one this
/// emulator answers the api on is read by the invoke that follows.
fn assert_published_host(case: &str, authority: &str) {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port),
        None => panic!("case {case:?}: expected a published host:port, got {authority:?}"),
    };
    assert!(
        port.parse::<u16>().is_ok_and(|port| port > 0),
        "case {case:?}: expected a port a client can dial, got {authority:?}"
    );
    let address: std::net::IpAddr = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .unwrap_or_else(|_| {
            panic!(
                "case {case:?}: expected the address the api's data-plane socket landed on, got \
                 {authority:?}"
            )
        });
    assert!(
        address.is_loopback(),
        "case {case:?}: expected the interface the core binds its listeners on, got {authority:?}"
    );
}


/// One output of the settled stack, which every api of the table publishes two of.
fn output(case: &str, outputs: &BTreeMap<String, String>, key: &str) -> String {
    outputs
        .get(key)
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {key} output, got {outputs:?}"))
}

/// Assert the address this answer came back from is one a service of this emulator CLAIMED,
/// which is the half of the rule the original defect broke: an address nothing answers on.
///
/// Read before either assertion below — a route's own answer and the refusal of an endpoint that
/// serves nothing of the api are both answers of the api's own service, and neither is the front
/// door saying no service claimed the address.
fn assert_claimed(case: &str, answered: &Answered) {
    assert_ne!(
        answered.json().get("__type").and_then(Value::as_str),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the published address must be one this emulator serves, but no service \
         claimed it: {} {:?}",
        answered.status,
        answered.text()
    );
}

/// Deploy the stack on an emulator served as the row says, then hold every api it published to
/// the rule: the address is the one this emulator serves the api at, every reporter reports it,
/// and a client that dials it reaches the api's route — or, for the api that turned its default
/// endpoint off, nothing of the api at all.
async fn assert_row(row: &Row) {
    let router = emulator(row.served_base);
    let stack = "cfn-published-http-api-endpoint";
    let body = template();
    let created = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &body)],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateStack, got {} with {:?}",
        row.case,
        created.status,
        created.text()
    );
    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {:?}: expected the stack to reach {CREATE_COMPLETE}, got {status:?} in {described}",
        row.case
    );
    let outputs = outputs_of(&described);

    for api in DECLARED {
        let case = format!("{}, {}", row.case, api.case);
        let api_id = output(&case, &outputs, &id_output(api));
        let published = output(&case, &outputs, &endpoint_output(api));

        // 1. The address published for the api is one this emulator serves the api's data plane
        // at, in the form the endpoint it is served at decides.
        assert!(
            published.starts_with("https://"),
            "case {case:?}: an HTTP api is invoked over https, got {published:?}"
        );
        let (authority, base_path) = announced_parts(&published);
        let (expected_authority, expected_base_path) =
            expected_published_parts(row.served_base, &api_id);
        match expected_authority {
            Some(expected_authority) => assert_eq!(
                authority, expected_authority,
                "case {case:?}: the stack must publish the api on {expected_authority}, got \
                 {published:?}"
            ),
            None => assert_published_host(&case, &authority),
        }
        assert_eq!(
            base_path, expected_base_path,
            "case {case:?}: the stack must publish the base path {expected_base_path:?}, got \
             {published:?}"
        );

        // 2. The value is the api's own, settled when it was minted: API Gateway v2 reports the
        // same address for its own read, and for one reached at a DIFFERENT endpoint from the one
        // the stack's create reached.
        let read = apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}")).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetApi for the published id {api_id:?}, got {} {:?}",
            read.status,
            read.text()
        );
        assert_eq!(
            read.json().get("apiEndpoint").and_then(Value::as_str),
            Some(published.as_str()),
            "case {case:?}: GetApi must report the published endpoint, got {:?}",
            read.text()
        );
        let read_locally =
            apigatewayv2_at(&router, "127.0.0.1:19999", "GET", &format!("/v2/apis/{api_id}")).await;
        assert_eq!(
            read_locally.status, 200,
            "case {case:?}: expected 200 from GetApi reached at another endpoint, got {} {:?}",
            read_locally.status,
            read_locally.text()
        );
        assert_eq!(
            read_locally.json().get("apiEndpoint").and_then(Value::as_str),
            Some(published.as_str()),
            "case {case:?}: the api's endpoint is settled when it is minted, so a reader at \
             another endpoint reports the same value, got {:?}",
            read_locally.text()
        );

        // 3. The invoke URL a client builds from the published value: the stage name appended,
        // which for the stage that serves the api's root is no segment at all.
        let invoke_path = format!("{base_path}{}{ROUTE_PATH}", api.stage_path);
        let answered = request(&router, &authority, &invoke_path).await;
        let dialled = format!("{case} [dialling {authority}{invoke_path}]");
        assert_claimed(&dialled, &answered);
        if api.disabled {
            assert_serves_nothing(&dialled, &answered);
        } else {
            assert_serves_its_route(&dialled, api.upstream, &answered);
        }
    }
}

// The data plane performs a route's integration as a task of the runtime the request is served
// on, so the suite drives the router on the flavour the emulator boots.
#[tokio::test(flavor = "multi_thread")]
async fn a_stack_publishes_an_api_endpoint_a_served_emulator_answers_on() {
    // The staged api's invoke path is its declared stage, so the table and the template are held
    // to each other — the api looked up by the logical id it is declared under, so reordering the
    // table cannot quietly check another row.
    let staged = DECLARED
        .iter()
        .find(|api| api.logical == "StagedApi")
        .expect("setup: the table must hold the api the template declares a named stage for");
    assert_eq!(
        staged.stage_path,
        format!("/{NAMED_STAGE}"),
        "setup: the staged api's stage path must be the stage the template declares"
    );
    for row in SERVED_ROWS {
        assert_row(row).await;
    }
}

/// An emulator served on no port at all has one address to announce: the api's own documented
/// `execute-api` host, which this emulator's front door claims host-style — so the published
/// value is served there too.
#[tokio::test(flavor = "multi_thread")]
async fn a_stack_publishes_an_api_endpoint_an_unserved_emulator_answers_on() {
    assert_row(&UNSERVED_ROW).await;
}
