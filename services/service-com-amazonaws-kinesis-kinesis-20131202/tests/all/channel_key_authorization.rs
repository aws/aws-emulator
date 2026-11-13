// SPDX-License-Identifier: CC0-1.0

//! Whether a CHANNEL's service execution role may use the KMS key its create names, end to
//! end over the wire, with Kinesis, KMS and IAM mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so every document the decision reads is one
//! KMS's or IAM's own handlers wrote.
//!
//! RULE: a channel delivers under its `ServiceExecutionRoleARN` and wraps what it delivers
//! under the key its `EncryptionConfiguration.KeyId` names, so `CreateChannel` checks the two
//! TOGETHER before it provisions anything — which is why it models the whole KMS error family
//! beside `ValidationException`. The role needs `kms:GenerateDataKey` and `kms:Decrypt` on
//! that key, and KMS weighs the two authorities over such a use the way it always does: the
//! key's own policy grants them outright when it names the role, and delegates to the role's
//! identity-based policies when it names the account instead, while an explicit deny in
//! either refuses. A configuration the documents do not allow is refused HTTP 400
//! `ValidationException` with the fixed sentence the service publishes for it, and NO channel
//! exists afterwards — `ListChannels` does not show the name.
//!
//! The recorded flows this was derived from create a channel under a key whose policy denies
//! the data-key actions, and one under a role whose policy omits them, which are rows of the
//! table below. The rows differ along what the rule generalises over: which document carries
//! the grant (the key policy naming the role, the key policy naming the account with the
//! role's own policy allowing the actions), which document refuses it (an explicit deny, a
//! key policy naming another principal only, a role policy granting neither action or only
//! one of the two), and the form the create names the key by (key id, key ARN).
//!
//! Every Kinesis case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>`), IAM its awsQuery one, through
//! `MultiServiceRouter::dispatch`, so the statuses, bodies and modelled errors asserted here
//! are the ones a client sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    CHANNEL_ACTIVE, ROLE_NOT_AUTHORIZED_FOR_KEY_MESSAGE, STREAM_ACTIVE, STREAM_MODE_ON_DEMAND,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{description, member, storage_configuration, stream_configuration};
use crate::stream_tags::{error_code, error_message, poll_until, stream_arn, REGION};

/// The `X-Amz-Target` prefix of each JSON service a case drives.
const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";
const KMS_TARGET_PREFIX: &str = "TrentService";

/// Kinesis, KMS and IAM over ONE engine, so the key and the role a create names are the
/// records those services' own handlers hold.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated; the
        // router still routes by the SigV4 credential scope, which is where the request's
        // region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` of the JSON service `target_prefix` names, signed for `REGION`.
    async fn send(&self, target_prefix: &str, operation: &str, body: &Value) -> (u16, Value) {
        let signing_name = if target_prefix == KMS_TARGET_PREFIX {
            "kms"
        } else {
            "kinesis"
        };
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One Kinesis call.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.send(KINESIS_TARGET_PREFIX, operation, body).await
    }

    /// One Kinesis call that must be answered 200.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body {response}"
        );
        response
    }

    /// One KMS call that must be answered 200.
    async fn kms(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.send(KMS_TARGET_PREFIX, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected KMS {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// One awsQuery call to IAM, answering the XML body it served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from IAM {action}, got {status} with {text}"
        );
        text
    }

    /// A role of this account carrying `policy` as its inline policy, as its ARN.
    async fn role(&self, case: &str, name: &str, policy: &Value) -> String {
        let created = self
            .iam(
                case,
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    (
                        "AssumeRolePolicyDocument",
                        trusted_by_kinesis().to_string(),
                    ),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case}: CreateRole answers the role Arn, got {created}"));
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        arn
    }

    /// A key of this account carrying `policy`, as its `(KeyId, Arn)`.
    async fn key(&self, case: &str, policy: Option<&Value>) -> (String, String) {
        let mut body = json!({});
        if let Some(policy) = policy {
            body["Policy"] = json!(policy.to_string());
        }
        let created = self.kms(case, "CreateKey", &body).await;
        let metadata = &created["KeyMetadata"];
        (
            metadata["KeyId"].as_str().unwrap_or_default().to_string(),
            metadata["Arn"].as_str().unwrap_or_default().to_string(),
        )
    }

    /// An `ACTIVE` on-demand stream named `name`, which is the only capacity mode a channel
    /// supports. Answers its ARN.
    async fn on_demand_stream(&self, case: &str, name: &str) -> String {
        self.ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": name,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
        poll_until(case, &format!("the stream {name} reports {STREAM_ACTIVE}"), || async {
            let (status, response) = self
                .call("DescribeStreamSummary", &json!({ "StreamName": name }))
                .await;
            status == 200
                && response["StreamDescriptionSummary"]["StreamStatus"].as_str()
                    == Some(STREAM_ACTIVE)
        })
        .await;
        stream_arn(REGION, name)
    }

    /// Whether `ListChannels` shows a channel named `name`.
    async fn lists_channel(&self, case: &str, name: &str) -> bool {
        let listed = self.ok(case, "ListChannels", &json!({})).await;
        listed["ChannelSummaries"]
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .any(|summary| summary["ChannelName"].as_str() == Some(name))
            })
            .unwrap_or(false)
    }
}

/// `value` as an `application/x-www-form-urlencoded` field: everything but the unreserved
/// characters percent-encoded, which is how an awsQuery client writes a policy document into
/// a form body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The trust policy a role this service delivers under carries.
fn trusted_by_kinesis() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "kinesis.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }]
    })
}

/// An identity policy allowing `actions` on every resource.
fn allowing(actions: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A key policy whose one statement under test names `principal`, beside the statement every
/// key policy has to carry: KMS refuses a document that would lock the account out of
/// `kms:PutKeyPolicy` ("The new key policy will not allow you to update the key policy in the
/// future"), and that statement grants the account nothing else — so what decides the
/// delivery actions is the statement under test alone.
fn key_policy(effect: &str, principal: Value, actions: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "KeepTheAccountAbleToUpdateThisPolicy",
                "Effect": "Allow",
                "Principal": account_root(),
                "Action": ["kms:PutKeyPolicy", "kms:GetKeyPolicy", "kms:DescribeKey"],
                "Resource": "*",
            },
            {
                "Sid": "ThePolicyUnderTest",
                "Effect": effect,
                "Principal": principal,
                "Action": actions,
                "Resource": "*",
            },
        ]
    })
}

/// The account root, which is the principal the DEFAULT key policy names — "the default key
/// policy … gives the Amazon Web Services account that owns the KMS key permission to use
/// it", which delegates the decision to the account's own identity policies.
fn account_root() -> Value {
    json!({ "AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID) })
}

/// The two KMS operations a channel's delivery needs the key for.
const DELIVERY_ACTIONS: [&str; 2] = ["kms:GenerateDataKey", "kms:Decrypt"];

/// One row of the table: the documents in force, and whether the create is served.
struct Row {
    case: &'static str,
    /// The key policy the key is created with, `None` for the default policy KMS writes.
    key_policy: Option<Value>,
    /// The role's inline policy.
    role_policy: Value,
    /// Whether the create names the key by its ARN rather than by its key id.
    by_arn: bool,
    /// Whether the documents let the role use the key.
    served: bool,
}

/// The rows the rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The default key policy delegates to the account, and the role's own policy allows
        // both actions: the create is served.
        Row {
            case: "channel-key-delegated-and-granted",
            key_policy: None,
            role_policy: allowing(&DELIVERY_ACTIONS),
            by_arn: false,
            served: true,
        },
        // The key policy names the ROLE, which grants the actions outright — the role needs
        // no identity policy of its own for them.
        Row {
            case: "channel-key-policy-names-the-role",
            // Filled in per row below, where the role's ARN is known.
            key_policy: None,
            role_policy: allowing(&["s3:ListAllMyBuckets"]),
            by_arn: true,
            served: true,
        },
        // The key policy denies the data-key actions to EVERYONE: an explicit deny refuses,
        // whatever the role's own policy allows.
        Row {
            case: "channel-key-policy-denies-everyone",
            key_policy: Some(key_policy(
                "Deny",
                json!("*"),
                &["kms:GenerateDataKey*", "kms:Decrypt"],
            )),
            role_policy: allowing(&DELIVERY_ACTIONS),
            by_arn: false,
            served: false,
        },
        // The key policy grants the actions to ANOTHER principal only, so nothing in it
        // reaches this role.
        Row {
            case: "channel-key-policy-names-another-principal",
            key_policy: Some(key_policy(
                "Allow",
                json!({
                    "AWS": format!(
                        "arn:aws:iam::{}:role/another-delivery-role",
                        engine::EMULATOR_ACCOUNT_ID
                    )
                }),
                &DELIVERY_ACTIONS,
            )),
            role_policy: allowing(&DELIVERY_ACTIONS),
            by_arn: true,
            served: false,
        },
        // The key policy delegates to the account and the role's policy grants it nothing of
        // KMS at all.
        Row {
            case: "channel-key-role-granted-nothing",
            key_policy: None,
            role_policy: allowing(&["s3:ListAllMyBuckets"]),
            by_arn: false,
            served: false,
        },
        // The same, with the role granted ONE of the two actions: both are needed.
        Row {
            case: "channel-key-role-granted-one-action",
            key_policy: None,
            role_policy: allowing(&["kms:GenerateDataKey"]),
            by_arn: false,
            served: false,
        },
    ]
}

/// The create is served exactly when the documents in force let the role use the key, and a
/// refused create leaves no channel behind.
#[tokio::test]
async fn a_create_is_served_only_where_the_documents_let_the_role_use_the_key() {
    let emulator = Emulator::new();
    let source = emulator
        .on_demand_stream("channel-key-source", "channel-key-source")
        .await;

    for row in rows() {
        let case = row.case;
        let role = emulator.role(case, case, &row.role_policy).await;
        // The row whose key policy names the role can only be written once that role's ARN
        // exists, which is why the policy is built here rather than in the table.
        let policy = match (row.key_policy.as_ref(), case) {
            (None, "channel-key-policy-names-the-role") => Some(key_policy(
                "Allow",
                json!({ "AWS": role.clone() }),
                &DELIVERY_ACTIONS,
            )),
            (policy, _) => policy.cloned(),
        };
        let (key_id, key_arn) = emulator.key(case, policy.as_ref()).await;
        let named_by = if row.by_arn { &key_arn } else { &key_id };

        let body = json!({
            "ChannelName": case,
            "ServiceExecutionRoleARN": role,
            "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
            "S3DestinationConfiguration": {
                "StorageConfiguration": storage_configuration("channel-key-bucket"),
            },
            "EncryptionConfiguration": { "EncryptionType": "KMS", "KeyId": named_by },
        });
        let (status, response) = emulator.call("CreateChannel", &body).await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {case}: expected the create to be served, got {status} with body \
                 {response}"
            );
            assert_eq!(
                member(case, &description(case, &response), "ChannelName"),
                case,
                "case {case}: expected the created channel reported back, got {response}"
            );
            assert!(
                emulator.lists_channel(case, case).await,
                "case {case}: expected the created channel listed"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            ROLE_NOT_AUTHORIZED_FOR_KEY_MESSAGE,
            "case {case}: expected the sentence the service publishes for a role that may \
             not use the key, got {response}"
        );
        assert!(
            !emulator.lists_channel(case, case).await,
            "case {case}: a refused create leaves no channel behind"
        );
    }
}

/// Granting the role what it was missing lets the identical create through: the decision is
/// read from the documents on every call rather than cached.
#[tokio::test]
async fn the_refusal_lifts_once_the_role_is_granted_the_actions() {
    let emulator = Emulator::new();
    let case = "channel-key-granted-later";
    let source = emulator.on_demand_stream(case, "channel-key-later-source").await;
    let role = emulator
        .role(case, case, &allowing(&["s3:ListAllMyBuckets"]))
        .await;
    let (key_id, _) = emulator.key(case, None).await;
    let body = json!({
        "ChannelName": case,
        "ServiceExecutionRoleARN": role,
        "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
        "S3DestinationConfiguration": {
            "StorageConfiguration": storage_configuration("channel-key-bucket"),
        },
        "EncryptionConfiguration": { "EncryptionType": "KMS", "KeyId": key_id },
    });

    let (status, response) = emulator.call("CreateChannel", &body).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {case}: expected the create refused while the role lacks the actions, got \
         {status} with body {response}"
    );

    emulator
        .iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", case.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", allowing(&DELIVERY_ACTIONS).to_string()),
            ],
        )
        .await;

    let created = emulator.ok(case, "CreateChannel", &body).await;
    assert_eq!(
        member(case, &description(case, &created), "ChannelName"),
        case,
        "case {case}: expected the identical create served once the role was granted the \
         actions, got {created}"
    );
    // The channel the second create minted is a real one: it settles and can be read back.
    poll_until(case, "the channel reports ACTIVE", || async {
        let (status, response) = emulator
            .send(
                KINESIS_TARGET_PREFIX,
                "DescribeChannel",
                &json!({ "ChannelARN": channel_arn_of(case) }),
            )
            .await;
        status == 200
            && response["ChannelDescription"]["ChannelStatus"].as_str() == Some(CHANNEL_ACTIVE)
    })
    .await;
    assert!(
        emulator.lists_channel(case, case).await,
        "case {case}: expected the created channel listed"
    );
}

/// The ARN a channel named `name` of this account and region is addressed by.
fn channel_arn_of(name: &str) -> String {
    format!(
        "arn:aws:kinesis:{REGION}:{}:channel/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}
