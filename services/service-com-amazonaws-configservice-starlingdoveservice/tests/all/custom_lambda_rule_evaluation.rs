// SPDX-License-Identifier: CC0-1.0

//! What a Config CUSTOM LAMBDA rule's own evaluator does, end to end.
//!
//! The rule under test: **a Config rule whose `Source.Owner` is `CUSTOM_LAMBDA` is
//! evaluated by the function the account points it at, not by Config: for every evaluation
//! pass Config invokes — the one a write of the rule starts, and the one
//! `StartConfigRulesEvaluation` asks for — Config INVOKES that function with the invoking
//! event of the trigger the rule declares and the `ResultToken` of that pass, and the
//! verdict the function delivers with `PutEvaluations` is what the rule's compliance reads
//! then report. One invocation per pass: a pump that runs every few hundred milliseconds
//! does not invoke the function again for a pass it has already invoked.**
//!
//! Nothing here computes a verdict: the `COMPLIANT`/`NON_COMPLIANT` each row asserts is
//! the one its own handler decided, in Python, from the event Config handed it — which is
//! why the rows differ along what the handler answers and along what the event has to
//! carry for it to answer at all.
//!
//! And because the invocation is Config's own work rather than the clock's, what
//! `DescribeConfigRuleEvaluationStatus` reports about such a rule is read off the
//! invocations that were MADE: the successful invocation is a call that reached the
//! function, the successful evaluation is a verdict an evaluator delivered, and a call that
//! could not be made is reported as the failed invocation it was.
//!
//! One row set is about the DELIVERY itself rather than about Config: an evaluator reports what
//! it found through its own SDK client, and such a client takes a timestamp member in whatever
//! form its caller has it — the `configurationItemCaptureTime` the configuration item carries
//! is TEXT — so every form of the same instant delivers the same verdict.
//!
//! Config, IAM and Lambda are mounted on ONE engine, as `core/router/src/lib.rs` mounts
//! them: the function runs in process and the SDK call its handler makes is dispatched back
//! into this same router, which is how its verdict reaches Config.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";
/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The runtime every function below runs, and the module and attribute its package exposes
/// its handler as.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long one invocation of a handler is given, as the function's own `Timeout`: well
/// past what an interpreted run needs.
const FUNCTION_TIMEOUT: i64 = 30;

/// How long a poll waits for work the clock owes — the function becoming `Active`, the
/// evaluation pass, the invocation and the delivery it makes. Many passes' worth of slack,
/// not a span the behaviour depends on.
const BUDGET: Duration = Duration::from_secs(30);

/// How long a row watches for an invocation nothing asked for.
const QUIET: Duration = Duration::from_secs(2);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The resource type the handler reports its verdict about when the event carries no
/// configuration item: the account pseudo-resource, the one
/// `ExternalEvaluation$ComplianceResourceType` names ("Config accepts `AWS::::Account`
/// resource type") and the one a periodic rule's evaluator judges.
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// The resource type Config records an S3 bucket under, which is what a change-triggered
/// rule's event carries the configuration item of.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// Config, IAM and Lambda over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    callback: smithy_server_runtime::EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// An S3 client over the in-process callback, addressing path style exactly as the
    /// recorder's own client does: a bucket is the recorded resource a change-triggered
    /// rule is invoked for.
    fn s3(&self) -> aws_sdk_s3::Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
            .force_path_style(true)
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }

    /// An IAM client over the in-process callback.
    fn iam(&self) -> aws_sdk_iam::Client {
        service_common::iam_client::iam_client_through(&self.callback, REGION)
    }

    /// Put the configuration recorder a Config rule needs — "you must create a
    /// configuration recorder before you can create or update a Config rule" — with its own
    /// role created in IAM first, which is what lets Config be shown able to assume it.
    async fn given_recorder(&self) {
        let role = self
            .iam()
            .create_role()
            .role_name("config-role")
            .assume_role_policy_document(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "config.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            )
            .send()
            .await
            .expect("creating the recorder's role must succeed")
            .role
            .expect("CreateRole answers the role")
            .arn;
        self.ok(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": "the-recorder",
                    "roleARN": role,
                    "recordingGroup": { "allSupported": true },
                }
            }),
        )
        .await;
    }

    /// The same recorder, with the delivery channel a recorder needs and recording
    /// STARTED, so the pass records the account's resources: a change-triggered rule is
    /// invoked by a recorded configuration item.
    async fn recording(&self) {
        self.given_recorder().await;
        self.ok(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-delivery" } }),
        )
        .await;
        self.ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "the-recorder" }),
        )
        .await;
    }

    /// Create `bucket` through S3's own API and wait until Config's inventory holds it, so
    /// a rule written afterwards is one the recorded item can trigger.
    async fn given_recorded_bucket(&self, case: &str, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: creating {bucket} must succeed, got {error:?}")
            });
        let deadline = Instant::now() + BUDGET;
        loop {
            let listed = self
                .ok(
                    "ListDiscoveredResources",
                    &json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceIds": [bucket] }),
                )
                .await;
            if listed["resourceIdentifiers"]
                .as_array()
                .is_some_and(|identifiers| !identifiers.is_empty())
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {bucket} to be recorded, got {listed}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    /// One Config call (`awsJson1_1`), answering the HTTP status and the parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/config/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("config.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("StarlingDoveService.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A Config call that must answer 200.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One Lambda `restJson1` call, as its status and parsed body.
    async fn lambda(&self, method: &str, uri: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body
            .map(|value| value.to_string().into_bytes())
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
///
/// Shared with the suite that pins which Custom Lambda rules Config will take at all, so
/// one statement of how a deployment package is written serves both.
pub(crate) fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler every row runs: it reads the event Config handed it and delivers ONE
/// evaluation back with `PutEvaluations`, spending the `resultToken` that event carries —
/// which is what "this operation is required in every Lambda function that is invoked by an
/// Config rule" asks of it.
///
/// The verdict it delivers is `verdict`; the annotation names the `messageType` of the
/// invoking event it was given, and the resource it reports the verdict ABOUT is the
/// configuration item that event carried, or the account when it carried none — so what
/// Config handed the function is observable in what Config then reports. A handler reaching
/// this with no `resultToken` raises, which fails the row rather than passing it with a
/// verdict nothing asked for.
fn handler_source(verdict: &str) -> String {
    format!(
        r#"
import json

import boto3

CONFIG = boto3.client("config")


def lambda_handler(event, context):
    token = event["resultToken"]
    invoking = json.loads(event["invokingEvent"])
    item = invoking.get("configurationItem")
    if item is None:
        resource_type = "{ACCOUNT_RESOURCE_TYPE}"
        resource_id = event["accountId"]
    else:
        resource_type = item["resourceType"]
        resource_id = item["resourceId"]
    CONFIG.put_evaluations(
        Evaluations=[
            {{
                "ComplianceResourceType": resource_type,
                "ComplianceResourceId": resource_id,
                "ComplianceType": "{verdict}",
                "Annotation": invoking["messageType"],
                "OrderingTimestamp": 0,
            }}
        ],
        ResultToken=token,
    )
    return {{"delivered": "{verdict}"}}
"#
    )
}

/// Create the function `name` from `source`, wait until it reports `Active`, and answer its
/// ARN.
///
/// The function is created with the resource-based policy a Config rule's evaluator needs:
/// Config invokes it as its own service principal, so `AddPermission` has to grant
/// `config.amazonaws.com` `lambda:InvokeFunction` on it before Config will take a rule
/// pointing at it at all.
async fn given_function(case: &str, emulator: &Emulator, name: &str, source: &str) -> String {
    let (status, created) = emulator
        .lambda(
            "POST",
            FUNCTIONS_URI,
            Some(&json!({
                "FunctionName": name,
                "Role": format!(
                    "arn:aws:iam::{}:role/config-rule-evaluator",
                    engine::emulator_account_id()
                ),
                "Runtime": RUNTIME,
                "Handler": HANDLER,
                "Timeout": FUNCTION_TIMEOUT,
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
                },
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction answered {status} {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {created}"))
        .to_string();
    let (status, granted) = emulator
        .lambda(
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/policy"),
            Some(&json!({
                "StatementId": "AllowExecutionFromConfig",
                "Action": "lambda:InvokeFunction",
                "Principal": "config.amazonaws.com",
                "SourceAccount": engine::emulator_account_id(),
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "case {case:?}: AddPermission answered {status} {granted}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = emulator
            .lambda("GET", &format!("{FUNCTIONS_URI}/{name}/configuration"), None)
            .await;
        if status == 200 && read["State"] == json!("Active") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the function to become Active, got {status} {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Write the rule `name` pointing at `function`, with the source details `details`.
async fn put_rule(emulator: &Emulator, name: &str, function: &str, details: Value) {
    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": name,
                    "Source": {
                        "Owner": "CUSTOM_LAMBDA",
                        "SourceIdentifier": function,
                        "SourceDetails": details,
                    },
                    "MaximumExecutionFrequency": "TwentyFour_Hours",
                }
            }),
        )
        .await;
}

/// The evaluation results the rule `name` reports, as
/// `(ComplianceType, Annotation, ResourceType, ResourceId)` rows.
async fn reported(emulator: &Emulator, name: &str) -> Vec<(String, String, String, String)> {
    let body = emulator
        .ok(
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": name }),
        )
        .await;
    body["EvaluationResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| {
            let qualifier =
                &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
            (
                result["ComplianceType"].as_str().unwrap_or_default().to_string(),
                result["Annotation"].as_str().unwrap_or_default().to_string(),
                qualifier["ResourceType"].as_str().unwrap_or_default().to_string(),
                qualifier["ResourceId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Read the rule's compliance details until they report something, and answer what they
/// reported.
async fn await_reported(
    case: &str,
    emulator: &Emulator,
    name: &str,
) -> Vec<(String, String, String, String)> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let results = reported(emulator, name).await;
        if !results.is_empty() {
            return results;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the rule's evaluator to deliver a verdict, got nothing"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_custom_lambda_rules_own_evaluator_delivers_the_verdict_config_reports() {
    // The rows differ along the verdict the function's own handler decides on and along
    // which trigger the rule declares — the periodic one Config hands a scheduled
    // notification, and the change-triggered one it hands a configuration item.
    let cases: [(&str, &str, Value, &str); 4] = [
        (
            "a scheduled rule whose evaluator finds the account non-compliant",
            "NON_COMPLIANT",
            json!([{
                "EventSource": "aws.config",
                "MessageType": "ScheduledNotification",
                "MaximumExecutionFrequency": "TwentyFour_Hours",
            }]),
            "ScheduledNotification",
        ),
        (
            "a scheduled rule whose evaluator finds the account compliant",
            "COMPLIANT",
            json!([{
                "EventSource": "aws.config",
                "MessageType": "ScheduledNotification",
                "MaximumExecutionFrequency": "TwentyFour_Hours",
            }]),
            "ScheduledNotification",
        ),
        (
            "a rule that declares no trigger at all",
            "NOT_APPLICABLE",
            json!([]),
            "ScheduledNotification",
        ),
        (
            "an evaluator that reports the account's data is not applicable",
            "NOT_APPLICABLE",
            json!([{
                "EventSource": "aws.config",
                "MessageType": "ScheduledNotification",
                "MaximumExecutionFrequency": "TwentyFour_Hours",
            }]),
            "ScheduledNotification",
        ),
    ];

    for (case, verdict, details, message_type) in cases {
        let emulator = Emulator::new();
        emulator.given_recorder().await;
        let function =
            given_function(case, &emulator, "rule-evaluator", &handler_source(verdict)).await;
        let rule = "lambda-backed-rule";
        put_rule(&emulator, rule, &function, details).await;

        // The pass a WRITE of the rule invokes is enough on its own: Config invokes the
        // evaluator without any further call. A periodic rule's event carries no
        // configuration item, so its evaluator reports the ACCOUNT.
        let results = await_reported(case, &emulator, rule).await;
        assert_eq!(
            results,
            vec![(
                verdict.to_string(),
                message_type.to_string(),
                ACCOUNT_RESOURCE_TYPE.to_string(),
                engine::emulator_account_id().to_string(),
            )],
            "case {case:?}: expected the verdict the evaluator delivered, got {results:?}"
        );

        // One invocation per pass: nothing invokes the function again for a pass that has
        // already been invoked, so the rule keeps reporting the one verdict it was
        // delivered.
        tokio::time::sleep(QUIET).await;
        let again = reported(&emulator, rule).await;
        assert_eq!(
            again, results,
            "case {case:?}: expected the delivered verdict to stand, got {again:?}"
        );

        // And the pass `StartConfigRulesEvaluation` asks for invokes the evaluator again,
        // with a token of its own, which is what lets a client re-trigger a rule.
        emulator
            .ok(
                "StartConfigRulesEvaluation",
                &json!({ "ConfigRuleNames": [rule] }),
            )
            .await;
        let after = await_reported(case, &emulator, rule).await;
        assert_eq!(
            after, results,
            "case {case:?}: expected the re-triggered evaluation to deliver the same verdict, \
             got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_change_triggered_rules_evaluator_is_handed_the_recorded_configuration_item() {
    // The other trigger: a rule that asks for the configuration-item notification is
    // invoked for each recorded resource its scope covers, with that item's own
    // notification — so what the evaluator reports its verdict ABOUT is the resource the
    // event carried, not the account.
    //
    // The rows differ along what the rule's scope narrows to: nothing at all, the recorded
    // resource's own type, and a type the account holds nothing of.
    let cases: [(&str, Option<Value>, bool); 3] = [
        ("no scope at all", None, true),
        (
            "a scope naming the recorded resource's type",
            Some(json!({ "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] })),
            true,
        ),
        (
            "a scope naming a type the account holds nothing of",
            Some(json!({ "ComplianceResourceTypes": ["AWS::EC2::Volume"] })),
            false,
        ),
    ];

    for (case, scope, invoked) in cases {
        let emulator = Emulator::new();
        emulator.recording().await;
        let bucket = "change-triggered-bucket";
        emulator.given_recorded_bucket(case, bucket).await;
        let function = given_function(
            case,
            &emulator,
            "change-evaluator",
            &handler_source("NON_COMPLIANT"),
        )
        .await;

        let rule = "change-triggered-rule";
        let mut body = json!({
            "ConfigRule": {
                "ConfigRuleName": rule,
                "Source": {
                    "Owner": "CUSTOM_LAMBDA",
                    "SourceIdentifier": function,
                    "SourceDetails": [{
                        "EventSource": "aws.config",
                        "MessageType": "ConfigurationItemChangeNotification",
                    }],
                },
            }
        });
        if let Some(scope) = scope {
            body["ConfigRule"]["Scope"] = scope;
        }
        emulator.ok("PutConfigRule", &body).await;

        if invoked {
            let results = await_reported(case, &emulator, rule).await;
            assert_eq!(
                results,
                vec![(
                    "NON_COMPLIANT".to_string(),
                    "ConfigurationItemChangeNotification".to_string(),
                    BUCKET_RESOURCE_TYPE.to_string(),
                    bucket.to_string(),
                )],
                "case {case:?}: expected the evaluator's verdict on the recorded bucket, got \
                 {results:?}"
            );
        } else {
            // A rule whose scope covers nothing the account holds has no item to be
            // triggered by, so its evaluator is never invoked and no verdict is recorded —
            // watched for a window many passes long rather than read once.
            let deadline = Instant::now() + QUIET;
            while Instant::now() < deadline {
                let results = reported(&emulator, rule).await;
                assert!(
                    results.is_empty(),
                    "case {case:?}: expected no verdict at all, got {results:?}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

/// A handler that delivers NOTHING: it is handed the event and returns without calling
/// `PutEvaluations` at all, which is what lets the counter-case below observe that Config
/// records no verdict of its own.
const SILENT_HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    return {"delivered": None}
"#;

/// A handler that delivers its verdict with `OrderingTimestamp` written the way `expression`
/// writes it — "the time of the event that triggered the evaluation"
/// (`Evaluation$OrderingTimestamp`), as the kinds of value a client's own SDK sends for a
/// timestamp member.
///
/// Everything else is [`handler_source`]'s delivery: the verdict is `COMPLIANT` and the
/// annotation names the `messageType` the event carried, so a row that delivered at all is
/// observable in what Config reports.
fn timestamp_handler_source(expression: &str) -> String {
    format!(
        r#"
import datetime
import json

import boto3

CONFIG = boto3.client("config")


def lambda_handler(event, context):
    invoking = json.loads(event["invokingEvent"])
    item = invoking.get("configurationItem") or {{}}
    CONFIG.put_evaluations(
        Evaluations=[
            {{
                "ComplianceResourceType": item.get("resourceType", "{ACCOUNT_RESOURCE_TYPE}"),
                "ComplianceResourceId": item.get("resourceId", event["accountId"]),
                "ComplianceType": "COMPLIANT",
                "Annotation": invoking["messageType"],
                "OrderingTimestamp": {expression},
            }}
        ],
        ResultToken=event["resultToken"],
    )
    return {{"delivered": "COMPLIANT"}}
"#
    )
}

#[tokio::test]
async fn an_evaluator_delivers_its_verdict_whichever_way_its_sdk_writes_the_timestamp() {
    // `Evaluation$OrderingTimestamp` is a timestamp member, and an SDK client sends one from
    // whatever its caller handed it: a `datetime`, a number of seconds since the epoch, or
    // the ISO-8601 text a configuration item carries its own instants as. Each is the same
    // instant on the wire, so each delivery is one Config records.
    //
    // The rows differ along which of those three the evaluator's own call writes.
    let cases: [(&str, &str); 4] = [
        ("an aware datetime", "datetime.datetime.now(datetime.timezone.utc)"),
        ("a naive datetime", "datetime.datetime(2026, 1, 2, 3, 4, 5)"),
        ("a number of seconds since the epoch", "0"),
        (
            "the ISO-8601 instant the configuration item carries",
            r#"item.get("configurationItemCaptureTime", "2026-01-02T03:04:05.000Z")"#,
        ),
    ];

    for (case, expression) in cases {
        let emulator = Emulator::new();
        emulator.recording().await;
        let bucket = "timestamp-delivery-bucket";
        emulator.given_recorded_bucket(case, bucket).await;
        let function = given_function(
            case,
            &emulator,
            "timestamp-evaluator",
            &timestamp_handler_source(expression),
        )
        .await;

        let rule = "timestamp-rule";
        emulator
            .ok(
                "PutConfigRule",
                &json!({
                    "ConfigRule": {
                        "ConfigRuleName": rule,
                        "Scope": { "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] },
                        "Source": {
                            "Owner": "CUSTOM_LAMBDA",
                            "SourceIdentifier": function,
                            "SourceDetails": [{
                                "EventSource": "aws.config",
                                "MessageType": "ConfigurationItemChangeNotification",
                            }],
                        },
                    }
                }),
            )
            .await;

        let results = await_reported(case, &emulator, rule).await;
        assert_eq!(
            results,
            vec![(
                "COMPLIANT".to_string(),
                "ConfigurationItemChangeNotification".to_string(),
                BUCKET_RESOURCE_TYPE.to_string(),
                bucket.to_string(),
            )],
            "case {case:?}: expected the evaluator's delivered verdict, got {results:?}"
        );
    }
}

#[tokio::test]
async fn an_evaluator_that_delivers_nothing_leaves_the_rule_unevaluated() {
    // The counter-case: the verdict of a Custom Lambda rule is the function's own. The
    // function is there and its policy admits Config, so the invocation is made — and
    // because the function delivers no evaluation, Config reports none rather than
    // inventing one of its own.
    let case = "an evaluator that delivers nothing";
    let emulator = Emulator::new();
    emulator.given_recorder().await;
    let function =
        given_function(case, &emulator, "silent-evaluator", SILENT_HANDLER_SOURCE).await;
    let rule = "rule-whose-evaluator-is-silent";
    put_rule(
        &emulator,
        rule,
        &function,
        json!([{
            "EventSource": "aws.config",
            "MessageType": "ScheduledNotification",
            "MaximumExecutionFrequency": "TwentyFour_Hours",
        }]),
    )
    .await;
    // Watched for a window many passes long: the rule stays unevaluated for every read in
    // it, rather than merely for the first one.
    let deadline = Instant::now() + QUIET;
    while Instant::now() < deadline {
        let results = reported(&emulator, rule).await;
        assert!(
            results.is_empty(),
            "case {case:?}: expected no verdict at all, got {results:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The rule `name`'s own `ConfigRuleEvaluationStatus`, as
/// `DescribeConfigRuleEvaluationStatus` reports it.
async fn evaluation_status(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeConfigRuleEvaluationStatus",
            &json!({ "ConfigRuleNames": [name] }),
        )
        .await;
    body["ConfigRulesEvaluationStatus"]
        .as_array()
        .and_then(|statuses| statuses.first().cloned())
        .unwrap_or_else(|| panic!("expected a status for the rule {name:?}, got {body}"))
}

/// Whether `status` carries the member `member` at all, which is what tells an instant
/// Config recorded from one it did not.
fn carries(status: &Value, member: &str) -> bool {
    !status[member].is_null()
}

/// Read the rule `name`'s evaluation status until it carries `member`, and answer that
/// status.
///
/// What it waits for is work Config owes by the clock — the pass, the invocation it makes
/// and the delivery that answers it — so it looks again until the member is there rather
/// than after a fixed wait.
async fn await_status_carrying(
    case: &str,
    emulator: &Emulator,
    name: &str,
    member: &str,
) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let status = evaluation_status(emulator, name).await;
        if carries(&status, member) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the status to carry {member}, got {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_evaluation_status_reports_only_the_invocations_that_were_made() {
    // The status is Config's record of its OWN side of a rule's evaluation: for a rule
    // whose verdicts are delivered back, the invocation it reports as successful is one it
    // made and the evaluation it reports as successful is one an evaluator delivered — so a
    // rule whose evaluator Config never had occasion to invoke reports neither, however
    // many passes of the clock go by.
    //
    // The rows differ along whether anything in the rule's scope was ever there to invoke
    // it for: a recorded resource of the type the scope names, and a type the account holds
    // nothing of.
    let cases: [(&str, Value, bool); 2] = [
        (
            "a scope naming the recorded resource's type",
            json!({ "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] }),
            true,
        ),
        (
            "a scope naming a type the account holds nothing of",
            json!({ "ComplianceResourceTypes": ["AWS::EC2::Volume"] }),
            false,
        ),
    ];

    for (case, scope, invoked) in cases {
        let emulator = Emulator::new();
        emulator.recording().await;
        let bucket = "status-honesty-bucket";
        emulator.given_recorded_bucket(case, bucket).await;
        let function = given_function(
            case,
            &emulator,
            "status-evaluator",
            &handler_source("COMPLIANT"),
        )
        .await;

        let rule = "status-rule";
        emulator
            .ok(
                "PutConfigRule",
                &json!({
                    "ConfigRule": {
                        "ConfigRuleName": rule,
                        "Scope": scope,
                        "Source": {
                            "Owner": "CUSTOM_LAMBDA",
                            "SourceIdentifier": function,
                            "SourceDetails": [{
                                "EventSource": "aws.config",
                                "MessageType": "ConfigurationItemChangeNotification",
                            }],
                        },
                    }
                }),
            )
            .await;

        if invoked {
            // The verdict is the evidence the evaluator ran and delivered; the invocation
            // Config made is recorded when the call it made answers, so the status is read
            // until it carries that instant rather than after a fixed wait.
            await_reported(case, &emulator, rule).await;
            let status =
                await_status_carrying(case, &emulator, rule, "LastSuccessfulInvocationTime")
                    .await;
            assert!(
                carries(&status, "LastSuccessfulEvaluationTime")
                    && status["FirstEvaluationStarted"] == json!(true),
                "case {case:?}: expected the delivery to be reported as the rule's \
                 evaluation, got {status}"
            );
            assert!(
                !carries(&status, "LastFailedInvocationTime")
                    && !carries(&status, "LastFailedEvaluationTime")
                    && !carries(&status, "LastErrorCode"),
                "case {case:?}: expected nothing to be reported as failed, got {status}"
            );
        } else {
            // Nothing in scope, so nothing invoked the evaluator: watched for a window many
            // passes long, because what is asserted is that the clock alone never makes the
            // status claim an invocation.
            let deadline = Instant::now() + QUIET;
            while Instant::now() < deadline {
                let status = evaluation_status(&emulator, rule).await;
                assert!(
                    !carries(&status, "LastSuccessfulInvocationTime")
                        && !carries(&status, "LastSuccessfulEvaluationTime")
                        && status["FirstEvaluationStarted"] == json!(false),
                    "case {case:?}: expected no invocation or evaluation to be claimed, got \
                     {status}"
                );
                assert!(
                    carries(&status, "FirstActivatedTime"),
                    "case {case:?}: expected the rule's activation to be reported, got {status}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

#[tokio::test]
async fn an_invocation_config_could_not_make_is_reported_as_a_failed_invocation() {
    // The failure arm of the same record: a call that does not reach the function ran
    // nothing, so what the status reports is a FAILED invocation and the error Config met —
    // while the successful invocation and evaluation it already reported stand, because they
    // happened.
    let case = "an evaluator that is no longer there to invoke";
    let emulator = Emulator::new();
    emulator.recording().await;
    emulator.given_recorded_bucket(case, "failed-push-first").await;
    let name = "vanishing-evaluator";
    let function = given_function(case, &emulator, name, &handler_source("COMPLIANT")).await;

    let rule = "failed-invocation-rule";
    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": rule,
                    "Scope": { "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] },
                    "Source": {
                        "Owner": "CUSTOM_LAMBDA",
                        "SourceIdentifier": function,
                        "SourceDetails": [{
                            "EventSource": "aws.config",
                            "MessageType": "ConfigurationItemChangeNotification",
                        }],
                    },
                }
            }),
        )
        .await;
    await_reported(case, &emulator, rule).await;
    let succeeded =
        await_status_carrying(case, &emulator, rule, "LastSuccessfulInvocationTime").await;
    let last_invocation = succeeded["LastSuccessfulInvocationTime"].clone();

    // The evaluator is taken away, and then a resource in the rule's scope is recorded: the
    // invocation that item owes is one Config cannot make.
    let (status, deleted) = emulator
        .lambda("DELETE", &format!("{FUNCTIONS_URI}/{name}"), None)
        .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected DeleteFunction to be accepted, got {status} {deleted}"
    );
    emulator
        .given_recorded_bucket(case, "failed-push-second")
        .await;

    let deadline = Instant::now() + BUDGET;
    loop {
        let status = evaluation_status(&emulator, rule).await;
        if carries(&status, "LastFailedInvocationTime") {
            assert!(
                carries(&status, "LastErrorMessage"),
                "case {case:?}: expected the error Config met to be reported, got {status}"
            );
            assert_eq!(
                status["LastSuccessfulInvocationTime"], last_invocation,
                "case {case:?}: expected the invocation that was made to stand, got {status}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the failed invocation to be reported, got {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
