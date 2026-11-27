// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the KMS/IAM boundary a SageMaker job's INPUT DOWNLOAD crosses —
//! SageMaker, Amazon S3, IAM and KMS mounted on ONE shared engine, the way `core/router`
//! assembles every service, so every bucket, key, role and object here is made through the
//! owning service's own operations and what is asserted is what a client sees.
//!
//! THE RULE UNDER TEST: an Amazon S3 object stored under a KMS key is readable only THROUGH
//! that key, so a read that serves its bytes performs `kms:Decrypt` on it — "If the object is
//! encrypted with SSE-KMS, you must also have the kms:GenerateDataKey and kms:Decrypt
//! permissions in IAM identity-based policies and KMS key policies for the KMS key"
//! (`HeadObject`'s own account of such a read's permissions). A job's data movement is made
//! under the job's `RoleArn`, so that decrypt is performed as the execution role: where the
//! authorities in force on the key withhold it from that role the object cannot be brought
//! down, and the leg of the job's work that read it FAILS the job — a terminal `Failed` status
//! whose `FailureReason` names the download leg and the refused decrypt — with nothing written
//! under the declared output location. Where they admit it, the job reads the object and
//! completes.
//!
//! The table spans what the rule generalises over: WHICH authority decides (the role's own
//! identity-based grant, the key's own policy, a grant KMS holds for the role), what it decides
//! (admit, say nothing, deny outright), which key the grant is scoped to, and an input that is
//! not stored under a KMS key at all — for which the rule asks nothing of the role.
//!
//! The decrypted bytes are asserted rather than assumed: the job's container copies its input
//! file into its output directory, so the object the job leaves behind holds exactly the bytes
//! the input object held — an output that exists only where the download really served them.
//!
//! Every wait here is a poll for the CONDITION — the status the job reached, the object Amazon
//! S3 serves — never a pause of a fixed length, and the one deadline that bounds a poll is far
//! longer than any span this state gives a job's work, so it bounds a BROKEN emulator rather
//! than the condition.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and so the Region whose endpoint holds the keys
/// those calls name: a KMS key is a regional resource, and an object and the key it is sealed
/// under live at the same endpoint.
const REGION: &str = "us-east-1";

/// The account id the emulator mints its ARNs under.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The service principal a SageMaker execution role's trust document admits.
const SERVICE_PRINCIPAL: &str = service_com_amazonaws_sagemaker_sagemaker::state::SERVICE_PRINCIPAL;

/// The IAM action a read of an SSE-KMS object performs on the key it is sealed under, which a
/// refused job's reason names: the KMS operation such a read performs
/// (`state::DECRYPT_OPERATION`), under the action namespace KMS publishes its actions in.
const DECRYPT_ACTION: &str = "kms:Decrypt";

/// The headers a write asks for server-side encryption under a KMS key with — the
/// `@httpHeader` members `PutObject` binds (`SSEKMSKeyId:
/// {httpHeader: "x-amz-server-side-encryption-aws-kms-key-id"}` and `ServerSideEncryption:
/// {httpHeader: "x-amz-server-side-encryption"}`).
const SSE_HEADER: &str = "x-amz-server-side-encryption";
const SSE_KEY_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// The `ServerSideEncryption` value that is SSE-KMS
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` of the object store's model).
const SSE_KMS: &str = "aws:kms";

/// How long a case polls for a condition before it fails, and how often it looks again.
///
/// GENEROUS on purpose: what is under test is the VERDICT the job reached and what it left
/// behind, never how fast it got there, so the deadline bounds a broken emulator and not the
/// condition.
const SETTLE_BUDGET: Duration = Duration::from_secs(180);
const SETTLE_POLL: Duration = Duration::from_millis(25);

/// The `ProcessingJobStatus` and `TrainingJobStatus` members a job's work has ENDED in.
const TERMINAL_STATUSES: [&str; 3] = ["Completed", "Failed", "Stopped"];

/// The directory a processing container reads its input data under, and the one it writes its
/// results to: the two directories of the processing container's own contract.
const INPUT_DIRECTORY: &str = "/opt/ml/processing/input";
const OUTPUT_DIRECTORY: &str = "/opt/ml/processing/output";

/// The object the job's input prefix holds, and the bytes it holds.
const INPUT_KEY: &str = "secure-input/data.csv";
const INPUT_PREFIX: &str = "secure-input/";
const INPUT_BYTES: &str = "4,1,2\n6,2,3\n8,3,4\n";

/// Where the job's declared output goes, and the object its container leaves there.
const OUTPUT_PREFIX: &str = "secure-output/";
const OUTPUT_KEY: &str = "secure-output/copy.csv";

/// Fail `case` unless there is still time to look again for `what`, then wait one poll interval.
async fn look_again(deadline: Instant, case: &str, what: &str, last: &str) {
    assert!(
        Instant::now() < deadline,
        "case {case:?}: {what} did not hold within {SETTLE_BUDGET:?}; last saw {last}"
    );
    tokio::time::sleep(SETTLE_POLL).await;
}

/// A SigV4 credential scope naming `service`: it is what routes a request to that service. The
/// key id and signature are not credentials — this harness turns request authentication off, so
/// nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// SageMaker, Amazon S3, IAM and KMS on ONE shared engine, the way the router mounts every
/// service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One awsJson1_1 call, as a client makes it: `target` is the `X-Amz-Target` prefix the
    /// service publishes and `signing` the credential scope's service name.
    async fn json(
        &self,
        signing: &str,
        target: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing}.{REGION}.amazonaws.com/"))
            .header("host", format!("{signing}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .header("authorization", authorization(signing))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One SageMaker call that must be served.
    async fn sagemaker_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.json("sagemaker", "SageMaker", operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {answered}"
        );
        answered
    }

    /// One KMS call that must be served.
    async fn kms_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.json("kms", "TrentService", operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {answered}"
        );
        answered
    }

    /// A customer managed KMS key, made with KMS's own `CreateKey`, as its ARN. `policy` is the
    /// key policy the key is created with, and `None` leaves KMS's own default document.
    async fn created_key(&self, case: &str, policy: Option<&str>) -> String {
        let mut request = json!({ "Description": "a job input key" });
        if let Some(policy) = policy {
            request["Policy"] = json!(policy);
        }
        let answered = self.kms_ok(case, "CreateKey", &request).await;
        answered["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created key carries an ARN, got {answered}"))
            .to_string()
    }

    /// A KMS grant on `key_arn` giving `grantee` the operations `operations` names, made with
    /// KMS's own `CreateGrant`.
    async fn created_grant(&self, case: &str, key_arn: &str, grantee: &str, operations: &[&str]) {
        self.kms_ok(
            case,
            "CreateGrant",
            &json!({
                "KeyId": key_arn,
                "GranteePrincipal": grantee,
                "Operations": operations,
            }),
        )
        .await;
    }

    /// One path-style Amazon S3 request carrying `headers`: its status, its response headers and
    /// its body.
    async fn s3(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization("s3"));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().to_vec())
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    async fn given_bucket(&self, case: &str, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), &[], &[]).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the bucket {bucket} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject`, under the KMS key `sealed_under` names when it names
    /// one, and with no encryption of the caller's own otherwise.
    async fn given_object(
        &self,
        case: &str,
        bucket: &str,
        key: &str,
        body: &str,
        sealed_under: Option<&str>,
    ) {
        let headers: Vec<(&str, &str)> = match sealed_under {
            Some(key_arn) => vec![(SSE_HEADER, SSE_KMS), (SSE_KEY_HEADER, key_arn)],
            None => Vec::new(),
        };
        let (status, answered) = self
            .s3("PUT", &format!("/{bucket}/{key}"), &headers, body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the object {key} must be stored, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// One IAM call (`awsQuery`), as a client makes it.
    async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(
                &percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
                    .to_string(),
            );
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Hold `role_name` in IAM, trusting the SageMaker service principal, carrying the Amazon S3
    /// grant a job's own reads and writes are made under and — where `decrypt_on` names a key —
    /// the `kms:Decrypt` grant on that key. Answers the role's ARN.
    async fn given_job_role(
        &self,
        case: &str,
        role_name: &str,
        bucket: &str,
        decrypt_on: Option<&str>,
    ) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", role_name),
                    ("AssumeRolePolicyDocument", &trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the role {role_name} must be created, got {status} {body}"
        );
        let mut statements = vec![json!({
            "Effect": "Allow",
            "Action": ["s3:GetObject", "s3:PutObject", "s3:ListBucket"],
            "Resource": [
                format!("arn:aws:s3:::{bucket}"),
                format!("arn:aws:s3:::{bucket}/*"),
            ],
        })];
        if let Some(key_arn) = decrypt_on {
            statements.push(json!({
                "Effect": "Allow",
                "Action": [DECRYPT_ACTION, "kms:GenerateDataKey"],
                "Resource": key_arn,
            }));
        }
        let grant = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
        let (status, body) = self
            .iam(
                "PutRolePolicy",
                &[
                    ("RoleName", role_name),
                    ("PolicyName", "job-data-access"),
                    ("PolicyDocument", &grant),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the role {role_name} must carry its grant, got {status} {body}"
        );
        format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
    }

    /// The keys Amazon S3 lists under `prefix`, read through `ListObjectsV2`.
    async fn listed_keys(&self, case: &str, bucket: &str, prefix: &str) -> Vec<String> {
        let (status, body) = self
            .s3(
                "GET",
                &format!("/{bucket}?list-type=2&prefix={prefix}"),
                &[],
                &[],
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the listing of {bucket}/{prefix} must be served, got {status} {}",
            String::from_utf8_lossy(&body)
        );
        let text = String::from_utf8_lossy(&body).to_string();
        let mut keys: Vec<String> = text
            .split("<Key>")
            .skip(1)
            .filter_map(|tail| tail.split_once("</Key>").map(|(key, _)| key.to_string()))
            .collect();
        keys.sort();
        keys
    }

    /// The bytes Amazon S3 serves for one object, polled until it serves them.
    async fn object_bytes(&self, case: &str, bucket: &str, key: &str) -> String {
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), &[], &[]).await;
            if status == 200 {
                return String::from_utf8_lossy(&body).to_string();
            }
            look_again(
                deadline,
                case,
                &format!("Amazon S3 serves {bucket}/{key}"),
                &format!("{status} {}", String::from_utf8_lossy(&body)),
            )
            .await;
        }
    }

    /// Poll one job read until it reports a status its work does not leave, and answer it.
    async fn ended(&self, case: &str, operation: &str, request: &Value, member: &str) -> Value {
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            let read = self.sagemaker_ok(case, operation, request).await;
            let status = read
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if TERMINAL_STATUSES.contains(&status.as_str()) {
                return read;
            }
            look_again(
                deadline,
                case,
                "the job reached a terminal status",
                &read.to_string(),
            )
            .await;
        }
    }
}

/// An ECR-shaped path for a container of the caller's own.
fn own_image(repository: &str) -> String {
    format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/{repository}:1")
}

/// A `CreateProcessingJob` body whose container COPIES its downloaded input into its output
/// directory, so the object the job leaves behind holds the bytes the input object held.
fn processing_request(name: &str, role: &str, bucket: &str) -> Value {
    let program = format!("cp {INPUT_DIRECTORY}/data.csv {OUTPUT_DIRECTORY}/copy.csv");
    json!({
        "ProcessingJobName": name,
        "AppSpecification": {
            "ImageUri": own_image("my-processor"),
            "ContainerEntrypoint": ["/bin/sh", "-c"],
            "ContainerArguments": [program],
        },
        "ProcessingResources": {
            "ClusterConfig": {
                "InstanceType": "ml.m5.large",
                "InstanceCount": 1,
                "VolumeSizeInGB": 10,
            },
        },
        "RoleArn": role,
        "ProcessingInputs": [{
            "InputName": "input",
            "S3Input": {
                "S3Uri": format!("s3://{bucket}/{INPUT_PREFIX}"),
                "LocalPath": INPUT_DIRECTORY,
                "S3DataType": "S3Prefix",
                "S3InputMode": "File",
            },
        }],
        "ProcessingOutputConfig": {
            "Outputs": [{
                "OutputName": "output",
                "S3Output": {
                    "S3Uri": format!("s3://{bucket}/{OUTPUT_PREFIX}"),
                    "LocalPath": OUTPUT_DIRECTORY,
                    "S3UploadMode": "EndOfJob",
                },
            }],
        },
    })
}

/// A key policy document: the statement KMS's own default policy carries, which keeps the key
/// manageable by its account, plus `extra` statements of the case's own.
fn key_policy(extra: Vec<Value>) -> String {
    let mut statements = vec![json!({
        "Sid": "Enable IAM User Permissions",
        "Effect": "Allow",
        "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
        "Action": "kms:*",
        "Resource": "*",
    })];
    statements.extend(extra);
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// Which authority a row puts in force on the key the job's input is sealed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authority {
    /// The role's own identity-based policy grants the decrypt on that key.
    IdentityGrantsOnThisKey,
    /// The role's identity policy grants no KMS action at all.
    IdentityGrantsNothing,
    /// The role's identity policy grants the decrypt, but on ANOTHER key.
    IdentityGrantsOnAnotherKey,
    /// The key's own policy names the role in an allow, and its identity policy grants nothing.
    KeyPolicyAllowsTheRole,
    /// The key's policy DENIES the role the decrypt its identity policy grants.
    KeyPolicyDeniesTheRole,
    /// A grant KMS holds for the role names the decrypt, and its identity policy grants nothing.
    GrantNamesTheRole,
    /// The input object is not sealed under a KMS key at all, and the role has no KMS grant.
    NotSealedUnderAKey,
}

/// RULE: a job's input download performs `kms:Decrypt` on the key the object is sealed under, as
/// the job's execution role — so the job completes and reads the object's bytes exactly where
/// the authorities in force admit that action, and fails its download leg with nothing written
/// where they withhold it.
///
/// The rows differ along WHICH authority decides and what it decides (the module header carries
/// the table).
#[tokio::test]
async fn a_processing_jobs_input_download_performs_the_keys_decrypt_as_the_execution_role() {
    // (case, the authority in force, whether the download is served)
    let rows: [(&str, Authority, bool); 7] = [
        (
            "the role's identity policy grants the decrypt on the key",
            Authority::IdentityGrantsOnThisKey,
            true,
        ),
        (
            "the role's identity policy grants no KMS action",
            Authority::IdentityGrantsNothing,
            false,
        ),
        (
            "the role's identity grant names another key",
            Authority::IdentityGrantsOnAnotherKey,
            false,
        ),
        (
            "the key policy names the role in an allow",
            Authority::KeyPolicyAllowsTheRole,
            true,
        ),
        (
            "the key policy denies the role the decrypt its identity policy grants",
            Authority::KeyPolicyDeniesTheRole,
            false,
        ),
        (
            "a KMS grant to the role names the decrypt",
            Authority::GrantNamesTheRole,
            true,
        ),
        (
            "the input object is not sealed under a KMS key",
            Authority::NotSealedUnderAKey,
            true,
        ),
    ];

    for (index, (case, authority, served)) in rows.into_iter().enumerate() {
        let bucket = "job-input-decrypt";
        let emulator = Emulator::new();
        emulator.given_bucket(case, bucket).await;
        let role_name = format!("input-decrypt-role-{index}");
        // The role has to exist before a key policy can name it.
        let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
        let policy = match authority {
            Authority::KeyPolicyAllowsTheRole => Some(key_policy(vec![json!({
                "Effect": "Allow",
                "Principal": { "AWS": role_arn.clone() },
                "Action": DECRYPT_ACTION,
                "Resource": "*",
            })])),
            Authority::KeyPolicyDeniesTheRole => Some(key_policy(vec![json!({
                "Effect": "Deny",
                "Principal": { "AWS": role_arn.clone() },
                "Action": DECRYPT_ACTION,
                "Resource": "*",
            })])),
            _ => None,
        };
        let key_arn = emulator.created_key(case, policy.as_deref()).await;
        // A second key, so the row whose grant names ANOTHER key names a key that really exists.
        let other_key_arn = emulator.created_key(case, None).await;

        let decrypt_on = match authority {
            Authority::IdentityGrantsOnThisKey | Authority::KeyPolicyDeniesTheRole => {
                Some(key_arn.clone())
            }
            Authority::IdentityGrantsOnAnotherKey => Some(other_key_arn.clone()),
            Authority::IdentityGrantsNothing
            | Authority::KeyPolicyAllowsTheRole
            | Authority::GrantNamesTheRole
            | Authority::NotSealedUnderAKey => None,
        };
        let held_role = emulator
            .given_job_role(case, &role_name, bucket, decrypt_on.as_deref())
            .await;
        assert_eq!(
            held_role, role_arn,
            "case {case:?}: the role IAM holds must be the one the key policy named, got \
             {held_role:?}"
        );
        if authority == Authority::GrantNamesTheRole {
            emulator
                .created_grant(case, &key_arn, &role_arn, &["Decrypt"])
                .await;
        }

        let sealed_under = (authority != Authority::NotSealedUnderAKey).then(|| key_arn.clone());
        emulator
            .given_object(case, bucket, INPUT_KEY, INPUT_BYTES, sealed_under.as_deref())
            .await;

        let job = format!("inputdecryptjob{index}");
        emulator
            .sagemaker_ok(
                case,
                "CreateProcessingJob",
                &processing_request(&job, &role_arn, bucket),
            )
            .await;
        let read = emulator
            .ended(
                case,
                "DescribeProcessingJob",
                &json!({ "ProcessingJobName": &job }),
                "ProcessingJobStatus",
            )
            .await;
        let status = read
            .get("ProcessingJobStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reason = read
            .get("FailureReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if served {
            assert_eq!(
                status, "Completed",
                "case {case:?}: the download is admitted, so the job completes, got {read}"
            );
            // The bytes the container copied out of its input directory, which are the bytes the
            // input object holds: an output that exists only where the download served them.
            let copied = emulator.object_bytes(case, bucket, OUTPUT_KEY).await;
            assert_eq!(
                copied, INPUT_BYTES,
                "case {case:?}: the job's output must hold the input's own bytes, got {copied:?}"
            );
        } else {
            assert_eq!(
                status, "Failed",
                "case {case:?}: a download the key's authorities refuse fails the job, got {read}"
            );
            for expected in [
                "Data download failed",
                DECRYPT_ACTION,
                key_arn.as_str(),
                "input",
            ] {
                assert!(
                    reason.contains(expected),
                    "case {case:?}: the reason must name {expected:?}, got {reason:?}"
                );
            }
            let left = emulator.listed_keys(case, bucket, OUTPUT_PREFIX).await;
            assert!(
                left.is_empty(),
                "case {case:?}: a job whose download was refused writes nothing under its \
                 declared output, got {left:?}"
            );
        }
    }
}

/// RULE (the same rule, another operation's data movement): a training job's channel is read the
/// same way, so a channel object sealed under a key the execution role may not decrypt fails the
/// training job's download leg and leaves no model artifact.
#[tokio::test]
async fn a_training_jobs_channel_download_performs_the_keys_decrypt_as_the_execution_role() {
    // The rows differ along whether the role's identity policy grants the decrypt on the key the
    // channel's object is sealed under.
    let rows: [(&str, bool); 2] = [
        ("the role's identity policy grants the decrypt", true),
        ("the role's identity policy grants no KMS action", false),
    ];

    for (index, (case, granted)) in rows.into_iter().enumerate() {
        let bucket = "channel-input-decrypt";
        let emulator = Emulator::new();
        emulator.given_bucket(case, bucket).await;
        let key_arn = emulator.created_key(case, None).await;
        let role_name = format!("channel-decrypt-role-{index}");
        let role_arn = emulator
            .given_job_role(
                case,
                &role_name,
                bucket,
                granted.then(|| key_arn.clone()).as_deref(),
            )
            .await;
        emulator
            .given_object(
                case,
                bucket,
                "training/train.csv",
                INPUT_BYTES,
                Some(&key_arn),
            )
            .await;

        let job = format!("channeldecryptjob{index}");
        emulator
            .sagemaker_ok(
                case,
                "CreateTrainingJob",
                &json!({
                    "TrainingJobName": &job,
                    "AlgorithmSpecification": {
                        "TrainingImage": own_image("my-trainer"),
                        "TrainingInputMode": "File",
                    },
                    "RoleArn": role_arn,
                    "InputDataConfig": [{
                        "ChannelName": "train",
                        "DataSource": {
                            "S3DataSource": {
                                "S3DataType": "S3Prefix",
                                "S3Uri": format!("s3://{bucket}/training/train.csv"),
                            },
                        },
                        "ContentType": "text/csv",
                    }],
                    "OutputDataConfig": { "S3OutputPath": format!("s3://{bucket}/out/") },
                    "ResourceConfig": {
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 1,
                        "VolumeSizeInGB": 10,
                    },
                    "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
                }),
            )
            .await;

        let read = emulator
            .ended(
                case,
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": &job }),
                "TrainingJobStatus",
            )
            .await;
        let status = read
            .get("TrainingJobStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if granted {
            assert_eq!(
                status, "Completed",
                "case {case:?}: the channel's read is admitted, so the job completes, got {read}"
            );
        } else {
            assert_eq!(
                status, "Failed",
                "case {case:?}: a channel read the key's authorities refuse fails the job, got \
                 {read}"
            );
            let reason = read
                .get("FailureReason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            for expected in ["Data download failed", DECRYPT_ACTION, key_arn.as_str()] {
                assert!(
                    reason.contains(expected),
                    "case {case:?}: the reason must name {expected:?}, got {reason:?}"
                );
            }
            let left = emulator.listed_keys(case, bucket, "out/").await;
            assert!(
                left.is_empty(),
                "case {case:?}: a job whose channel read was refused writes no artifact, got \
                 {left:?}"
            );
        }
    }
}
