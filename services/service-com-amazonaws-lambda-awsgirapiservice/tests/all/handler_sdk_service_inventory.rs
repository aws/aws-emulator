// SPDX-License-Identifier: CC0-1.0

//! WHICH services the AWS SDK a running Python handler imports answers to — the inventory a
//! `boto3.client(...)` token is resolved against — end to end over the wire.
//!
//! RULE: a managed runtime ships the AWS SDK on the handler's import path with the catalog of
//! the services it can call, so a client built for a service the runtime's call seam binds is
//! CONSTRUCTED AND USED rather than refused, whichever protocol that service speaks, and
//! `botocore.exceptions.UnknownServiceError` is raised for a token no service answers to and
//! for nothing else — carrying botocore's own sentence, which names the token asked for and
//! lists the catalog.
//!
//! The rows differ along what the rule generalises over: the protocol the named service binds
//! its requests with (an AWS JSON one, `restJson1`, and the object store's own REST protocol,
//! which is neither), which spelling of its SDK id the token uses (the words run together and
//! hyphenated, as the JavaScript SDK's client package spells them), and tokens no service
//! answers to at all (a real AWS service this runtime binds no requests for, a near miss on
//! one it does, and the empty token). One row — the object store, whose protocol is its own —
//! also asserts the URL its client PRESIGNS, so a token that is merely listed but bound to
//! nothing fails here.
//!
//! Every row runs in ONE invocation: the probe presigns rather than sends, so no row needs the
//! named service mounted, and an invocation is the dearest thing this target does.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::aws_api::reachable_services;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{await_function_active, call, router_on};
// The one .zip writer this crate's suites package a handler with, exported by the suite that
// first needed it rather than copied here.
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region the function runs in, which its clients sign and address for.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The bucket and key the object-store row presigns a read of.
const BUCKET: &str = "orders-archive";
const KEY: &str = "orders/one.txt";

/// The service tokens the runtime's SDK inventory must answer to.
///
/// Source: the services this crate's call seam embeds a Smithy model for and binds requests to
/// — the `*_MODEL` includes in `src/state/execution/aws_api.rs`, one per
/// `services/service-com-amazonaws-<service>-*` crate — each named here by the token the AWS
/// SDK for Python builds a client for it with. A service whose model is embedded and whose
/// protocol is bound, but whose token the inventory omits, is one no handler can call at all.
const MUST_REACH: &[&str] = &[
    "apigateway",
    "apigatewaymanagementapi",
    "dynamodb",
    "events",
    "logs",
    "opensearch",
    "rekognition",
    "s3",
    "secretsmanager",
    "sesv2",
    "sfn",
    "sns",
    "sqs",
    "xray",
];

/// One row: the token a handler builds its client with, and whether the runtime's SDK answers
/// to it.
struct Row {
    case: &'static str,
    /// What the handler passes to `boto3.client(...)`.
    token: &'static str,
    /// Whether a service answers to that token, so the client is built and used rather than
    /// refused as unknown.
    reachable: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an AWS JSON service, by the token its SDK id gives it",
            token: "sqs",
            reachable: true,
        },
        Row {
            case: "another AWS JSON service, named by its endpoint prefix",
            token: "events",
            reachable: true,
        },
        Row {
            case: "the same service by the other spelling of its SDK id",
            token: "eventbridge",
            reachable: true,
        },
        Row {
            case: "a restJson1 service",
            token: "apigateway",
            reachable: true,
        },
        Row {
            case: "another restJson1 service, signing under a different name",
            token: "sesv2",
            reachable: true,
        },
        Row {
            case: "the data plane of a WebSocket api, whose backend pushes to its connections",
            token: "apigatewaymanagementapi",
            reachable: true,
        },
        Row {
            case: "a multi-word SDK id with the words run together",
            token: "secretsmanager",
            reachable: true,
        },
        Row {
            case: "the same service with the words hyphenated, as the JavaScript SDK spells it",
            token: "secrets-manager",
            reachable: true,
        },
        Row {
            case: "a multi-word SDK id hyphenated, for another service",
            token: "cloudwatch-logs",
            reachable: true,
        },
        Row {
            case: "a workflow service, by the token the Python SDK names it",
            token: "stepfunctions",
            reachable: true,
        },
        Row {
            case: "the same workflow service by its own SDK id",
            token: "sfn",
            reachable: true,
        },
        Row {
            case: "a search service, by its endpoint prefix",
            token: "es",
            reachable: true,
        },
        Row {
            case: "a tracing service",
            token: "xray",
            reachable: true,
        },
        Row {
            case: "the object store, whose protocol is neither of the JSON ones",
            token: "s3",
            reachable: true,
        },
        Row {
            case: "the content delivery service, whose protocol is the other XML one",
            token: "cloudfront",
            reachable: true,
        },
        Row {
            case: "the notification service, whose protocol is a form-encoded action",
            token: "sns",
            reachable: true,
        },
        Row {
            case: "an image analysis service, whose handler-side calls an object pipeline makes",
            token: "rekognition",
            reachable: true,
        },
        Row {
            case: "a real AWS service this runtime binds no requests for",
            token: "ec2",
            reachable: false,
        },
        Row {
            case: "a near miss on a token that is answered to",
            token: "s3control",
            reachable: false,
        },
        Row {
            case: "the empty token",
            token: "",
            reachable: false,
        },
    ]
}

/// The handler every row runs: it builds a client for each token the event names and asks that
/// client to PRESIGN a read, which resolves the token against the runtime's SDK inventory and
/// sends nothing.
///
/// What it reports per token is the exception class the SDK raised, if any, and its sentence —
/// so what is asserted is what the SDK gave the guest, not what this suite decided.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    probed = {}
    for token in event["tokens"]:
        client = boto3.client(token)
        try:
            url = client.generate_presigned_url(
                "get_object",
                Params={"Bucket": event["bucket"], "Key": event["key"]},
                ExpiresIn=120,
            )
        except botocore.exceptions.UnknownServiceError as error:
            probed[token] = {"raised": "UnknownServiceError", "sentence": str(error)}
        except botocore.exceptions.BotoCoreError as error:
            probed[token] = {"raised": type(error).__name__, "sentence": str(error)}
        else:
            probed[token] = {"raised": None, "url": url}
    return {"probed": probed, "region": client.meta.region_name}
"#;

/// Lambda alone: the probe presigns rather than sends, so no row's service is mounted.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    router_on(&engine)
}

/// The sentence botocore renders for a token no service answers to: its own
/// `UnknownServiceError` template, filled with the token asked for and the catalog the runtime
/// holds.
fn unknown_service_sentence(token: &str) -> String {
    format!(
        "Unknown service: '{token}'. Valid service names are: {}",
        reachable_services().join(", ")
    )
}

#[tokio::test]
async fn the_runtimes_sdk_answers_to_every_service_token_its_call_seam_binds() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler resolving service tokens against the runtime's SDK";
    let router = emulator();

    let caller = "sdk-service-inventory-caller";
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": caller,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 20,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("lambda_function.py", HANDLER_SOURCE)])),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(&router, case, caller).await;

    let rows = rows();
    let tokens: Vec<&str> = rows.iter().map(|row| row.token).collect();
    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{caller}/invocations"),
        &json!({ "tokens": tokens, "bucket": BUCKET, "key": KEY }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    let payload = answered.json();
    let probed = payload["probed"].clone();

    for row in &rows {
        let reported = probed[row.token].clone();
        assert!(
            reported.is_object(),
            "case {:?}: expected the handler to report the token {:?}, got {reported:?}",
            row.case,
            row.token
        );
        let raised = reported["raised"].as_str();
        if row.reachable {
            assert_ne!(
                raised,
                Some("UnknownServiceError"),
                "case {:?}: expected the token {:?} to name a service the SDK answers to, got \
                 {reported:?}",
                row.case,
                row.token
            );
        } else {
            assert_eq!(
                raised,
                Some("UnknownServiceError"),
                "case {:?}: expected UnknownServiceError for the token {:?}, got {reported:?}",
                row.case,
                row.token
            );
            assert_eq!(
                reported["sentence"].as_str(),
                Some(unknown_service_sentence(row.token).as_str()),
                "case {:?}: expected botocore's unknown-service sentence for {:?}, got \
                 {reported:?}",
                row.case,
                row.token
            );
        }
    }

    // The catalog the runtime REPORTS is the one its call seam holds, so the sentence a guest
    // reads lists exactly the services a call may be made to.
    let listed: Vec<String> = probed["ec2"]["sentence"]
        .as_str()
        .and_then(|sentence| sentence.split_once("Valid service names are: "))
        .map(|(_before, catalog)| catalog.split(", ").map(str::to_string).collect())
        .unwrap_or_default();
    assert_eq!(
        listed,
        reachable_services(),
        "case {case:?}: expected the reported catalog to be the seam's own, got {listed:?}"
    );
    for token in MUST_REACH {
        assert!(
            listed.iter().any(|named| named == token),
            "case {case:?}: expected the reported catalog to name {token:?}, got {listed:?}"
        );
    }

    // The object store's client is bound as well as listed: the URL it presigns addresses this
    // emulator's own object endpoint and carries a SigV4 query signature over that one request.
    let object_url = probed["s3"]["url"].as_str().unwrap_or_default();
    assert!(
        object_url.starts_with(&format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/{KEY}?"))
            && object_url.contains("X-Amz-Signature=")
            && object_url.contains("X-Amz-Expires=120"),
        "case {case:?}: expected a presigned object URL for the object store, got \
         {object_url:?}"
    );

    // A client the SDK answers to is one the guest can keep using: the region it reports is the
    // one the invocation runs in, read from the run's own environment rather than passed in.
    assert_eq!(
        payload["region"].as_str(),
        Some(REGION),
        "case {case:?}: expected the client to report the run's Region, got {}",
        payload["region"]
    );
}

/// RULE, the other half: the catalog the runtime reports holds each service ONCE, under every
/// token that names it, so a guest reading the sentence reads a list of services rather than a
/// list of spellings.
#[test]
fn the_reported_catalog_names_each_token_once() {
    let listed = reachable_services();
    for token in MUST_REACH {
        assert!(
            listed.iter().any(|named| named == token),
            "case {token:?}: expected the call seam to bind requests for it, got {listed:?}"
        );
    }
    let mut once = listed.clone();
    once.sort_unstable();
    once.dedup();
    assert_eq!(
        once.len(),
        listed.len(),
        "case {listed:?}: expected each token once, got {once:?}"
    );
}
