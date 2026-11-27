// SPDX-License-Identifier: CC0-1.0

//! What a handler ADMINISTERING a bucket's own subresources can do with the AWS SDK the
//! runtime supplies: Lambda, IAM, the token service and Amazon S3 on ONE engine, a Python
//! function that reads, replaces and REMOVES the bucket's resource policy, its tag set and
//! its default encryption, and every answer checked against what Amazon S3 itself holds.
//!
//! RULE: the AWS SDK a managed runtime supplies is generated from the called service's whole
//! model, so a handler's call on ANY operation that service models and this emulator serves
//! is built, signed and dispatched to it, and what comes back is that operation's own output
//! members — there is no operation of a reachable service the SDK answers out of itself.
//!
//! The rows differ along WHICH subresource of the bucket the trio of operations acts on — the
//! resource policy, the tag set and the default encryption — and along what the service
//! answers for a bucket carrying none of it: a refusal of its own for the two a bucket may
//! carry nothing in, and the default every general purpose bucket carries for encryption.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::given_role;
use crate::event_source_mappings::{await_function_active, call, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region the function runs in and the bucket is created in.
const REGION: &str = "us-east-1";

/// The bucket the handler administers.
const BUCKET: &str = "handler-subresource-bucket";

/// What a read of one subresource answers when the bucket carries none of it.
enum Absent {
    /// Amazon S3's own refusal, by the code and the status it answers.
    Refused {
        /// The error code S3 names.
        code: &'static str,
        /// The status it answers with.
        status: u64,
    },
    /// A served answer: the state the service applies to a bucket that configured none.
    Served(Value),
}

/// One subresource of the bucket: what the row is called, the name the handler's own dispatch
/// knows the trio of operations by, the value the replacement writes, what the read reports
/// once it is written, and what a read answers while the bucket carries none of it.
struct Subresource {
    /// What the row is called in a failure.
    case: &'static str,
    /// The subresource, as the handler names its read, its replacement and its removal.
    kind: &'static str,
    /// What the replacement writes, handed to the handler so the assertion compares what the
    /// test asked for with what the service answered.
    value: Value,
    /// What the read must report after the replacement.
    after: Value,
    /// What a read answers before the replacement, and again after the removal.
    absent: Absent,
}

/// The subresources this suite drives, one row per bucket subresource the service models a
/// read, a replacement and a removal of.
///
/// Each trio is the operations the model declares for it
/// (`smithy_query(query="operation",operation="GetBucketPolicy,PutBucketPolicy,
/// DeleteBucketPolicy,GetBucketTagging,PutBucketTagging,DeleteBucketTagging,
/// GetBucketEncryption,PutBucketEncryption,DeleteBucketEncryption",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`).
fn subresources() -> Vec<Subresource> {
    vec![
        Subresource {
            case: "the bucket's own resource policy",
            kind: "policy",
            value: Value::String(policy_document().to_string()),
            after: json!({ "document": policy_document() }),
            // "There is no such thing as a NoSuchBucketPolicy error" is not the case: S3
            // answers that code for a bucket carrying no policy (`GetBucketPolicy`).
            absent: Absent::Refused {
                code: "NoSuchBucketPolicy",
                status: 404,
            },
        },
        Subresource {
            case: "the bucket's own tag set",
            kind: "tagging",
            value: json!([
                { "Key": "owner", "Value": "the-handler" },
                { "Key": "stage", "Value": "behavioural" },
            ]),
            after: json!({ "tags": [["owner", "the-handler"], ["stage", "behavioural"]] }),
            // "GetBucketTagging has the following special error: Error code: NoSuchTagSet"
            absent: Absent::Refused {
                code: "NoSuchTagSet",
                status: 404,
            },
        },
        Subresource {
            case: "the bucket's default server-side encryption",
            kind: "encryption",
            value: json!([{
                "ApplyServerSideEncryptionByDefault": { "SSEAlgorithm": "AES256" },
                "BucketKeyEnabled": true,
            }]),
            after: json!({ "rules": [["AES256", true]] }),
            // "By default, all buckets have a default encryption configuration that uses
            // server-side encryption with Amazon S3 managed keys (SSE-S3)"
            // (`GetBucketEncryption`), so a bucket that configured none is SERVED that
            // default rather than refused — and the removal puts it back.
            absent: Absent::Served(json!({ "rules": [["AES256", false]] })),
        },
    ]
}

/// The resource policy the handler writes: a well-formed bucket policy that takes nothing
/// away from the caller, so the calls after it are judged exactly as the ones before it were.
///
/// The principal is the ACCOUNT ROOT rather than anyone at all: a policy naming every
/// principal is a public one, which S3 refuses while a bucket's Block Public Access settings
/// prevent public policies — the state a bucket is created in.
fn policy_document() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "ReadableByTheAccount",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": "s3:GetObject",
            "Resource": format!("arn:aws:s3:::{BUCKET}/*"),
        }],
    })
}

/// Lambda, IAM, the token service and Amazon S3 over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own calls are dispatched back into this
/// same router, so the bucket it administers is the bucket this suite created.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One request to Amazon S3, made as a CLIENT makes one: its protocol is `restXml`, so the
/// request is a method and a path and the answer is a document.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Create the bucket the handler administers.
async fn given_bucket(router: &MultiServiceRouter, case: &str) {
    let (status, document) = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBucket to answer 200, got {status}: {document}"
    );
}

/// The handler this suite runs: for each subresource the event names, the read before, the
/// replacement, the read after, the removal and the read after that — each reported under its
/// own name, as values the invocation's own answer carries.
const HANDLER_SOURCE: &str = r#"
import json

import boto3
import botocore.exceptions


def _refusal(error):
    """The service's own code, sentence and status, as the SDK raised them."""
    return {
        "ok": False,
        "code": error.response["Error"]["Code"],
        "message": error.response["Error"]["Message"],
        "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
    }


def _operations(client, bucket, kind, value):
    """The read, the replacement and the removal of ONE subresource of the bucket."""
    if kind == "policy":
        return (
            lambda: client.get_bucket_policy(Bucket=bucket),
            lambda: client.put_bucket_policy(Bucket=bucket, Policy=value),
            lambda: client.delete_bucket_policy(Bucket=bucket),
        )
    if kind == "tagging":
        return (
            lambda: client.get_bucket_tagging(Bucket=bucket),
            lambda: client.put_bucket_tagging(Bucket=bucket, Tagging={"TagSet": value}),
            lambda: client.delete_bucket_tagging(Bucket=bucket),
        )
    if kind == "encryption":
        return (
            lambda: client.get_bucket_encryption(Bucket=bucket),
            lambda: client.put_bucket_encryption(
                Bucket=bucket, ServerSideEncryptionConfiguration={"Rules": value}
            ),
            lambda: client.delete_bucket_encryption(Bucket=bucket),
        )
    raise ValueError("this handler drives no subresource named %r" % (kind,))


def _reported(kind, read):
    """What one read says about the subresource, as primitive values."""
    if kind == "policy":
        return {"document": json.loads(read["Policy"])}
    if kind == "tagging":
        return {
            "tags": sorted(
                [[one["Key"], one["Value"]] for one in read.get("TagSet", [])]
            )
        }
    rules = []
    for rule in (read.get("ServerSideEncryptionConfiguration") or {}).get("Rules", []):
        applied = rule.get("ApplyServerSideEncryptionByDefault") or {}
        rules.append([applied.get("SSEAlgorithm"), bool(rule.get("BucketKeyEnabled"))])
    return {"rules": rules}


def _attempted(kind, call, reported):
    """One call, as what it answered or as the refusal it was answered with."""
    try:
        answered = call()
    except botocore.exceptions.ClientError as error:
        return _refusal(error)
    except botocore.exceptions.BotoCoreError as error:
        # A call the SDK itself declined to make, which is no answer from the service at
        # all: reported as one so the assertion names it.
        return {"ok": False, "code": type(error).__name__, "message": str(error)}
    if not reported:
        return {"ok": True, "status": answered["ResponseMetadata"]["HTTPStatusCode"]}
    return {"ok": True, "read": _reported(kind, answered)}


def lambda_handler(event, context):
    client = boto3.client("s3")
    bucket = event["bucket"]
    report = {}
    for row in event["rows"]:
        kind = row["kind"]
        read, replace, remove = _operations(client, bucket, kind, row["value"])
        report[kind] = {
            "before": _attempted(kind, read, True),
            "replaced": _attempted(kind, replace, False),
            "after": _attempted(kind, read, True),
            "removed": _attempted(kind, remove, False),
            "after_removal": _attempted(kind, read, True),
        }
    return report
"#;

/// Create the function `name`, running as `role_arn`, out of [`HANDLER_SOURCE`].
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(HANDLER_SOURCE)),
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
    await_function_active(router, case, name).await;
}

/// Invoke `name`, handing it the table of subresources to drive.
async fn invoked(router: &MultiServiceRouter, case: &str, name: &str, rows: &[Subresource]) -> Value {
    let payload = json!({
        "bucket": BUCKET,
        "rows": rows
            .iter()
            .map(|row| json!({ "kind": row.kind, "value": row.value }))
            .collect::<Vec<Value>>(),
    });
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &payload,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.header("X-Amz-Function-Error"),
        None,
        "case {case:?}: expected the handler to run without a function error, got {}",
        answered.text()
    );
    answered.json()
}

/// Assert one read answered what the row says a bucket carrying none of the subresource is
/// answered.
fn assert_absent(case: &str, label: &str, absent: &Absent, reported: &Value) {
    match absent {
        Absent::Refused { code, status } => {
            assert_eq!(
                reported["ok"],
                json!(false),
                "case {case:?} ({label}): expected Amazon S3's own refusal, got {reported}"
            );
            assert_eq!(
                reported["code"],
                json!(code),
                "case {case:?} ({label}): expected the code {code}, got {reported}"
            );
            assert_eq!(
                reported["status"],
                json!(status),
                "case {case:?} ({label}): expected the status {status}, got {reported}"
            );
        }
        Absent::Served(expected) => {
            assert_eq!(
                reported["ok"],
                json!(true),
                "case {case:?} ({label}): expected the service's own default to be served, \
                 got {reported}"
            );
            assert_eq!(
                &reported["read"], expected,
                "case {case:?} ({label}): expected {expected}, got {reported}"
            );
        }
    }
}

/// A handler that reads, replaces and removes each of the bucket's subresources through the
/// SDK the runtime supplies: every call reaches Amazon S3, and the store holds what the
/// handler wrote and loses what it removed.
#[tokio::test]
async fn a_handler_drives_each_bucket_subresource_through_the_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler administering a bucket's subresources";
    let router = emulator();
    given_bucket(&router, case).await;
    let role = given_role(
        &router,
        case,
        "handler-subresource-role",
        Some(json!([{ "Effect": "Allow", "Action": "s3:*", "Resource": "*" }])),
    )
    .await;
    let function = "handler-subresource-driver";
    given_function(&router, case, function, &role).await;
    let rows = subresources();
    let report = invoked(&router, case, function, &rows).await;

    for row in &rows {
        let held = &report[row.kind];
        assert_absent(row.case, "before the replacement", &row.absent, &held["before"]);
        for step in ["replaced", "removed"] {
            assert_eq!(
                held[step]["ok"],
                json!(true),
                "case {:?}: expected the {step} call to be served by Amazon S3, got {held}",
                row.case
            );
        }
        assert_eq!(
            held["after"]["ok"],
            json!(true),
            "case {:?}: expected the read after the replacement to be served, got {held}",
            row.case
        );
        assert_eq!(
            held["after"]["read"], row.after,
            "case {:?}: expected the read to report {}, got {held}",
            row.case, row.after
        );
        assert_absent(
            row.case,
            "after the removal",
            &row.absent,
            &held["after_removal"],
        );
    }

    // What Amazon S3 itself holds once the handler has run: the two subresources it removed
    // are gone from the store, which is what the reads above are a report of.
    for (path, code) in [
        (format!("/{BUCKET}?policy"), "NoSuchBucketPolicy"),
        (format!("/{BUCKET}?tagging"), "NoSuchTagSet"),
    ] {
        let (status, document) = s3(&router, "GET", &path, b"").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the store to carry nothing at {path}, got {status}: \
             {document}"
        );
        assert!(
            document.contains(code),
            "case {case:?}: expected the store's refusal to name {code}, got {document}"
        );
    }
}
