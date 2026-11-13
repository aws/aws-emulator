// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING handler's own Amazon S3 call may do, end to end over the wire: Lambda,
//! Amazon S3, IAM and the token service on ONE engine, a `python3.x` function whose code
//! does `import boto3` and reads an object, and the object, the bucket policy and the role
//! policies all written with those services' own operations.
//!
//! The rule pinned here: a handler's SDK call is made with temporary credentials for the
//! FUNCTION'S EXECUTION ROLE — "the execution role grants the function permission to use
//! Amazon Web Services services" — so the service serving that call judges it against the
//! policies in force on that role and against its own resource policy, exactly as it judges
//! any other signed request. Two functions whose roles carry different grants are therefore
//! refused different things, and the principal a refusal names is the role session the call
//! was made as.
//!
//! The rows differ along what the rule generalises over: WHERE the grant comes from (the
//! role's own identity policy, the bucket's policy naming the role, both, neither), whether
//! a policy carries an explicit `Deny` and which side it is on, and how widely the grant is
//! scoped (the key the handler reads, another key, the whole bucket). Each row asserts both
//! halves: what the handler read back — which is what the invocation answers — and, for a
//! read it was served, that the bytes are the ones the bucket holds.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The module and attribute every package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The access-key id standing for the account's own published credential: neither the token
/// service nor IAM holds a record of it, so it resolves to the account's ambient identity,
/// which is what every request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The names of the buckets this suite reads: one per row of the read table, so a row's
/// own bucket policy is the only resource policy in force on its read.
const BUCKET_PREFIX: &str = "handler-read-authz";

/// The bucket the write table and the unbound-call test use.
const BUCKET: &str = "handler-write-authz";

/// The key the handler reads, and the bytes that key holds.
const KEY: &str = "reports/held.csv";
const BODY: &str = "id,total\n1,42\n";

/// Another key, which a narrowly scoped grant names instead of the one a read addresses.
const OTHER_KEY: &str = "reports/other.csv";

/// Another role of the same account, which a bucket policy names where the row is about a
/// grant made to someone else.
const OTHER_ROLE: &str = "another-reader";

/// The name of the inline identity policy a row writes on the execution role.
const INLINE_POLICY: &str = "inline";

/// The code S3 answers an authorization refusal with, and the status it carries (Amazon S3
/// "Error responses": `AccessDenied`, HTTP 403).
pub(crate) const ACCESS_DENIED: &str = "AccessDenied";
pub(crate) const ACCESS_DENIED_STATUS: i64 = 403;

/// Lambda, Amazon S3, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the bucket it reads is the bucket this suite wrote.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The handler's OWN call is signed by the runtime, with the credential it was given.
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

    /// One Amazon S3 request, path style, as the account's own credential.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, String) {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header("authorization", Self::authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (
            parts.status.as_u16(),
            String::from_utf8_lossy(&body).to_string(),
        )
    }

    /// One Amazon S3 request that must be served.
    pub(crate) async fn s3_ok(&self, case: &str, method: &str, uri: &str, body: &[u8]) -> String {
        let (status, answered) = self.s3(method, uri, body).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: S3 {method} {uri} must succeed, got {status} with {answered}"
        );
        answered
    }

    /// One awsQuery call to IAM or the token service, as the account's own credential,
    /// whatever it answers.
    async fn query_any(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
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
        (
            status,
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One awsQuery call that must be served.
    async fn query(
        &self,
        case: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let (status, answered) = self.query_any(signing_name, version, action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {signing_name} {action} must succeed, got {status} {answered}"
        );
        answered
    }

    /// The identity policy in force on `role_name` from now on: `document`, or none at all
    /// — which is what makes a row about the grant that is in force when the handler calls,
    /// rather than about the one in force when its credentials were minted.
    pub(crate) async fn role_policy(&self, case: &str, role_name: &str, document: Option<&str>) {
        match document {
            Some(document) => {
                self.query(
                    case,
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", INLINE_POLICY.to_string()),
                        ("PolicyDocument", document.to_string()),
                    ],
                )
                .await;
            }
            // A role carrying no such policy is already in the state the row asks for, and
            // IAM says so with its own `NoSuchEntity`.
            None => {
                self.query_any(
                    "iam",
                    "2010-05-08",
                    "DeleteRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", INLINE_POLICY.to_string()),
                    ],
                )
                .await;
            }
        }
    }

    /// One Lambda `restJson1` call, as its status and parsed body.
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

    /// Create the execution role `role_name`, trusted by Lambda, carrying `policy` as its
    /// inline identity policy when one is given — written with IAM's own operations, so the
    /// role a session is minted of is the record `iam:CreateRole` wrote.
    pub(crate) async fn execution_role(
        &self,
        case: &str,
        role_name: &str,
        policy: Option<&str>,
    ) -> String {
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
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.query(
                case,
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", INLINE_POLICY.to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        role_arn(role_name)
    }

    /// Create a function whose code is `source`, running as `role_arn`, and wait out the
    /// provisioning window.
    pub(crate) async fn function(&self, case: &str, name: &str, role_arn: &str, source: &str) {
        let (status, _headers, created) = self
            .lambda(
                "POST",
                FUNCTIONS_URI,
                &serde_json::to_vec(&json!({
                    "FunctionName": name,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Timeout": 10,
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

    /// `Invoke` the function synchronously with `event`, answering the payload the run
    /// produced and the `FunctionError` signal it carried, if any.
    pub(crate) async fn invoke(
        &self,
        case: &str,
        name: &str,
        event: &Value,
    ) -> (Option<String>, Value) {
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
        (signal, payload)
    }

    /// One bucket holding the object a read addresses, carrying `policy` as its bucket
    /// policy when the row names one.
    async fn bucket(&self, case: &str, name: &str, policy: Option<&str>) {
        self.s3_ok(case, "PUT", &format!("/{name}"), &[]).await;
        self.s3_ok(case, "PUT", &format!("/{name}/{KEY}"), BODY.as_bytes())
            .await;
        if let Some(policy) = policy {
            self.s3_ok(case, "PUT", &format!("/{name}?policy"), policy.as_bytes())
                .await;
        }
    }
}

/// The ARN of the role IAM holds under `role_name`.
pub(crate) fn role_arn(role_name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{role_name}",
        engine::emulator_account_id()
    )
}

/// The assumed-role session ARN a call made by an invocation of `function_name` running as
/// `role_name` speaks for: the session a function's calls are made under is named for the
/// FUNCTION.
pub(crate) fn session_arn(role_name: &str, function_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{function_name}",
        engine::emulator_account_id()
    )
}

/// One policy document, in the flavour its `Principal` element decides: a bucket policy
/// names one, an identity policy names none.
pub(crate) fn document(
    effect: &str,
    action: &str,
    resource: &str,
    principal: Option<&str>,
) -> String {
    let mut statement = json!({
        "Effect": effect,
        "Action": action,
        "Resource": resource,
    });
    if let Some(principal) = principal {
        statement["Principal"] = json!({"AWS": principal});
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// `application/x-www-form-urlencoded` for one awsQuery parameter value, leaving only RFC
/// 3986's unreserved characters literal. It is also how an AWS SDK encodes a non-greedy
/// `@httpLabel`, so `pub(crate)`: the suites that address a resource by its ARN reach this
/// one encoder rather than writing a second copy of it.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
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

/// The handler the read table runs: it makes ONE `GetObject` call per address the event
/// names and reports what each answered, so the invocation's payload IS what the SDK gave
/// the guest — the object's bytes, or the refusal S3 worded.
pub(crate) const READER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("s3")
    results = []
    for address in event["reads"]:
        try:
            answered = client.get_object(Bucket=address["bucket"], Key=address["key"])
        except botocore.exceptions.ClientError as error:
            results.append({
                "read": False,
                "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"],
                "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
            })
            continue
        results.append({
            "read": True,
            "body": answered["Body"].read().decode("utf-8"),
            "length": answered.get("ContentLength"),
            "etag": answered.get("ETag"),
            "status": answered["ResponseMetadata"]["HTTPStatusCode"],
        })
    return {"results": results}
"#;

/// One row of the read table: the bucket it reads, the policy that bucket carries, and what
/// the handler's read of it must come to.
struct Row {
    case: &'static str,
    /// The bucket this row reads, which is one of its own: a row's bucket policy is then the
    /// only resource policy in force on its read.
    bucket: String,
    /// The bucket policy that bucket carries, if any.
    bucket_policy: Option<String>,
    /// Whether the handler must be served the object's bytes.
    served: bool,
    /// The reason clause the refusal must end in, for a row that is refused.
    reason: Option<&'static str>,
}

/// The rows that share ONE identity policy on the execution role, read in one invocation:
/// the role's policies are the FUNCTION's, so what varies within a group is the bucket's own
/// policy and what varies between groups is the role's.
struct Group {
    /// The identity policy the execution role carries for this group's reads, if any.
    identity: Option<String>,
    rows: Vec<Row>,
}

/// The groups: where the grant comes from (the role's own policy, the bucket's policy naming
/// the role, naming its session, naming another role, neither), how widely a grant is scoped
/// (this key, another key, another action) and which side an explicit deny is on.
fn groups(role_name: &str, function_name: &str) -> Vec<Group> {
    // One bucket per row, named for the group and the row it belongs to.
    let bucket = |group: usize, row: usize| format!("{BUCKET_PREFIX}-{group}-{row}");
    // The object a row reads, as a bucket policy names it.
    let object = |group: usize, row: usize| format!("arn:aws:s3:::{}/{KEY}", bucket(group, row));
    // A grant over every bucket of this suite, which is the form a role's own policy usually
    // takes: the rows of a group each read a bucket of their own.
    let every_object = format!("arn:aws:s3:::{BUCKET_PREFIX}-*/*");
    let role = role_arn(role_name);
    let session = session_arn(role_name, function_name);
    let no_grant = Some("because no identity-based policy allows the s3:GetObject action");
    vec![
        Group {
            identity: None,
            rows: vec![
                Row {
                    case: "the bucket policy alone grants the role, with no identity grant",
                    bucket: bucket(0, 0),
                    bucket_policy: Some(document(
                        "Allow",
                        "s3:GetObject",
                        &object(0, 0),
                        Some(&role),
                    )),
                    served: true,
                    reason: None,
                },
                Row {
                    case: "the bucket policy alone grants the role's session",
                    bucket: bucket(0, 1),
                    bucket_policy: Some(document(
                        "Allow",
                        "s3:GetObject",
                        &object(0, 1),
                        Some(&session),
                    )),
                    served: true,
                    reason: None,
                },
                Row {
                    case: "the bucket policy grants another role than this one",
                    bucket: bucket(0, 2),
                    bucket_policy: Some(document(
                        "Allow",
                        "s3:GetObject",
                        &object(0, 2),
                        Some(&role_arn(OTHER_ROLE)),
                    )),
                    served: false,
                    reason: no_grant,
                },
                Row {
                    case: "neither side grants anything",
                    bucket: bucket(0, 3),
                    bucket_policy: None,
                    served: false,
                    reason: no_grant,
                },
            ],
        },
        Group {
            identity: Some(document("Allow", "s3:GetObject", &every_object, None)),
            rows: vec![
                Row {
                    case: "the role's own identity policy grants the read",
                    bucket: bucket(1, 0),
                    bucket_policy: None,
                    served: true,
                    reason: None,
                },
                Row {
                    case: "an explicit deny in the bucket policy overrides the identity grant",
                    bucket: bucket(1, 1),
                    bucket_policy: Some(document(
                        "Deny",
                        "s3:GetObject",
                        &object(1, 1),
                        Some(&role),
                    )),
                    served: false,
                    reason: Some("with an explicit deny in a resource-based policy"),
                },
            ],
        },
        Group {
            // The grant names a key the read does not address, which is no grant for it.
            identity: Some(document(
                "Allow",
                "s3:GetObject",
                &format!("arn:aws:s3:::{BUCKET_PREFIX}-*/{OTHER_KEY}"),
                None,
            )),
            rows: vec![Row {
                case: "the identity grant names another key",
                bucket: bucket(2, 0),
                bucket_policy: None,
                served: false,
                reason: no_grant,
            }],
        },
        Group {
            // The grant names another action on this very object, which is no grant for
            // this one.
            identity: Some(document("Allow", "s3:PutObject", &every_object, None)),
            rows: vec![Row {
                case: "the identity grant names another action",
                bucket: bucket(3, 0),
                bucket_policy: None,
                served: false,
                reason: no_grant,
            }],
        },
        Group {
            identity: Some(document("Deny", "s3:GetObject", &every_object, None)),
            rows: vec![Row {
                case: "an explicit deny in the identity policy overrides the bucket grant",
                bucket: bucket(4, 0),
                bucket_policy: Some(document(
                    "Allow",
                    "s3:GetObject",
                    &object(4, 0),
                    Some(&role),
                )),
                served: false,
                reason: Some("with an explicit deny in an identity-based policy"),
            }],
        },
    ]
}

/// One invocation per identity policy, each reading every bucket of its group, all on ONE
/// emulator and ONE function: what the handler read back, and which principal a refusal
/// named.
///
/// The role's policy is rewritten BETWEEN invocations rather than per emulator, so the
/// groups also pin that each call is judged against the documents in force when it is made —
/// a grant written after the function's credentials were minted takes effect on the next
/// call, and one taken away is refused again.
#[tokio::test]
async fn a_handlers_own_s3_read_is_judged_against_its_execution_roles_policies() {
    let _stream = one_invocation_stream_at_a_time().await;
    let role_name = "lambda-s3-reader";
    let function_name = "s3-reader";
    let emulator = Emulator::new();
    let role = emulator.execution_role("setup", role_name, None).await;
    // The other role a row's bucket policy names: a bucket policy may only name a principal
    // this account holds, so the role the grant is NOT for is a role IAM holds too — which
    // is what makes the row about the grant and not about the ARN.
    emulator.execution_role("setup", OTHER_ROLE, None).await;
    emulator
        .function("setup", function_name, &role, READER_SOURCE)
        .await;
    let groups = groups(role_name, function_name);
    // Every row's own bucket, holding the one object each read addresses and carrying that
    // row's bucket policy.
    for group in &groups {
        for row in &group.rows {
            emulator
                .bucket(row.case, &row.bucket, row.bucket_policy.as_deref())
                .await;
        }
    }

    for group in &groups {
        emulator
            .role_policy("a group of reads", role_name, group.identity.as_deref())
            .await;
        let reads: Vec<Value> = group
            .rows
            .iter()
            .map(|row| json!({ "bucket": row.bucket, "key": KEY }))
            .collect();
        let (signal, payload) = emulator
            .invoke(
                "a group of reads",
                function_name,
                &json!({ "reads": reads }),
            )
            .await;
        assert_eq!(
            signal, None,
            "the handler must run to completion, got the function error {signal:?} with \
             {payload}"
        );
        let answered = payload["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            answered.len(),
            group.rows.len(),
            "expected one answer per read of the group, got {payload}"
        );
        for (row, answer) in group.rows.iter().zip(&answered) {
            let case = row.case;
            let bucket = &row.bucket;
            assert_eq!(
                answer["read"],
                json!(row.served),
                "case {case:?}: expected the read to be served={}, got {answer}",
                row.served
            );
            if row.served {
                assert_eq!(
                    answer["body"],
                    json!(BODY),
                    "case {case:?}: expected the handler to read the bytes the bucket \
                     holds, got {answer}"
                );
                assert_eq!(
                    answer["length"],
                    json!(BODY.len()),
                    "case {case:?}: expected the object's own length, got {answer}"
                );
                assert_eq!(
                    answer["status"],
                    json!(200),
                    "case {case:?}: expected S3's own 200, got {answer}"
                );
                continue;
            }
            assert_eq!(
                answer["code"],
                json!(ACCESS_DENIED),
                "case {case:?}: expected S3's own refusal code, got {answer}"
            );
            assert_eq!(
                answer["status"],
                json!(ACCESS_DENIED_STATUS),
                "case {case:?}: expected S3's own 403, got {answer}"
            );
            // The refusal names the identity the call was made as — the session of the
            // function's execution role, named for the function — the action and the
            // object.
            let expected = format!(
                "User: {} is not authorized to perform: s3:GetObject on resource: \
                 \"arn:aws:s3:::{bucket}/{KEY}\" {}",
                session_arn(role_name, function_name),
                row.reason.unwrap_or_default()
            );
            assert_eq!(
                answer["message"],
                json!(expected),
                "case {case:?}: expected the refusal {expected:?}, got {answer}"
            );
        }
    }
}

/// The handler of the write table: it writes an object, lists the prefix it wrote under and
/// reads one back, reporting what each call answered.
const WRITER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("s3")
    try:
        written = client.put_object(
            Bucket=event["bucket"],
            Key=event["key"],
            Body=event["body"].encode("utf-8"),
            ContentType="text/csv",
            Metadata={"written-by": context.function_name},
        )
        listed = client.list_objects_v2(Bucket=event["bucket"], Prefix=event["prefix"])
        read = client.get_object(Bucket=event["bucket"], Key=event["key"])
        head = client.head_object(Bucket=event["bucket"], Key=event["key"])
    except botocore.exceptions.ClientError as error:
        return {"ok": False, "code": error.response["Error"]["Code"]}
    return {
        "ok": True,
        "etag": written.get("ETag"),
        "keys": sorted(entry["Key"] for entry in listed.get("Contents", [])),
        "count": listed.get("KeyCount"),
        "body": read["Body"].read().decode("utf-8"),
        "content_type": read.get("ContentType"),
        "metadata": head.get("Metadata"),
    }
"#;

/// A handler that writes, lists and reads: every call is served because the role's policy
/// grants each action, and what the handler reported is what the BUCKET holds, read back
/// through S3's own API.
#[tokio::test]
async fn a_handler_writes_lists_and_reads_the_bucket_its_role_may_use() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "a handler whose role grants the object operations it makes";
    let role_name = "lambda-s3-writer";
    let function_name = "s3-writer";
    let written_key = "staged/report.csv";
    let written_body = "written,by\nthe,handler\n";
    let emulator = Emulator::new();
    let identity = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject"],
                "Resource": format!("arn:aws:s3:::{BUCKET}/*"),
            },
            {
                "Effect": "Allow",
                "Action": "s3:ListBucket",
                "Resource": format!("arn:aws:s3:::{BUCKET}"),
            },
        ],
    })
    .to_string();
    let role = emulator
        .execution_role(case, role_name, Some(&identity))
        .await;
    emulator.bucket(case, BUCKET, None).await;
    emulator
        .function(case, function_name, &role, WRITER_SOURCE)
        .await;
    let (signal, payload) = emulator
        .invoke(
            case,
            function_name,
            &json!({
                "bucket": BUCKET,
                "key": written_key,
                "prefix": "staged/",
                "body": written_body,
            }),
        )
        .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got {signal:?} with {payload}"
    );
    assert_eq!(
        payload["ok"],
        json!(true),
        "case {case:?}: expected every call to be served, got {payload}"
    );
    assert_eq!(
        payload["keys"],
        json!([written_key]),
        "case {case:?}: expected the listing to hold the key the handler wrote, got {payload}"
    );
    assert_eq!(
        payload["count"],
        json!(1),
        "case {case:?}: expected the listing to count one key, got {payload}"
    );
    assert_eq!(
        payload["body"],
        json!(written_body),
        "case {case:?}: expected the handler to read back the bytes it wrote, got {payload}"
    );
    assert_eq!(
        payload["content_type"],
        json!("text/csv"),
        "case {case:?}: expected the content type the write named, got {payload}"
    );
    assert_eq!(
        payload["metadata"],
        json!({"written-by": function_name}),
        "case {case:?}: expected the metadata the write named, got {payload}"
    );
    // What the BUCKET holds: the work the handler reported is the work that happened, read
    // where a client reads it.
    let held = emulator
        .s3_ok(case, "GET", &format!("/{BUCKET}/{written_key}"), &[])
        .await;
    assert_eq!(
        held, written_body,
        "case {case:?}: expected the bucket to hold the bytes the handler wrote, got \
         {held:?}"
    );
    let etag = payload["etag"].as_str().unwrap_or_default();
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "case {case:?}: expected the write to answer S3's own quoted ETag, got {etag:?}"
    );
}

/// An operation of Amazon S3 this runtime's SDK does not bind, and a parameter one of its
/// bound operations does not carry, are each reported as a call that was NOT MADE, naming
/// what was not bound — never as a call sent with a value dropped.
#[tokio::test]
async fn a_call_this_runtimes_sdk_cannot_bind_is_not_made() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "a handler naming an unbound operation and an unbound parameter";
    let role_name = "lambda-s3-prober";
    let function_name = "s3-prober";
    let source = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("s3")
    reported = {}
    for name, call in (
        ("unbound_operation", lambda: client.create_bucket(Bucket=event["bucket"])),
        ("unbound_parameter", lambda: client.get_object(
            Bucket=event["bucket"], Key=event["key"], PartNumber=2)),
    ):
        try:
            call()
            reported[name] = None
        except botocore.exceptions.SdkCallNotMade as error:
            reported[name] = str(error)
    return reported
"#;
    let emulator = Emulator::new();
    let identity = document(
        "Allow",
        "s3:GetObject",
        &format!("arn:aws:s3:::{BUCKET}/*"),
        None,
    );
    let role = emulator
        .execution_role(case, role_name, Some(&identity))
        .await;
    emulator.bucket(case, BUCKET, None).await;
    emulator
        .function(case, function_name, &role, source)
        .await;
    let (signal, payload) = emulator
        .invoke(
            case,
            function_name,
            &json!({ "bucket": BUCKET, "key": KEY }),
        )
        .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got {signal:?} with {payload}"
    );
    let operation = payload["unbound_operation"].as_str().unwrap_or_default();
    assert!(
        operation.contains("CreateBucket") && operation.contains("GetObject"),
        "case {case:?}: expected the unbound operation to be named beside the bound ones, \
         got {payload}"
    );
    let parameter = payload["unbound_parameter"].as_str().unwrap_or_default();
    assert!(
        parameter.contains("PartNumber"),
        "case {case:?}: expected the unbound parameter to be named, got {payload}"
    );
}
