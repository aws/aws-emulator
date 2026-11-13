// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for RESOLVING the customer managed key a CloudWatch Logs request names,
//! before any association with it is recorded.
//!
//! The rule under test: **a `kmsKeyId` names a KMS key only as that KEY's own ARN, and the key
//! it names is usable only when KMS itself reports a key in its enabled state — an identifier
//! KMS can find no key for is refused with `InvalidParameterException` and `The specified KMS
//! Key Id could not be found.`, a key KMS reports as not enabled with the same code and `The
//! specified KMS key is marked as disabled.`, and nothing is recorded either way.**
//! `AssociateKmsKey` states the class: "If you attempt to associate a KMS key with a log group
//! but the KMS key does not exist or the KMS key is disabled, you receive an
//! InvalidParameterException error", and the member is documented as "The Amazon Resource Name
//! (ARN) of the KMS key to use when encrypting log data".
//!
//! CloudWatch Logs and KMS are mounted on ONE shared engine — the same assembly `core/router`
//! performs — so every key a row names is a key KMS itself holds, in the state KMS itself
//! reports, and no part of this suite tells CloudWatch Logs anything about it. The key policy
//! gate that follows this resolution is the subject of `tests/all/cmk_key_policy_scope.rs`, so
//! every key created here carries a policy that lets CloudWatch Logs use it.
//!
//! The rows span what the rule generalises over: the FORM of the identifier (a key ARN, an
//! alias ARN of the very same key, an alias name, a bare key id, a value that is no ARN), and
//! what KMS reports for a well-formed key ARN (a key it does not hold, a disabled key, a key
//! pending deletion, and the same key once it is enabled again). Each row is driven through
//! both operations that take a `kmsKeyId` for a log group — `CreateLogGroup` and
//! `AssociateKmsKey`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for, which is the Region the ARNs carry.
const REGION: &str = "eu-west-2";

/// The error code every refusal of this rule carries, as `AssociateKmsKey` documents it.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// CloudWatch Logs and KMS on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One awsJson1_1 call to the service signing as `service` under `prefix`.
    async fn call(
        &self,
        service: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs call: its status and body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call("logs", "Logs_20140328", operation, body).await
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One KMS call that must succeed.
    async fn kms_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call("kms", "TrentService", operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: KMS {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// Create a KMS key, answering its ARN. `extra` carries the members a row's key needs
    /// beyond the policy — a key spec and a usage, for a key that is not a symmetric
    /// encryption key.
    async fn created_key(&self, case: &str, extra: Value) -> String {
        let mut body = json!({
            "Policy": permissive_key_policy(),
            "BypassPolicyLockoutSafetyCheck": true,
        });
        let map = body.as_object_mut().expect("the request body is an object");
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            map.insert(member, value);
        }
        let created = self.kms_ok(case, "CreateKey", &body).await;
        created["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created key carries an Arn, got {created}"))
            .to_string()
    }

    /// Whether CloudWatch Logs holds a log group under `name`.
    async fn holds_group(&self, case: &str, name: &str) -> bool {
        self.described_group(case, name).await.is_some()
    }

    /// The `kmsKeyId` CloudWatch Logs reports for `name`, or `None` when it reports none.
    async fn reported_key(&self, case: &str, name: &str) -> Option<String> {
        self.described_group(case, name)
            .await
            .and_then(|group| {
                group
                    .get("kmsKeyId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    /// The one log group CloudWatch Logs reports under `name`, read back through the operation
    /// a client reads it with.
    async fn described_group(&self, case: &str, name: &str) -> Option<Value> {
        let page = self
            .logs_ok(
                case,
                "DescribeLogGroups",
                &json!({"logGroupIdentifiers": [name]}),
            )
            .await;
        page["logGroups"]
            .as_array()
            .and_then(|groups| groups.first())
            .cloned()
    }
}

/// A key policy letting CloudWatch Logs use the key for any resource of this account, so the
/// KEY POLICY gate (the subject of another suite) never decides a row here.
fn permissive_key_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "Enable IAM User Permissions",
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
                "Action": "kms:*",
                "Resource": "*",
            },
            {
                "Effect": "Allow",
                "Principal": {"Service": "logs.amazonaws.com"},
                "Action": [
                    "kms:Encrypt*",
                    "kms:Decrypt*",
                    "kms:ReEncrypt*",
                    "kms:GenerateDataKey*",
                    "kms:Describe*",
                ],
                "Resource": "*",
            },
        ],
    })
    .to_string()
}

/// A log group's ARN in the form a refusal about the key names it — `LogGroup$logGroupArn`,
/// which carries no trailing `:*`.
fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}",
        engine::emulator_account_id()
    )
}

/// What the service must answer for one `kmsKeyId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The key is resolved and the association is recorded.
    Accepted,
    /// Refused: KMS can find no key under the identifier — which includes every identifier
    /// form that is not a key's own ARN.
    NotFound,
    /// Refused: KMS reports the key, in a state it cannot be used in.
    Disabled,
}

impl Expected {
    /// The message this outcome's refusal carries. Both are fixed text for their class, naming
    /// nothing of the request.
    fn message(self) -> Option<&'static str> {
        match self {
            Self::Accepted => None,
            Self::NotFound => Some("The specified KMS Key Id could not be found."),
            Self::Disabled => Some("The specified KMS key is marked as disabled."),
        }
    }
}

/// One row: an identifier to name, and what the service must answer for it.
struct Row {
    /// What the row varies, named in every assertion.
    case: String,
    /// The `kmsKeyId` the row names.
    kms_key_id: String,
    /// What the service must answer.
    expected: Expected,
}

/// The rows, built against one emulator: each key a row names is created through KMS's own API
/// here, so what the rule is judged on is the state KMS reports.
async fn rows(emulator: &Emulator) -> Vec<Row> {
    let usable = emulator.created_key("setup", json!({})).await;
    let disabled = emulator.created_key("setup", json!({})).await;
    emulator
        .kms_ok("setup", "DisableKey", &json!({"KeyId": disabled}))
        .await;
    // A key SCHEDULED FOR DELETION is reported by KMS in a state it cannot be used in, the
    // same as a disabled one: "when KeyState is Enabled this value is true, otherwise it is
    // false" (`KeyMetadata$Enabled`).
    let pending_deletion = emulator.created_key("setup", json!({})).await;
    emulator
        .kms_ok(
            "setup",
            "ScheduleKeyDeletion",
            &json!({"KeyId": pending_deletion, "PendingWindowInDays": 7}),
        )
        .await;
    // An ALIAS of the very same usable key: the alias resolves to that key in KMS, and is still
    // not the key's own ARN.
    const ALIAS_NAME: &str = "alias/cloudwatch-logs";
    emulator
        .kms_ok(
            "setup",
            "CreateAlias",
            &json!({"AliasName": ALIAS_NAME, "TargetKeyId": usable}),
        )
        .await;
    let alias_arn = engine::emulator_arn("kms", REGION, ALIAS_NAME);
    // The key id of the usable key, which is the ARN's own resource id without the ARN around
    // it.
    let bare_key_id = usable
        .rsplit_once("key/")
        .expect("a key ARN carries a key id")
        .1
        .to_string();
    let absent = engine::emulator_arn("kms", REGION, "key/8d6a0b2e-19cc-4ff6-9a6f-2d0f6b5b1cf3");

    vec![
        Row {
            case: "the ARN of an enabled key".to_string(),
            kms_key_id: usable.clone(),
            expected: Expected::Accepted,
        },
        Row {
            case: "an alias ARN of that same key".to_string(),
            kms_key_id: alias_arn,
            expected: Expected::NotFound,
        },
        Row {
            case: "an alias NAME of that same key".to_string(),
            kms_key_id: ALIAS_NAME.to_string(),
            expected: Expected::NotFound,
        },
        Row {
            case: "the bare key id of that same key".to_string(),
            kms_key_id: bare_key_id,
            expected: Expected::NotFound,
        },
        Row {
            case: "a value that is no ARN at all".to_string(),
            kms_key_id: "not-an-arn".to_string(),
            expected: Expected::NotFound,
        },
        Row {
            case: "an ARN of another vendor".to_string(),
            kms_key_id: log_group_arn("some-group"),
            expected: Expected::NotFound,
        },
        Row {
            case: "a well-formed key ARN KMS holds no key for".to_string(),
            kms_key_id: absent,
            expected: Expected::NotFound,
        },
        Row {
            case: "the ARN of a disabled key".to_string(),
            kms_key_id: disabled,
            expected: Expected::Disabled,
        },
        Row {
            case: "the ARN of a key pending deletion".to_string(),
            kms_key_id: pending_deletion,
            expected: Expected::Disabled,
        },
    ]
}

/// `CreateLogGroup` creates the group only for a `kmsKeyId` that resolves to a usable key, and
/// a refused create leaves no group behind.
#[tokio::test]
async fn create_log_group_resolves_the_key_before_it_creates_anything() {
    let emulator = Emulator::new();
    for (index, row) in rows(&emulator).await.into_iter().enumerate() {
        let group = format!("resolution/create-{index}");
        let case = &row.case;
        let (status, parsed) = emulator
            .logs(
                "CreateLogGroup",
                &json!({"logGroupName": group, "kmsKeyId": row.kms_key_id}),
            )
            .await;

        match row.expected.message() {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the key resolves, so the create must succeed, got {status} \
                     {parsed}"
                );
                assert_eq!(
                    emulator.reported_key(case, &group).await.as_deref(),
                    Some(row.kms_key_id.as_str()),
                    "case {case:?}: the group reports the key it was created with"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to be refused, got {status} {parsed}"
                );
                assert_eq!(
                    crate::harness::error_code(&parsed),
                    INVALID_PARAMETER,
                    "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
                );
                assert_eq!(
                    parsed.get("message").and_then(Value::as_str),
                    Some(expected),
                    "case {case:?}: expected {expected:?}, got {parsed}"
                );
                assert!(
                    !emulator.holds_group(case, &group).await,
                    "case {case:?}: a refused create leaves no log group behind"
                );
            }
        }
    }
}

/// `AssociateKmsKey` records the key only for a `kmsKeyId` that resolves to a usable key, and a
/// refused association leaves the group's own `kmsKeyId` as it was.
#[tokio::test]
async fn associate_kms_key_resolves_the_key_before_it_records_anything() {
    let emulator = Emulator::new();
    for (index, row) in rows(&emulator).await.into_iter().enumerate() {
        let group = format!("resolution/associate-{index}");
        let case = &row.case;
        emulator
            .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;

        let (status, parsed) = emulator
            .logs(
                "AssociateKmsKey",
                &json!({"logGroupName": group, "kmsKeyId": row.kms_key_id}),
            )
            .await;

        match row.expected.message() {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the key resolves, so the association must succeed, got \
                     {status} {parsed}"
                );
                assert_eq!(
                    emulator.reported_key(case, &group).await.as_deref(),
                    Some(row.kms_key_id.as_str()),
                    "case {case:?}: the group reports the associated key"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the association to be refused, got {status} {parsed}"
                );
                assert_eq!(
                    crate::harness::error_code(&parsed),
                    INVALID_PARAMETER,
                    "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
                );
                assert_eq!(
                    parsed.get("message").and_then(Value::as_str),
                    Some(expected),
                    "case {case:?}: expected {expected:?}, got {parsed}"
                );
                assert_eq!(
                    emulator.reported_key(case, &group).await,
                    None,
                    "case {case:?}: a refused association records no key"
                );
            }
        }
    }
}

/// The STATE of the key is read on every call rather than once: a key that is disabled is
/// refused, and the very same key is accepted again once KMS reports it enabled — and the
/// association a refusal did not make is not there in between.
#[tokio::test]
async fn a_disabled_key_is_refused_until_kms_reports_it_enabled_again() {
    let emulator = Emulator::new();
    let case = "a key disabled and then enabled again";
    let group = "resolution/key-state";
    let key = emulator.created_key(case, json!({})).await;
    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;

    emulator
        .kms_ok(case, "DisableKey", &json!({"KeyId": key}))
        .await;
    let (status, parsed) = emulator
        .logs(
            "AssociateKmsKey",
            &json!({"logGroupName": group, "kmsKeyId": key}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a disabled key is refused, got {status} {parsed}"
    );
    assert_eq!(
        emulator.reported_key(case, group).await,
        None,
        "case {case:?}: the refused association recorded no key"
    );

    emulator
        .kms_ok(case, "EnableKey", &json!({"KeyId": key}))
        .await;
    emulator
        .logs_ok(
            case,
            "AssociateKmsKey",
            &json!({"logGroupName": group, "kmsKeyId": key}),
        )
        .await;
    assert_eq!(
        emulator.reported_key(case, group).await.as_deref(),
        Some(key.as_str()),
        "case {case:?}: the key the same request named is recorded once KMS reports it enabled"
    );
}
