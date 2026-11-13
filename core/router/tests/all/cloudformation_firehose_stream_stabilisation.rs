// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::KinesisFirehose::DeliveryStream` is not
//! reported complete before Amazon Data Firehose reports that stream ready — so the first
//! data-plane call a client makes after the stack completes is accepted.
//!
//! The rule under test: **CloudFormation does not report a resource (and so the stack
//! operation holding it) created while the service that owns it still reports the resource
//! short of the state it is usable in; it holds the resource `CREATE_IN_PROGRESS`, under the
//! identifier that service already assigned, until that service reports it ready.** For a
//! Firehose stream the state it is usable in is `ACTIVE`: `CreateDeliveryStream` "is an
//! asynchronous operation that immediately returns. The initial status of the Firehose stream
//! is CREATING. After the Firehose stream is created, its status is ACTIVE and it now accepts
//! data … Attempts to send data to a delivery stream that is not in the ACTIVE state cause an
//! exception." Two crates have to agree for that to hold (CloudFormation makes the calls;
//! Firehose decides the stream's status), so the suite lives here rather than in either one.
//!
//! What this pins is the pair of observations an infrastructure-as-code client makes: the
//! instant the stack first reports itself complete, Firehose reports every stream the template
//! declared `ACTIVE` and takes a record for it. A stack reported complete while its stream is
//! still `CREATING` fails these cases, because the put made straight after it is refused.
//!
//! The rows differ along what the rule generalises over: how many streams one template
//! declares (so the stack waits for each of them, not just the first), whether the template
//! NAMES a stream or leaves the name to CloudFormation, which destination configuration it
//! declares, whether server-side encryption is asked for at create, and whether the template
//! is written in JSON or YAML. Every expected value is computed here from what the row
//! declares, never read back out of the answer being checked.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element reader this test crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
// The SigV4 header builder is the crate's one too, and it signs for the same Region this
// suite's hosts name.
use super::query_wire::{form_encode, opt_element};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// How often the suite asks the stack whether it has completed, and how long it goes on
/// asking. The interval is short because the first completed observation is what the rule is
/// stated about: the poll that sees the stack complete is the one whose stream status and put
/// are then checked.
///
/// The bound on the asking is a WALL-CLOCK deadline rather than a count of reads. A count
/// bounds the wait by how fast the host serves ONE read, which is not what is being waited
/// for: the spans the services behind a stack choose take the time they take, so a fixed
/// number of cheap reads can run out before the stack can complete on a loaded machine while
/// allowing no more on an idle one. The deadline is long enough that a loaded machine is
/// slower than those spans without the rule being any different, and all it is for is that
/// the wait TERMINATES.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// The `StackStatus` / `ResourceStatus` values the rule is stated in.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `DeliveryStreamStatus` a stream accepts data in.
const ACTIVE: &str = "ACTIVE";

/// The record every case writes, as the base64 the awsJson wire carries a blob in.
const RECORD_DATA: &str = "bWFya2VyCg==";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
///
/// Request authentication is off, as in every other suite here; the in-process callback
/// CloudFormation makes signs its request either way (the credential scope is what routes
/// it). Amazon S3 and IAM are here because every template below declares the bucket a stream
/// delivers to and the role it delivers under — resource types CloudFormation provisions too,
/// and which Firehose resolves before it writes a stream.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Firehose operation over its awsJson1_1 wire.
async fn firehose(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("firehose.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Firehose_20150804.{operation}"))
        .header("authorization", authorization("firehose"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The `(PhysicalResourceId, ResourceStatus)` the stack reports for ONE of its resources,
/// read through `DescribeStackResource` so the answer carries that resource alone.
async fn stack_resource(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> (Option<String>, Option<String>, String) {
    let (status, body) = cloudformation(
        router,
        "DescribeStackResource",
        &[("StackName", stack), ("LogicalResourceId", logical_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResource {stack:?}/{logical_id:?}: expected HTTP 200, got {status} with \
         body {body}"
    );
    (
        opt_element(&body, "PhysicalResourceId"),
        opt_element(&body, "ResourceStatus"),
        body,
    )
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
pub(crate) async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it first reports `wanted`, and answer the last status it reported with
/// the `DescribeStacks` answer that status was read from. The FIRST such observation is what
/// the rule above is stated about, so nothing is waited out past it.
///
/// `pub(crate)` because every suite deploying a stack whose resources their owning services
/// report as still coming up needs exactly this: the operation's completion is owed from the
/// instant those services report the resources ready, which no fixed sleep can name. The wait
/// is on that CONDITION, looking again until it holds or [`SETTLE_DEADLINE`] passes.
pub(crate) async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let giving_up_at = Instant::now() + SETTLE_DEADLINE;
    let mut last = stack_status(router, stack).await;
    while last.0 != wanted && Instant::now() < giving_up_at {
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// A trust policy for a delivery role: Firehose has to be able to assume the role every
/// stream is configured with.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "firehose.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// One stream a row's template declares: the logical id it is declared under, and the name
/// SUFFIX the template gives it — `None` where the template declares no name at all, so the
/// name is CloudFormation's to generate.
struct DeclaredStream {
    logical_id: &'static str,
    name_suffix: Option<&'static str>,
}

/// One row of the table: a template, and the streams it declares.
struct Row {
    case: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), built from the stack
    /// name the row runs under.
    template: fn(&str) -> String,
    streams: &'static [DeclaredStream],
}

const ROWS: &[Row] = &[
    Row {
        case: "one named stream with an S3 destination and buffering hints, JSON",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a named Firehose stream delivering to a declared bucket",
                "Resources": {
                    "DeliveryRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": format!("{stack}-role"),
                            "AssumeRolePolicyDocument": trust_policy()
                        }
                    },
                    "DestinationBucket": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": format!("{stack}-data") }
                    },
                    "Stream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "DeliveryStreamName": format!("{stack}-stream"),
                            "DeliveryStreamType": "DirectPut",
                            "S3DestinationConfiguration": {
                                "BucketARN": { "Fn::GetAtt": ["DestinationBucket", "Arn"] },
                                "RoleARN": { "Fn::GetAtt": ["DeliveryRole", "Arn"] },
                                "Prefix": "raw/events/",
                                "BufferingHints": { "IntervalInSeconds": 60, "SizeInMBs": 1 }
                            }
                        }
                    }
                }
            })
            .to_string()
        },
        streams: &[DeclaredStream {
            logical_id: "Stream",
            name_suffix: Some("-stream"),
        }],
    },
    Row {
        case: "a stream whose name CloudFormation generates, ExtendedS3 destination, JSON",
        template: |stack| {
            json!({
                "Resources": {
                    "DeliveryRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": format!("{stack}-role"),
                            "AssumeRolePolicyDocument": trust_policy()
                        }
                    },
                    "DestinationBucket": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": format!("{stack}-data") }
                    },
                    "Stream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "ExtendedS3DestinationConfiguration": {
                                "BucketARN": { "Fn::GetAtt": ["DestinationBucket", "Arn"] },
                                "RoleARN": { "Fn::GetAtt": ["DeliveryRole", "Arn"] },
                                "CompressionFormat": "GZIP"
                            }
                        }
                    }
                }
            })
            .to_string()
        },
        streams: &[DeclaredStream {
            logical_id: "Stream",
            name_suffix: None,
        }],
    },
    Row {
        case: "two streams in one template, so the stack waits for both, JSON",
        template: |stack| {
            json!({
                "Resources": {
                    "DeliveryRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": format!("{stack}-role"),
                            "AssumeRolePolicyDocument": trust_policy()
                        }
                    },
                    "DestinationBucket": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": format!("{stack}-data") }
                    },
                    "FirstStream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "DeliveryStreamName": format!("{stack}-first"),
                            "S3DestinationConfiguration": {
                                "BucketARN": { "Fn::GetAtt": ["DestinationBucket", "Arn"] },
                                "RoleARN": { "Fn::GetAtt": ["DeliveryRole", "Arn"] },
                                "Prefix": "first/"
                            }
                        }
                    },
                    "SecondStream": {
                        "Type": "AWS::KinesisFirehose::DeliveryStream",
                        "Properties": {
                            "DeliveryStreamName": format!("{stack}-second"),
                            "S3DestinationConfiguration": {
                                "BucketARN": { "Fn::GetAtt": ["DestinationBucket", "Arn"] },
                                "RoleARN": { "Fn::GetAtt": ["DeliveryRole", "Arn"] },
                                "Prefix": "second/"
                            }
                        }
                    }
                }
            })
            .to_string()
        },
        streams: &[
            DeclaredStream {
                logical_id: "FirstStream",
                name_suffix: Some("-first"),
            },
            DeclaredStream {
                logical_id: "SecondStream",
                name_suffix: Some("-second"),
            },
        ],
    },
    Row {
        case: "a named stream with server-side encryption asked for at create, YAML",
        template: |stack| {
            [
                "Resources:".to_string(),
                "  DeliveryRole:".to_string(),
                "    Type: AWS::IAM::Role".to_string(),
                "    Properties:".to_string(),
                format!("      RoleName: {stack}-role"),
                "      AssumeRolePolicyDocument:".to_string(),
                "        Version: \"2012-10-17\"".to_string(),
                "        Statement:".to_string(),
                "          - Effect: Allow".to_string(),
                "            Principal:".to_string(),
                "              Service: firehose.amazonaws.com".to_string(),
                "            Action: sts:AssumeRole".to_string(),
                "  DestinationBucket:".to_string(),
                "    Type: AWS::S3::Bucket".to_string(),
                "    Properties:".to_string(),
                format!("      BucketName: {stack}-data"),
                "  Stream:".to_string(),
                "    Type: AWS::KinesisFirehose::DeliveryStream".to_string(),
                "    Properties:".to_string(),
                format!("      DeliveryStreamName: {stack}-encrypted"),
                "      DeliveryStreamType: DirectPut".to_string(),
                "      DeliveryStreamEncryptionConfigurationInput:".to_string(),
                "        KeyType: AWS_OWNED_CMK".to_string(),
                "      S3DestinationConfiguration:".to_string(),
                "        BucketARN: !GetAtt DestinationBucket.Arn".to_string(),
                "        RoleARN: !GetAtt DeliveryRole.Arn".to_string(),
                "        Prefix: encrypted/".to_string(),
                String::new(),
            ]
            .join("\n")
        },
        streams: &[DeclaredStream {
            logical_id: "Stream",
            name_suffix: Some("-encrypted"),
        }],
    },
];

/// Every row: while Firehose has not reported the declared stream `ACTIVE` the stack and the
/// stream's resource are still `CREATE_IN_PROGRESS` under the name the stream was created
/// with — and the instant the stack first reports itself complete, every stream it declared
/// is `ACTIVE` in Firehose and takes a record.
#[tokio::test]
async fn a_stack_completes_only_once_firehose_reports_its_streams_active() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("firehose-stabilising-{index}");
        let template = (row.template)(&stack_name);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                // Every row's template declares the IAM role its streams deliver under.
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The FIRST observation of each stream resource: the identifier Firehose was given is
        // already reported, and the resource is still `CREATE_IN_PROGRESS` — the create was
        // accepted, not finished.
        let mut stream_names: Vec<(&str, String)> = Vec::new();
        for declared in row.streams {
            let (physical_id, resource_status, body) =
                stack_resource(&router, &stack_name, declared.logical_id).await;
            let name = physical_id.unwrap_or_default();
            match declared.name_suffix {
                Some(suffix) => assert_eq!(
                    name,
                    format!("{stack_name}{suffix}"),
                    "case {case:?}: expected the resource to report the name the template \
                     declared, got {name:?} in {body}"
                ),
                // A name CloudFormation generated carries the stack's name, so what is
                // checked is the SHAPE of a generated name rather than a value this suite
                // could compute.
                None => assert!(
                    name.to_lowercase()
                        .starts_with(&format!("{stack_name}-").to_lowercase())
                        && name.len() > stack_name.len() + 1,
                    "case {case:?}: expected a generated name carrying the stack name, got \
                     {name:?} in {body}"
                ),
            }
            assert_eq!(
                resource_status.as_deref(),
                Some(CREATE_IN_PROGRESS),
                "case {case:?}: expected the stream resource to still be \
                 {CREATE_IN_PROGRESS} while Firehose has not reported it {ACTIVE}, got {body}"
            );
            stream_names.push((declared.logical_id, name));
        }

        // The stack holds the operation open for as long as any of its streams is still
        // coming up, and reports itself complete only once they are all ready.
        let (status, described) = wait_for_stack_status(&router, &stack_name, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        // From that first completed observation on, every stream the template declared is a
        // stream a client can use: Firehose reports it ACTIVE and takes the record written
        // straight after the stack completed.
        for (logical_id, stream_name) in &stream_names {
            let (status, described) = firehose(
                &router,
                "DescribeDeliveryStream",
                &json!({ "DeliveryStreamName": stream_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeDeliveryStream for \
                 {stream_name:?}, got {status} with body {described}"
            );
            assert_eq!(
                described["DeliveryStreamDescription"]["DeliveryStreamStatus"].as_str(),
                Some(ACTIVE),
                "case {case:?}: expected Firehose to report {stream_name:?} {ACTIVE} once the \
                 stack reported itself {CREATE_COMPLETE}, got {described}"
            );

            let (status, put) = firehose(
                &router,
                "PutRecord",
                &json!({
                    "DeliveryStreamName": stream_name,
                    "Record": { "Data": RECORD_DATA }
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutRecord against {stream_name:?} to be accepted once \
                 the stack reported itself {CREATE_COMPLETE}, got {status} with body {put}"
            );
            assert!(
                put["RecordId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected PutRecord against {stream_name:?} to answer a \
                 RecordId, got {put}"
            );

            // And the stack reports that same resource created, under the same name.
            let (physical_id, resource_status, body) =
                stack_resource(&router, &stack_name, logical_id).await;
            assert_eq!(
                resource_status.as_deref(),
                Some(CREATE_COMPLETE),
                "case {case:?}: expected the {logical_id} resource to be {CREATE_COMPLETE} \
                 once the stack completed, got {body}"
            );
            assert_eq!(
                physical_id.as_deref(),
                Some(stream_name.as_str()),
                "case {case:?}: expected the {logical_id} resource to go on reporting \
                 {stream_name:?}, got {body}"
            );
        }
    }
}
