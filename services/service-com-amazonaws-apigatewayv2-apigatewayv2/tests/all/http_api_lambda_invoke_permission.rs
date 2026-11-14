// SPDX-License-Identifier: CC0-1.0

//! The grant that admits a gateway's invocation of an `AWS_PROXY` route's FUNCTION: the
//! function's own resource-based policy, driven end to end over a deployed HTTP api stage.
//!
//! The rule these cases assert, one level above the recorded case (a CDK `HttpApi` whose
//! `AWS::Lambda::Permission` was stripped):
//!
//! > A gateway invokes an `AWS_PROXY` route's function as its OWN service principal
//! > (`apigateway.amazonaws.com`), so the invocation is made only where that function's
//! > resource-based policy allows that principal the `lambda:InvokeFunction` action with the
//! > ROUTE's own `execute-api` ARN as the `aws:SourceArn` such a grant may be narrowed by
//! > (`arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{method}/{path}`). An
//! > invocation the policy does not admit is NEVER MADE — the function does not run — and the
//! > gateway answers the route `500` `{"message":"Internal Server Error"}`, the status an HTTP
//! > api maps an integration that produced no response to.
//!
//! Nothing here is a value this emulator fills in: whether the function RAN is read from the
//! function's own side effect — the handler records every invocation in an Amazon SQS queue —
//! so a refused row asserts that the deployment package never ran, not merely that the gateway
//! answered `500`. AWS Lambda and Amazon SQS are therefore mounted beside this service.
//!
//! The rows span what the rule generalises over: no resource policy at all; a grant naming no
//! source at all; a grant narrowed to the api with wildcards, to this route exactly, to
//! ANOTHER route of the same api and to another api altogether; a grant to another service
//! principal; a grant of another action; and a grant that is REVOKED again. The recorded case
//! is the first row.
//!
//! The dispatch of an ADMITTED invocation — the event, the response mapping, the fault classes
//! — is `http_api_backend_dispatch.rs`; only the grant varies here.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::http_api_backend_dispatch::{
    gateway_may_invoke, given_api, given_ungranted_function, granted, lambda_integration, revoked,
};
use crate::websocket_handshake_data_plane::{message, send};

/// The Region every request below is signed for — the one `http_api_backend_dispatch` creates
/// its functions and apis in, so the ARNs here name the Region those records live in.
const REGION: &str = "us-east-1";

/// The status and sentence an HTTP api answers for an integration that produced no response,
/// which is what an invocation the function's policy does not admit falls into.
const INTEGRATION_FAULT: u16 = 500;
const INTEGRATION_FAULT_MESSAGE: &str = "Internal Server Error";

/// The route every row fronts, and the path a request to it carries under the stage that
/// serves the api's root.
const ROUTE_KEY: &str = "GET /items";
const ROUTE_PATH: &str = "/items";

/// The method and path a grant narrowed to THIS route names, and one of ANOTHER route of the
/// same api.
const ROUTE_METHOD: &str = "GET";
const OTHER_ROUTE_PATH: &str = "elsewhere";

/// The status the handler answers when it runs, so an admitted row is told from a refused one
/// by the answer as well as by the record.
const SERVED: u16 = 201;

/// How long a row waits for the record the function wrote, and how often it looks: the
/// invocation is synchronous, so the record is there by the time the route answered — the wait
/// is for the queue's own delivery and is generous for a loaded machine.
const RECORD_WAIT: Duration = Duration::from_secs(20);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// This service, AWS Lambda and Amazon SQS on ONE engine, with request authentication off:
/// the route's function lives in Lambda, and the queue it records each invocation in lives in
/// SQS.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One Amazon SQS call that must answer 200.
async fn sqs(emulator: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let host = format!("sqs.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: SQS {operation} must answer 200, got {status} {parsed}"
    );
    parsed
}

/// The queue the function records every invocation in.
async fn given_queue(emulator: &MultiServiceRouter, case: &str) -> String {
    let created = sqs(
        emulator,
        case,
        "CreateQueue",
        json!({ "QueueName": "lambda-invoke-permission-records" }),
    )
    .await;
    created
        .get("QueueUrl")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateQueue answered no QueueUrl: {created}"))
        .to_string()
}

/// The handler every row fronts: it records the invocation in `queue_url` and answers
/// [`SERVED`], so a row reads whether the function RAN from the queue rather than from the
/// status alone.
fn recording_handler(queue_url: &str) -> String {
    format!(
        "import json\n\
         import boto3\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   boto3.client(\"sqs\").send_message(\n\
         \x20       QueueUrl={queue:?},\n\
         \x20       MessageBody=json.dumps({{\"path\": event.get(\"rawPath\")}}),\n\
         \x20   )\n\
         \x20   return {{\"statusCode\": {SERVED}, \"body\": \"ran\"}}\n",
        queue = queue_url,
    )
}

/// How many records the queue holds, taking everything it has: a refused row must find none,
/// an admitted row exactly one.
async fn records(emulator: &MultiServiceRouter, case: &str, queue_url: &str) -> usize {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0 }),
    )
    .await;
    received
        .get("Messages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// Wait until the queue holds one record, and answer how many it held — for an ADMITTED row,
/// where the function's own `send_message` is a second in-process call that the route's answer
/// does not wait for.
async fn awaited_record(emulator: &MultiServiceRouter, case: &str, queue_url: &str) -> usize {
    let deadline = std::time::Instant::now() + RECORD_WAIT;
    loop {
        let held = records(emulator, case, queue_url).await;
        if held > 0 || std::time::Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// What one row grants on the function before the route is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// Nothing at all: the function carries no resource policy.
    None,
    /// `lambda:InvokeFunction` to the gateway, naming no source.
    AnySource,
    /// The same, narrowed to a source ARN the row builds from the api it created.
    Narrowed(Source),
    /// `lambda:InvokeFunction` to ANOTHER service principal.
    OtherPrincipal,
    /// Another action to the gateway.
    OtherAction,
    /// The unnarrowed grant, added and then revoked again.
    Revoked,
}

/// Which `aws:SourceArn` a narrowed grant names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Every stage, method and path of THIS api — the form a synthesised app grants.
    ThisApiWildcard,
    /// This route exactly: this api, this stage, this method, this path.
    ThisRoute,
    /// Another route of this api.
    AnotherRoute,
    /// Another api altogether.
    AnotherApi,
}

impl Source {
    /// The ARN this source names, for the api `api_id` and the account the emulator serves.
    fn arn(self, api_id: &str) -> String {
        let account = engine::emulator_account_id();
        let resource = match self {
            Self::ThisApiWildcard => format!("{api_id}/*/*/*"),
            Self::ThisRoute => format!("{api_id}/$default/{ROUTE_METHOD}{ROUTE_PATH}"),
            Self::AnotherRoute => {
                format!("{api_id}/$default/{ROUTE_METHOD}/{OTHER_ROUTE_PATH}")
            }
            Self::AnotherApi => "otherapi00/*/*/*".to_string(),
        };
        format!("arn:aws:execute-api:{REGION}:{account}:{resource}")
    }
}

/// One row: the grant the function carries, and whether the gateway's invocation is admitted.
struct Row {
    case: &'static str,
    grant: Grant,
    admitted: bool,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded case: the permission a synthesised app provisions is not there.
        case: "a function carrying no resource policy at all",
        grant: Grant::None,
        admitted: false,
    },
    Row {
        case: "a grant to the gateway naming no source",
        grant: Grant::AnySource,
        admitted: true,
    },
    Row {
        case: "a grant narrowed to every stage, method and path of this api",
        grant: Grant::Narrowed(Source::ThisApiWildcard),
        admitted: true,
    },
    Row {
        case: "a grant narrowed to this route exactly",
        grant: Grant::Narrowed(Source::ThisRoute),
        admitted: true,
    },
    Row {
        case: "a grant narrowed to another route of this api",
        grant: Grant::Narrowed(Source::AnotherRoute),
        admitted: false,
    },
    Row {
        case: "a grant narrowed to another api altogether",
        grant: Grant::Narrowed(Source::AnotherApi),
        admitted: false,
    },
    Row {
        case: "a grant to another service principal",
        grant: Grant::OtherPrincipal,
        admitted: false,
    },
    Row {
        case: "a grant of another action to the gateway",
        grant: Grant::OtherAction,
        admitted: false,
    },
    Row {
        case: "a grant that was added and then revoked",
        grant: Grant::Revoked,
        admitted: false,
    },
];

#[tokio::test(flavor = "multi_thread")]
async fn an_aws_proxy_route_invokes_its_function_only_where_the_functions_policy_admits_it() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let (emulator, engine) = emulator();
        let queue_url = given_queue(&emulator, case).await;
        let function = format!("permission-row-{index}");
        let arn = given_ungranted_function(
            &emulator,
            &engine,
            case,
            &function,
            &recording_handler(&queue_url),
        )
        .await;
        let (api_id, host) = given_api(
            &emulator,
            case,
            lambda_integration(&arn, "2.0"),
            ROUTE_KEY,
            json!({ "stageName": "$default", "autoDeploy": true }),
        )
        .await;

        match row.grant {
            Grant::None => {}
            Grant::AnySource => gateway_may_invoke(&emulator, case, &function, None).await,
            Grant::Narrowed(source) => {
                gateway_may_invoke(&emulator, case, &function, Some(&source.arn(&api_id))).await;
            }
            Grant::OtherPrincipal => {
                granted(
                    &emulator,
                    case,
                    &function,
                    "lambda:InvokeFunction",
                    "events.amazonaws.com",
                    None,
                )
                .await;
            }
            Grant::OtherAction => {
                granted(
                    &emulator,
                    case,
                    &function,
                    "lambda:GetFunction",
                    "apigateway.amazonaws.com",
                    None,
                )
                .await;
            }
            Grant::Revoked => {
                gateway_may_invoke(&emulator, case, &function, None).await;
                revoked(
                    &emulator,
                    case,
                    &function,
                    "lambda:InvokeFunction",
                    "apigateway.amazonaws.com",
                    None,
                )
                .await;
            }
        }

        let answer = send(&emulator, &host, ROUTE_METHOD, ROUTE_PATH, &[]).await;
        if row.admitted {
            assert_eq!(
                answer.status,
                SERVED,
                "case {case:?}: expected the function's own answer, got {}",
                answer.summary()
            );
            assert_eq!(
                awaited_record(&emulator, case, &queue_url).await,
                1,
                "case {case:?}: expected the function to have run exactly once"
            );
            continue;
        }
        assert_eq!(
            answer.status,
            INTEGRATION_FAULT,
            "case {case:?}: expected {INTEGRATION_FAULT} for an invocation the function's \
             policy does not admit, got {}",
            answer.summary()
        );
        assert_eq!(
            message(&answer),
            INTEGRATION_FAULT_MESSAGE,
            "case {case:?}: expected the gateway's own sentence, got {}",
            answer.summary()
        );
        // The invocation is synchronous — the route's answer comes after the function would
        // have returned — so the queue AT THIS INSTANT says whether the deployment package
        // ran at all.
        assert_eq!(
            records(&emulator, case, &queue_url).await,
            0,
            "case {case:?}: expected no invocation of the function at all"
        );
    }
}

/// The grant takes effect on the NEXT request, in both directions: the policy is read from
/// Lambda per invocation rather than held by this service, so adding the permission makes a
/// refused route serve and revoking it makes a serving route refuse — with no redeploy of the
/// stage and no change to the api.
#[tokio::test(flavor = "multi_thread")]
async fn a_permission_added_or_revoked_decides_the_next_request() {
    let case = "the grant is added and revoked while the stage serves";
    let (emulator, engine) = emulator();
    let queue_url = given_queue(&emulator, case).await;
    let function = "permission-lifecycle";
    let arn = given_ungranted_function(
        &emulator,
        &engine,
        case,
        function,
        &recording_handler(&queue_url),
    )
    .await;
    let (_api_id, host) = given_api(
        &emulator,
        case,
        lambda_integration(&arn, "2.0"),
        ROUTE_KEY,
        json!({ "stageName": "$default", "autoDeploy": true }),
    )
    .await;

    // Before the grant: refused, and nothing ran.
    let refused = send(&emulator, &host, ROUTE_METHOD, ROUTE_PATH, &[]).await;
    assert_eq!(
        refused.status,
        INTEGRATION_FAULT,
        "case {case:?}: expected {INTEGRATION_FAULT} before the permission exists, got {}",
        refused.summary()
    );
    assert_eq!(
        records(&emulator, case, &queue_url).await,
        0,
        "case {case:?}: expected no invocation before the permission exists"
    );

    // Added: the next request is served by the function itself.
    gateway_may_invoke(&emulator, case, function, None).await;
    let served = send(&emulator, &host, ROUTE_METHOD, ROUTE_PATH, &[]).await;
    assert_eq!(
        served.status,
        SERVED,
        "case {case:?}: expected the function's own answer once the permission exists, got {}",
        served.summary()
    );
    assert_eq!(
        awaited_record(&emulator, case, &queue_url).await,
        1,
        "case {case:?}: expected exactly the one invocation the permission admitted"
    );

    // Revoked: the next request is refused again, with nothing run for it.
    revoked(
        &emulator,
        case,
        function,
        "lambda:InvokeFunction",
        "apigateway.amazonaws.com",
        None,
    )
    .await;
    let refused = send(&emulator, &host, ROUTE_METHOD, ROUTE_PATH, &[]).await;
    assert_eq!(
        refused.status,
        INTEGRATION_FAULT,
        "case {case:?}: expected {INTEGRATION_FAULT} once the permission is revoked, got {}",
        refused.summary()
    );
    assert_eq!(
        records(&emulator, case, &queue_url).await,
        0,
        "case {case:?}: expected no invocation once the permission is revoked"
    );
}
