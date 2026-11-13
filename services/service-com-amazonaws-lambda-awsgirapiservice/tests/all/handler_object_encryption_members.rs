// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING handler learns about how the objects it reads and writes are PROTECTED,
//! end to end over the wire: Lambda, Amazon S3, KMS, IAM and the token service on ONE
//! engine, a `python3.x` function whose code does `import boto3`, and the objects written
//! with Amazon S3's own operations.
//!
//! RULE: a call a handler makes answers the output members the service answered, so an
//! object call carries BOTH halves of the object's encryption — the algorithm in force on it
//! and the identity of the key that algorithm used, beside the bucket-scoped-key flag — and
//! the key identity is the key's own ARN as KMS reports it, whatever spelling the write
//! named the key by. A handler that learns only the algorithm cannot tell which customer
//! master key protects the bytes it was handed.
//!
//! The rows differ along what the rule generalises over: WHICH algorithm the object was
//! stored under (both KMS values, the AES256 value, and no member at all so the bucket's own
//! default decides), WHICH form the write named the key by (a key ARN, a bare key id, an
//! alias), and whether a bucket-scoped key accompanies it. What each row expects is read off
//! the answer AMAZON S3 ITSELF gives a client for the same object — the bound response
//! headers of a `HeadObject` on the wire — so what is pinned is that the handler is told
//! what the service reported, and for the KMS rows additionally that the value is the ARN
//! KMS's own `DescribeKey` names that key by.
//!
//! The write half is pinned the same way: the handler's own `PutObject` under a key it names
//! and its `CopyObject` report the key the write resolved to, and the object reads back under
//! that key.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::form_encode;
use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The Region every request below is signed for, and so the Region whose endpoint holds the
/// keys those requests name: a KMS key is a regional resource, and an object and the key it
/// is encrypted under live at the same endpoint.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The access-key id standing for the account's own published credential, which is what
/// every request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The name of the inline identity policy the execution role carries.
const INLINE_POLICY: &str = "inline";

/// The bucket every object below is written to.
const BUCKET: &str = "handler-encryption-members";

/// The bytes every object below holds.
const PAYLOAD: &str = "sepal,petal\n5,2\n";

/// The response headers Amazon S3 reports an object's encryption in, which is where this
/// suite reads what the service itself answered a client
/// (`GetObjectOutput$ServerSideEncryption` -> `x-amz-server-side-encryption`,
/// `$SSEKMSKeyId` -> `x-amz-server-side-encryption-aws-kms-key-id`, `$BucketKeyEnabled` ->
/// `x-amz-server-side-encryption-bucket-key-enabled`).
const ALGORITHM_HEADER: &str = "x-amz-server-side-encryption";
const KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";
const BUCKET_KEY_HEADER: &str = "x-amz-server-side-encryption-bucket-key-enabled";

/// The alias this suite gives the key it mints, so one row names the key by the second form
/// a key reference takes.
const KEY_ALIAS: &str = "alias/handler-ledger";

/// Lambda, Amazon S3, KMS, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own calls are dispatched back into
/// this same router, so the object it reads is the object this suite wrote and the key that
/// object names is the key KMS holds.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The handler's OWN calls are signed by the runtime, with the credential it was
        // given.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
    /// the request. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One Amazon S3 request, path style, as the account's own credential: its protocol is
    /// `restXml`, so the answer's own HEADERS are where an object's encryption is reported.
    async fn s3(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, http::HeaderMap, String) {
        let host = format!("s3.{REGION}.amazonaws.com");
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header("authorization", Self::authorization("s3"));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (
            parts.status.as_u16(),
            parts.headers,
            String::from_utf8_lossy(&body).to_string(),
        )
    }

    /// One Amazon S3 request that must be served, as the headers it answered.
    async fn s3_ok(
        &self,
        case: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> http::HeaderMap {
        let (status, answered, text) = self.s3(method, uri, headers, body).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: S3 {method} {uri} must succeed, got {status} with {text}"
        );
        answered
    }

    /// One KMS call over the shared engine: an awsJson1_1 service reached by
    /// `X-Amz-Target: TrentService.<Operation>`.
    async fn kms(&self, case: &str, operation: &str, body: &Value) -> Value {
        let host = format!("kms.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .header("authorization", Self::authorization("kms"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {case:?}: KMS {operation} must succeed, got {status} with {parsed}"
        );
        parsed
    }

    /// A customer managed key under its DEFAULT key policy — the one AWS writes, which
    /// "gives the Amazon Web Services account that owns the KMS key permission to use it"
    /// and so leaves each principal's own identity policies to decide
    /// (`CreateKeyRequest$Policy`) — as `(key id, key ARN)`.
    async fn created_key(&self, case: &str) -> (String, String) {
        let created = self
            .kms(case, "CreateKey", &json!({"Description": "the objects' key"}))
            .await;
        let member = |name: &str| {
            created["KeyMetadata"][name]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateKey answered no {name}: {created}"))
                .to_string()
        };
        (member("KeyId"), member("Arn"))
    }

    /// The ARN KMS itself names the key `reference` resolves to: what an object stored under
    /// that reference carries, whichever of the documented spellings the reference took.
    async fn described_key_arn(&self, case: &str, reference: &str) -> String {
        let described = self
            .kms(case, "DescribeKey", &json!({"KeyId": reference}))
            .await;
        described["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: DescribeKey answered no Arn for {reference:?}: {described}")
            })
            .to_string()
    }

    /// One awsQuery call to IAM or the token service that must be served.
    async fn query(&self, case: &str, signing_name: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization(signing_name))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: {signing_name} {action} must succeed, got {status} {answered}"
        );
    }

    /// Create the execution role `role_name`, trusted by Lambda, carrying `policy` as its
    /// inline identity policy — written with IAM's own operations, so the role a session is
    /// minted of is the record `iam:CreateRole` wrote.
    async fn execution_role(&self, case: &str, role_name: &str, policy: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.query(
            case,
            "iam",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.query(
            case,
            "iam",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        format!(
            "arn:aws:iam::{}:role/{role_name}",
            engine::emulator_account_id()
        )
    }

    /// One Lambda `restJson1` call, as its status, headers and parsed body.
    async fn lambda(&self, method: &str, uri: &str, body: &[u8]) -> (u16, http::HeaderMap, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("lambda"))
            .header("content-type", "application/json")
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let parsed: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        (parts.status.as_u16(), parts.headers, parsed)
    }

    /// Create a function whose code is `source`, running as `role_arn`, and wait out the
    /// provisioning window.
    async fn function(&self, case: &str, name: &str, role_arn: &str, source: &str) {
        let (status, _headers, created) = self
            .lambda(
                "POST",
                FUNCTIONS_URI,
                &serde_json::to_vec(&json!({
                    "FunctionName": name,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Timeout": 30,
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD
                            .encode(package(source)),
                    },
                }))
                .expect("the create body serializes"),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
        );
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    }

    /// `Invoke` the function synchronously with `event` and answer the payload the run
    /// produced, insisting the handler ran to completion.
    async fn invoked(&self, case: &str, name: &str, event: &Value) -> Value {
        let (status, headers, payload) = self
            .lambda(
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/invocations"),
                event.to_string().as_bytes(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        payload
    }
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
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

/// The handler this suite runs: one `GetObject` and one `HeadObject` per object the event
/// names, then one `PutObject` under a key the event names and one `CopyObject` asking for
/// KMS encryption, each reporting the encryption members the SDK handed it — so the
/// invocation's payload IS what the runtime gave the guest.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions

CLIENT = boto3.client("s3")


def _encryption(answered):
    """The encryption members of one answer, as the SDK handed them over."""
    return {
        "sse": answered.get("ServerSideEncryption"),
        "sse_key": answered.get("SSEKMSKeyId"),
        "bucket_key": answered.get("BucketKeyEnabled"),
    }


def _read(address):
    report = {}
    try:
        got = CLIENT.get_object(Bucket=address["bucket"], Key=address["key"])
    except botocore.exceptions.ClientError as error:
        return {"failed": "get_object", "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"]}
    report["get"] = _encryption(got)
    report["get"]["body"] = got["Body"].read().decode("utf-8")
    try:
        headed = CLIENT.head_object(Bucket=address["bucket"], Key=address["key"])
    except botocore.exceptions.ClientError as error:
        return {"failed": "head_object", "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"]}
    report["head"] = _encryption(headed)
    return report


def _write(write):
    report = {}
    try:
        written = CLIENT.put_object(
            Bucket=write["bucket"],
            Key=write["key"],
            Body=write["body"].encode("utf-8"),
            ServerSideEncryption=write["sse"],
            SSEKMSKeyId=write["reference"],
        )
    except botocore.exceptions.ClientError as error:
        return {"failed": "put_object", "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"]}
    report["put"] = _encryption(written)
    try:
        copied = CLIENT.copy_object(
            Bucket=write["bucket"],
            Key=write["copy"],
            CopySource={"Bucket": write["bucket"], "Key": write["source"]},
            ServerSideEncryption=write["sse"],
        )
    except botocore.exceptions.ClientError as error:
        return {"failed": "copy_object", "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"]}
    report["copy"] = _encryption(copied)
    report["read_back"] = _read({"bucket": write["bucket"], "key": write["key"]})
    return report


def lambda_handler(event, context):
    return {
        "reads": [_read(address) for address in event["reads"]],
        "write": _write(event["write"]),
    }
"#;

/// One row of the read table: the case, the encryption headers the write carries, and
/// whether the object is one stored under a KMS key whose ARN this suite also asks KMS for.
struct Row {
    case: &'static str,
    /// The `x-amz-server-side-encryption*` headers this row's `PutObject` sends.
    headers: Vec<(&'static str, String)>,
    /// The key reference the write names, for a row that names one: the ARN KMS reports for
    /// it is what the object must carry, whichever spelling the write used.
    reference: Option<String>,
    /// Whether the object is expected to report an S3 Bucket Key: `Some(true)` for the row
    /// whose write asks for one. Stated by the row itself, so the expectation the wire answer
    /// supplies is cross-checked against it rather than taken on trust — the header states
    /// that S3 USED a bucket-scoped key, so a write that asks for none reports nothing at all.
    bucket_key: Option<bool>,
}

/// The rows, built around the key `created_key` minted and the alias of it.
///
/// They differ along what the rule generalises over: WHICH algorithm (both KMS values, the
/// AES256 value, and no member at all so the bucket's own default decides), WHICH documented
/// form the key reference takes (a key ARN, a bare key id, an alias) and whether a
/// bucket-scoped key accompanies it.
fn rows(key_id: &str, key_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "SSE-KMS with the key named by its ARN",
            headers: vec![
                (ALGORITHM_HEADER, "aws:kms".to_string()),
                (KEY_ID_HEADER, key_arn.to_string()),
            ],
            reference: Some(key_arn.to_string()),
            bucket_key: None,
        },
        Row {
            case: "SSE-KMS with the same key named by its bare key id",
            headers: vec![
                (ALGORITHM_HEADER, "aws:kms".to_string()),
                (KEY_ID_HEADER, key_id.to_string()),
            ],
            reference: Some(key_id.to_string()),
            bucket_key: None,
        },
        Row {
            case: "SSE-KMS with the same key named by an alias of it",
            headers: vec![
                (ALGORITHM_HEADER, "aws:kms".to_string()),
                (KEY_ID_HEADER, KEY_ALIAS.to_string()),
            ],
            reference: Some(KEY_ALIAS.to_string()),
            bucket_key: None,
        },
        Row {
            case: "SSE-KMS with the key named by ARN and a bucket-scoped key",
            headers: vec![
                (ALGORITHM_HEADER, "aws:kms".to_string()),
                (KEY_ID_HEADER, key_arn.to_string()),
                (BUCKET_KEY_HEADER, "true".to_string()),
            ],
            reference: Some(key_arn.to_string()),
            bucket_key: Some(true),
        },
        Row {
            case: "dual-layer SSE-KMS with the key named by ARN",
            headers: vec![
                (ALGORITHM_HEADER, "aws:kms:dsse".to_string()),
                (KEY_ID_HEADER, key_arn.to_string()),
            ],
            reference: Some(key_arn.to_string()),
            bucket_key: None,
        },
        Row {
            case: "SSE-S3, the AES256 value alone, which names no key of the caller's",
            headers: vec![(ALGORITHM_HEADER, "AES256".to_string())],
            reference: None,
            bucket_key: None,
        },
        Row {
            case: "no encryption member at all, so the bucket's own default decides",
            headers: Vec::new(),
            reference: None,
            bucket_key: None,
        },
    ]
}

/// The header `answered` carries under `name`, or `None` for an answer carrying none.
fn header(answered: &http::HeaderMap, name: &str) -> Option<String> {
    answered
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// What a report the handler made says about one call's encryption members, as the three
/// values the rule is about.
fn reported(report: &Value, call: &str) -> (Option<String>, Option<String>, Option<bool>) {
    let members = &report[call];
    (
        members["sse"].as_str().map(str::to_string),
        members["sse_key"].as_str().map(str::to_string),
        members["bucket_key"].as_bool(),
    )
}

/// The key a row's object is expected to carry: for a row naming a reference, the ARN KMS
/// reports for it; for every other row, no key at all.
async fn expected_key(emulator: &Emulator, row: &Row) -> Option<String> {
    match &row.reference {
        Some(reference) => Some(emulator.described_key_arn(row.case, reference).await),
        None => None,
    }
}

/// RULE: an object call a handler makes reports the encryption members the service answered
/// — the algorithm AND the identity of the key it used, beside the bucket-scoped-key flag —
/// so a reader learns which customer master key protects the bytes it was handed from the
/// same call AWS reports it in, and a writer learns which key its write resolved to.
#[tokio::test]
async fn an_object_call_reports_the_key_the_object_is_protected_by() {
    let _stream = one_invocation_stream_at_a_time().await;
    let setup = "the encryption table";
    let function_name = "object-encryption-reader";
    let emulator = Emulator::new();

    // The role may read and write the objects and use the key they are protected under:
    // reading an SSE-KMS object decrypts it through that key on the caller's behalf, so the
    // read needs that authority beside the authority to read the object.
    let role = emulator
        .execution_role(
            setup,
            "lambda-encryption-reader",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["s3:GetObject", "s3:PutObject", "kms:Decrypt",
                               "kms:GenerateDataKey", "kms:DescribeKey"],
                    "Resource": "*",
                }],
            })
            .to_string(),
        )
        .await;
    emulator
        .function(setup, function_name, &role, HANDLER_SOURCE)
        .await;

    let (key_id, key_arn) = emulator.created_key(setup).await;
    emulator
        .kms(
            setup,
            "CreateAlias",
            &json!({"AliasName": KEY_ALIAS, "TargetKeyId": key_id}),
        )
        .await;
    emulator
        .s3_ok(setup, "PUT", &format!("/{BUCKET}"), &[], &[])
        .await;

    // Each row's object, written with Amazon S3's own operation, and what S3 ITSELF answers
    // a client about it: the bound response headers of a `HeadObject` on the wire, which are
    // the expectation every assertion below is made against.
    let rows = rows(&key_id, &key_arn);
    let mut addresses = Vec::new();
    let mut served = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let key = format!("protected/{index}.csv");
        let headers: Vec<(&str, &str)> = row
            .headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        emulator
            .s3_ok(
                row.case,
                "PUT",
                &format!("/{BUCKET}/{key}"),
                &headers,
                PAYLOAD.as_bytes(),
            )
            .await;
        let answered = emulator
            .s3_ok(row.case, "HEAD", &format!("/{BUCKET}/{key}"), &[], &[])
            .await;
        served.push((
            header(&answered, ALGORITHM_HEADER),
            header(&answered, KEY_ID_HEADER),
            header(&answered, BUCKET_KEY_HEADER).map(|value| value == "true"),
        ));
        addresses.push(json!({"bucket": BUCKET, "key": key}));
    }

    // The write half: the handler writes an object naming the key by its bare key id and
    // copies the row that names no key at all into an object asking for KMS encryption, so
    // both write paths are read through the same seam.
    let plain_source = format!("protected/{}.csv", rows.len() - 1);
    let written_key = "written/by-the-handler.csv";
    let copied_key = "written/copied.csv";
    let payload = emulator
        .invoked(
            setup,
            function_name,
            &json!({
                "reads": addresses,
                "write": {
                    "bucket": BUCKET,
                    "key": written_key,
                    "copy": copied_key,
                    "source": plain_source,
                    "body": PAYLOAD,
                    "sse": "aws:kms",
                    "reference": key_id,
                },
            }),
        )
        .await;

    let reads = payload["reads"]
        .as_array()
        .unwrap_or_else(|| panic!("the handler reported no reads in {payload}"))
        .clone();
    assert_eq!(
        reads.len(),
        rows.len(),
        "expected one report per object, got {reads:?}"
    );

    for ((row, expected), report) in rows.iter().zip(served.iter()).zip(reads.iter()) {
        let case = row.case;
        assert!(
            report["failed"].is_null(),
            "case {case:?}: both reads must be served, got {report}"
        );
        let (expected_algorithm, expected_key_id, expected_bucket_key) = expected;
        // The key the object names is the key KMS holds, by its own ARN — which is what the
        // service answered the client above, so the two agree for the right reason.
        let from_kms = expected_key(&emulator, row).await;
        assert_eq!(
            expected_key_id, &from_kms,
            "case {case:?}: the service must report the key ARN KMS names, got \
             {expected_key_id:?} against {from_kms:?}"
        );
        if let Some(arn) = &from_kms {
            assert!(
                arn.starts_with("arn:aws:kms:") && arn.contains(":key/"),
                "case {case:?}: KMS must name the key by a key ARN, got {arn:?}"
            );
        }
        // And the bucket-scoped-key flag the service answered is the one the row asks for,
        // so the row whose write switches a bucket key on is compared against a stated
        // `Some(true)` rather than against whatever the wire answer happened to carry.
        assert_eq!(
            expected_bucket_key, &row.bucket_key,
            "case {case:?}: the service must report the bucket-scoped key {:?}, got \
             {expected_bucket_key:?}",
            row.bucket_key
        );
        for call in ["get", "head"] {
            let (algorithm, key_id, bucket_key) = reported(report, call);
            assert_eq!(
                &algorithm, expected_algorithm,
                "case {case:?}: the handler's {call} must report the algorithm \
                 {expected_algorithm:?}, got {algorithm:?} in {report}"
            );
            assert_eq!(
                &key_id, expected_key_id,
                "case {case:?}: the handler's {call} must report the key \
                 {expected_key_id:?}, got {key_id:?} in {report}"
            );
            assert_eq!(
                &bucket_key, expected_bucket_key,
                "case {case:?}: the handler's {call} must report the bucket-scoped key \
                 {expected_bucket_key:?}, got {bucket_key:?} in {report}"
            );
        }
        assert_eq!(
            report["get"]["body"].as_str(),
            Some(PAYLOAD),
            "case {case:?}: the handler must read back the stored bytes, got {report}"
        );
    }

    // The write half, judged the same way: against what Amazon S3 answers a client about the
    // objects the handler wrote.
    let write = &payload["write"];
    assert!(
        write["failed"].is_null(),
        "case {setup:?}: the handler's write must be served, got {write}"
    );
    for (call, key) in [("put", written_key), ("copy", copied_key)] {
        let answered = emulator
            .s3_ok(setup, "HEAD", &format!("/{BUCKET}/{key}"), &[], &[])
            .await;
        let expected = (
            header(&answered, ALGORITHM_HEADER),
            header(&answered, KEY_ID_HEADER),
        );
        let (algorithm, reported_key, _) = reported(write, call);
        assert_eq!(
            algorithm, expected.0,
            "case {setup:?}: the handler's {call} must report the algorithm {:?}, got \
             {algorithm:?} in {write}",
            expected.0
        );
        assert_eq!(
            reported_key, expected.1,
            "case {setup:?}: the handler's {call} must report the key {:?}, got \
             {reported_key:?} in {write}",
            expected.1
        );
        assert!(
            reported_key
                .as_deref()
                .is_some_and(|arn| arn.starts_with("arn:aws:kms:") && arn.contains(":key/")),
            "case {setup:?}: the handler's {call} must name the key by its ARN, got \
             {reported_key:?} in {write}"
        );
    }
    // And the object the handler wrote reads back through the seam under that same key.
    let read_back = &write["read_back"];
    let (algorithm, key, _) = reported(read_back, "get");
    let (head_algorithm, head_key, _) = reported(read_back, "head");
    let (_, written_key_id, _) = reported(write, "put");
    assert_eq!(
        (algorithm.clone(), key.clone()),
        (head_algorithm, head_key),
        "case {setup:?}: a read and a head of one object must agree, got {read_back}"
    );
    assert_eq!(
        (algorithm, key),
        (Some("aws:kms".to_string()), written_key_id.clone()),
        "case {setup:?}: the object the handler wrote must read back under the key the \
         write reported, got {read_back}"
    );
}
