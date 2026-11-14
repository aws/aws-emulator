// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this data plane's operations, driven end to end over the real wire path
//! (a restJson1 request through the multi-service router into the mounted service), so
//! what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` (HTTP 403) is a service-common error of
//! this model (`smithy_query(query="service_info")` lists it under `common_errors`), so
//! every operation evaluates the identity-based IAM policies in force on the principal its
//! credentials name — against `payment-cryptography:<OperationName>` on the ARN of the key
//! the request addressed — BEFORE it resolves that key. A request no statement allows
//! (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException` with IAM's own denial sentence and reports nothing of the key,
//! while a request a matching `Allow` covers reaches the operation's own answer.
//!
//! The principals are the real ones: each is made with IAM's and STS's OWN operations over
//! the same engine — `CreateRole` + `PutRolePolicy` + `AssumeRole`, `CreateUser` +
//! `PutUserPolicy` + `CreateAccessKey` — so the documents this service reads are the ones
//! those services wrote, and the keys are the ones the Payment Cryptography CONTROL plane
//! minted.
//!
//! The tables span what the rule generalises over: the operation (all six this service
//! implements, each judged as its own action), the document in force (none at all, another
//! action of this service, another service's actions, an explicit deny beside an allow, an
//! allow of exactly the action, of it spelled in lower case, of the whole namespace and of
//! everything), the resource an allow is scoped to (every resource, exactly the key, every
//! key of the account and Region, another key's ARN) and the kind of caller (an assumed-role
//! session, an IAM user with a long-term key, and the account's own published credential).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::EmulatedService as ControlPlane;
use service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::EmulatedService as DataPlane;

/// The Region every request below signs for.
const REGION: &str = "us-west-2";

/// The IAM action namespace this endpoint's actions are published under, which is also its
/// SigV4 signing name (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "payment-cryptography";

/// The access key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the account's ambient identity, which no
/// identity policy governs — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The `X-Amz-Target` prefix of the control plane's `awsJson1_0` surface.
const CONTROL_TARGET_PREFIX: &str = "PaymentCryptographyControlPlane";

/// The `KeyUsage` values this suite creates keys under, from the control plane's modelled
/// `KeyUsage`.
const USAGE_DATA_ENCRYPTION: &str = "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY";
const USAGE_HMAC: &str = "TR31_M7_HMAC_KEY";
const USAGE_CARD_VERIFICATION: &str = "TR31_C0_CARD_VERIFICATION_KEY";

/// One emulator: this data plane beside the Payment Cryptography control plane that mints
/// its keys and the IAM and STS services the principals are made with, all on ONE shared
/// engine — the same set `core/router`'s `build_router` assembles, narrowed to what this
/// rule touches.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: its HTTP status, the error type the protocol announces, and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The refusal's `message`, under either spelling the model uses for one.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether this answer is the modelled authorization refusal.
    fn refused(&self) -> bool {
        self.error_type.as_deref() == Some("AccessDeniedException")
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the emulator core's business, not this service's: the
        // `Authorization` header carries only the credential scope, which is where a
        // service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(ControlPlane::new(Arc::clone(&engine)));
        router.register_service(DataPlane::new(Arc::clone(&engine)));
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

    /// Dispatch `request` and read back the status, the announced error type and the body.
    async fn dispatch(&self, request: http::Request<Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// Call one DATA plane operation (`restJson1`, `POST` to the operation's own URI),
    /// signed with `access_key_id` — what names the principal the request speaks for.
    async fn data(&self, access_key_id: &str, uri: &str, body: &Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header(
                "host",
                format!("dataplane.payment-cryptography.{REGION}.amazonaws.com"),
            )
            .header("content-type", "application/json")
            .header("authorization", scope(access_key_id, ACTION_PREFIX))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call one CONTROL plane operation (`awsJson1_0`, `POST /` with `X-Amz-Target`) with
    /// the account's own credential.
    async fn control(&self, operation: &str, body: &Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "host",
                format!("controlplane.payment-cryptography.{REGION}.amazonaws.com"),
            )
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{CONTROL_TARGET_PREFIX}.{operation}"))
            .header("authorization", scope(ACCOUNT_KEY, ACTION_PREFIX))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// One awsQuery call to IAM or STS with the account's own credential.
    async fn query(
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
            .header("authorization", scope(ACCOUNT_KEY, signing_name))
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Mint a key through the control plane's own `CreateKey` and answer its ARN.
    async fn create_key(&self, key_algorithm: &str, key_usage: &str, modes: Value) -> String {
        let answer = self
            .control(
                "CreateKey",
                &json!({
                    "KeyAttributes": {
                        "KeyUsage": key_usage,
                        "KeyClass": "SYMMETRIC_KEY",
                        "KeyAlgorithm": key_algorithm,
                        "KeyModesOfUse": modes,
                    },
                    "Exportable": true,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "CreateKey {key_algorithm}/{key_usage}: expected 200, got {answer:?}"
        );
        answer.body["Key"]["KeyArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateKey answered no KeyArn: {answer:?}"))
            .to_string()
    }

    /// Create an IAM role with `policy` as its inline policy (when one is given), assume it
    /// through STS, and answer the session's access key id with the session ARN STS reports
    /// for it — which is the principal a refusal names.
    async fn assumed_role(&self, role_name: &str, policy: Option<&Value>) -> (String, String) {
        let account = engine::emulator_account_id();
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
            }],
        });
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        let key = xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}"));
        let arn = xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {body}"));
        (key, arn)
    }

    /// Create an IAM user with `policy` as its inline policy (when one is given) and answer
    /// the access key id of a long-term key minted for it, with the user's own ARN.
    async fn iam_user(&self, user_name: &str, policy: Option<&Value>) -> (String, String) {
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the user {user_name:?} must succeed, got {status} {body}"
        );
        let arn = xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("CreateUser must answer the user Arn, got {body}"));
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutUserPolicy",
                    &[
                        ("UserName", user_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {user_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "minting a key for {user_name:?} must succeed, got {status} {body}"
        );
        let key = xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"));
        (key, arn)
    }
}

/// The SigV4 credential scope a request carries, which is where a service reads the
/// caller's access key id and the service it was signed for.
fn scope(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Percent-encode a form value the way an awsQuery client does: every byte outside RFC
/// 3986's unreserved set escaped, so a policy document's braces and quotes survive the
/// round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
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

/// Percent-encode the reserved characters a key ARN carries, so it fits the single URI
/// label `EncryptData` and `DecryptData` bind it to.
fn encoded(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// A message of `bytes` bytes, as the uppercase hex every data member's pattern admits.
fn hex_data(bytes: usize) -> String {
    (0..bytes)
        .map(|index| format!("{:02X}", (index * 7 + 3) % 256))
        .collect()
}

/// A policy allowing `actions` on `resource`.
fn allow(actions: Value, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resource}],
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The documents a row puts in force on the principal, and which of IAM's refusals the
/// request then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(action: &str, other: &str) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other), "*")),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("kms:*"), "*")),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of exactly this action beside an allow of it",
            Some(json!({
                "Statement": [
                    {"Effect": "Allow", "Action": action, "Resource": "*"},
                    {"Effect": "Deny", "Action": action, "Resource": "*"},
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an explicit deny of this whole namespace beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    {"Effect": "Allow", "Action": action, "Resource": "*"},
                    {"Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*"},
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action), "*")),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()), "*")),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["s3:GetObject", action]), "*")),
            None,
        ),
        (
            "an allow of every action of this namespace",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")), "*")),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"), "*")), None),
    ]
}

/// One operation of this service, as a client addresses it: the URI, a request body the
/// rule is judged on, and the key ARN its refusal names.
struct Operation {
    name: &'static str,
    uri: String,
    body: Value,
    key_arn: String,
}

/// The keys this suite's operations run under, each minted by the control plane with the
/// modes of use its own operation needs.
struct Keys {
    data: String,
    mac: String,
    card: String,
}

impl Keys {
    async fn mint(emulator: &Emulator) -> Self {
        Self {
            data: emulator
                .create_key(
                    "TDES_2KEY",
                    USAGE_DATA_ENCRYPTION,
                    json!({"Encrypt": true, "Decrypt": true}),
                )
                .await,
            mac: emulator
                .create_key(
                    "HMAC_SHA256",
                    USAGE_HMAC,
                    json!({"Generate": true, "Verify": true}),
                )
                .await,
            card: emulator
                .create_key(
                    "TDES_2KEY",
                    USAGE_CARD_VERIFICATION,
                    json!({"Generate": true, "Verify": true}),
                )
                .await,
        }
    }
}

/// Every operation this service implements, each with a request of its own and the key its
/// refusal names. The rule covers all of them, so the table is the whole implemented
/// surface rather than a sample of it.
fn operations(keys: &Keys) -> Vec<Operation> {
    vec![
        Operation {
            name: "EncryptData",
            uri: format!("/keys/{}/encrypt", encoded(&keys.data)),
            body: json!({
                "PlainText": hex_data(8),
                "EncryptionAttributes": {"Symmetric": {"Mode": "ECB"}},
            }),
            key_arn: keys.data.clone(),
        },
        Operation {
            name: "DecryptData",
            uri: format!("/keys/{}/decrypt", encoded(&keys.data)),
            body: json!({
                "CipherText": hex_data(8),
                "DecryptionAttributes": {"Symmetric": {"Mode": "ECB"}},
            }),
            key_arn: keys.data.clone(),
        },
        Operation {
            name: "GenerateMac",
            uri: "/mac/generate".to_string(),
            body: json!({
                "KeyIdentifier": keys.mac,
                "MessageData": hex_data(12),
                "GenerationAttributes": {"Algorithm": "HMAC"},
            }),
            key_arn: keys.mac.clone(),
        },
        Operation {
            name: "VerifyMac",
            uri: "/mac/verify".to_string(),
            body: json!({
                "KeyIdentifier": keys.mac,
                "MessageData": hex_data(12),
                "Mac": hex_data(16),
                "VerificationAttributes": {"Algorithm": "HMAC"},
            }),
            key_arn: keys.mac.clone(),
        },
        Operation {
            name: "GenerateCardValidationData",
            uri: "/cardvalidationdata/generate".to_string(),
            body: json!({
                "KeyIdentifier": keys.card,
                "PrimaryAccountNumber": "471234567890123",
                "GenerationAttributes": {
                    "CardVerificationValue1": {"CardExpiryDate": "1127", "ServiceCode": "121"},
                },
            }),
            key_arn: keys.card.clone(),
        },
        Operation {
            name: "VerifyCardValidationData",
            uri: "/cardvalidationdata/verify".to_string(),
            body: json!({
                "KeyIdentifier": keys.card,
                "PrimaryAccountNumber": "471234567890123",
                "ValidationData": "123",
                "VerificationAttributes": {
                    "CardVerificationValue1": {"CardExpiryDate": "1127", "ServiceCode": "121"},
                },
            }),
            key_arn: keys.card.clone(),
        },
    ]
}

/// A role name no other row uses, short enough for IAM's own bound.
fn role_name(index: usize) -> String {
    format!("pcd-probe-{index}")
}

/// THE RULE: every operation is judged against its OWN
/// `payment-cryptography:<OperationName>` action on the key the request addressed; a
/// refusal is HTTP 403 `AccessDeniedException` naming the principal, the action, that key's
/// ARN and which of IAM's rules refused it, and an allowed call reaches the operation's own
/// answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    let emulator = Emulator::new();
    let keys = Keys::mint(&emulator).await;
    let mut role = 0usize;

    for operation in operations(&keys) {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        // Another action of this service, which a row grants instead of the one under test.
        let other = operations(&keys)
            .into_iter()
            .map(|row| format!("{ACTION_PREFIX}:{}", row.name))
            .find(|candidate| *candidate != action)
            .expect("this service implements more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", operation.name);
            // A role of its own per row, so no row can pass on another's grant.
            role += 1;
            let (key_id, principal) = emulator
                .assumed_role(&role_name(role), document.as_ref())
                .await;

            let answer = emulator.data(&key_id, &operation.uri, &operation.body).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: expected HTTP 403, got {answer:?}"
                    );
                    assert!(
                        answer.refused(),
                        "case {case:?}: expected an AccessDeniedException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.message(),
                        format!(
                            "User: {principal} is not authorized to perform: {action} on \
                             resource: {} {}",
                            operation.key_arn,
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's wording, got {answer:?}"
                    );
                }
                None => {
                    // Authorization answers BEFORE the key is resolved, so an allowed call
                    // falls through to the operation's own verdict — a computed answer, or
                    // its own refusal of what the request asked for — and never to another
                    // authorization decision.
                    assert!(
                        !answer.refused() && answer.status != 403,
                        "case {case:?}: an allowed request must not be refused, got {answer:?}"
                    );
                }
            }
        }
    }
}

/// RULE: the decision is taken against the ARN of the KEY the request addressed, so an
/// allow scoped to one key covers that key and no other.
#[tokio::test]
async fn an_action_is_judged_against_the_key_the_request_addressed() {
    let emulator = Emulator::new();
    let keys = Keys::mint(&emulator).await;
    let another = emulator
        .create_key(
            "AES_128",
            USAGE_DATA_ENCRYPTION,
            json!({"Encrypt": true, "Decrypt": true}),
        )
        .await;
    let every_key = format!(
        "{}*",
        keys.data
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/"))
            .expect("a key ARN carries a key/<id> resource")
    );

    // (case, the Resource the allow is scoped to, whether it covers the key EncryptData
    // addresses below)
    let rows: Vec<(&str, String, bool)> = vec![
        ("every resource", "*".to_string(), true),
        ("exactly this key's ARN", keys.data.clone(), true),
        ("every key of this account and Region", every_key, true),
        ("another key's ARN", another.clone(), false),
        (
            "a key ARN of another Region",
            keys.data.replace(REGION, "eu-west-3"),
            false,
        ),
    ];

    let mut role = 100usize;
    for (case, resource, covered) in rows {
        role += 1;
        let (key_id, _) = emulator
            .assumed_role(
                &role_name(role),
                Some(&allow(json!(format!("{ACTION_PREFIX}:*")), &resource)),
            )
            .await;
        let answer = emulator
            .data(
                &key_id,
                &format!("/keys/{}/encrypt", encoded(&keys.data)),
                &json!({
                    "PlainText": hex_data(8),
                    "EncryptionAttributes": {"Symmetric": {"Mode": "ECB"}},
                }),
            )
            .await;
        assert_eq!(
            answer.refused(),
            !covered,
            "case {case:?}: an allow scoped to {resource:?} must {}cover this key, got \
             {answer:?}",
            if covered { "" } else { "not " }
        );
    }
}

/// RULE: a long-term IAM access key speaks for the USER that owns it, so the decision is
/// taken on that user's own documents and the refusal names the user's ARN.
#[tokio::test]
async fn a_long_term_key_is_judged_on_its_users_policies() {
    let emulator = Emulator::new();
    let keys = Keys::mint(&emulator).await;
    let action = format!("{ACTION_PREFIX}:GenerateMac");
    let body = json!({
        "KeyIdentifier": keys.mac,
        "MessageData": hex_data(12),
        "GenerationAttributes": {"Algorithm": "HMAC"},
    });

    let (refused_key, refused_user) = emulator.iam_user("pcd-reader", None).await;
    let answer = emulator.data(&refused_key, "/mac/generate", &body).await;
    assert_eq!(
        answer.message(),
        format!(
            "User: {refused_user} is not authorized to perform: {action} on resource: {} \
             because no identity-based policy allows the {action} action",
            keys.mac
        ),
        "case \"a user with no policy\": expected IAM's wording, got {answer:?}"
    );

    let (allowed_key, _) = emulator
        .iam_user("pcd-signer", Some(&allow(json!(action), "*")))
        .await;
    let answer = emulator.data(&allowed_key, "/mac/generate", &body).await;
    assert_eq!(
        answer.status, 200,
        "case \"a user allowed this action\": expected the computed MAC, got {answer:?}"
    );
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever any
/// role or user carries. This is what every other suite of this crate signs with.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let emulator = Emulator::new();
    let keys = Keys::mint(&emulator).await;
    // A role carrying nothing at all exists beside it, which is what refuses a session
    // above and must change nothing for the account's own credential.
    let _ = emulator.assumed_role("pcd-unprivileged", None).await;

    for operation in operations(&keys) {
        let answer = emulator
            .data(ACCOUNT_KEY, &operation.uri, &operation.body)
            .await;
        assert!(
            !answer.refused() && answer.status != 403,
            "case {}: the account's own credential must not be refused, got {answer:?}",
            operation.name
        );
    }
}

/// RULE: the decision is taken before the key is read, so a REFUSED call reports nothing of
/// the key it named — and a key that does not exist at all is refused in exactly the same
/// words as one that does, so the verdict discloses no existence.
#[tokio::test]
async fn a_refused_call_reports_nothing_of_the_key() {
    let emulator = Emulator::new();
    let keys = Keys::mint(&emulator).await;
    let (key_id, principal) = emulator.assumed_role("pcd-nothing", None).await;

    for operation in operations(&keys) {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        let answer = emulator.data(&key_id, &operation.uri, &operation.body).await;
        assert!(
            answer.refused(),
            "case {}: expected the refusal, got {answer:?}",
            operation.name
        );
        let reported: Vec<String> = answer
            .body
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .filter(|member| !["message", "Message", "__type"].contains(&member.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            reported.is_empty(),
            "case {}: a refused call reports only the error, got {reported:?}",
            operation.name
        );
        // The same action against an identifier the control plane holds no key for is
        // refused by the same sentence, naming that identifier as carried: nothing of the
        // verdict says whether a key exists.
        let unknown = "alias/no-such-key";
        let uri = if operation.uri.starts_with("/keys/") {
            format!(
                "/keys/{}/{}",
                encoded(unknown),
                operation.uri.rsplit('/').next().unwrap_or_default()
            )
        } else {
            operation.uri.clone()
        };
        let mut body = operation.body.clone();
        if let Some(object) = body.as_object_mut() {
            if object.contains_key("KeyIdentifier") {
                object.insert("KeyIdentifier".to_string(), json!(unknown));
            }
        }
        let answer = emulator.data(&key_id, &uri, &body).await;
        assert_eq!(
            answer.message(),
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {unknown} because no identity-based policy allows the {action} action"
            ),
            "case {}: an unknown key must be refused in the same words, got {answer:?}",
            operation.name
        );
    }
}

/// RULE: an allowed call is the operation's OWN answer, computed from the key the control
/// plane minted — so the gate decides WHO may call and changes nothing about what is
/// computed: a card validation value generated under the grant verifies under it.
#[tokio::test]
async fn an_allowed_call_answers_from_the_key() {
    let emulator = Emulator::new();
    let keys = Keys::mint(&emulator).await;
    let (key_id, _) = emulator
        .assumed_role(
            "pcd-card",
            Some(&allow(json!(format!("{ACTION_PREFIX}:*")), &keys.card)),
        )
        .await;
    let attributes = json!({
        "CardVerificationValue1": {"CardExpiryDate": "1127", "ServiceCode": "121"},
    });
    let pan = "471234567890123";

    let generated = emulator
        .data(
            &key_id,
            "/cardvalidationdata/generate",
            &json!({
                "KeyIdentifier": keys.card,
                "PrimaryAccountNumber": pan,
                "GenerationAttributes": attributes,
            }),
        )
        .await;
    assert_eq!(
        generated.status, 200,
        "case \"an allowed generate\": expected HTTP 200, got {generated:?}"
    );
    let value = generated.body["ValidationData"]
        .as_str()
        .unwrap_or_else(|| panic!("the generate answered no ValidationData: {generated:?}"))
        .to_string();

    let verified = emulator
        .data(
            &key_id,
            "/cardvalidationdata/verify",
            &json!({
                "KeyIdentifier": keys.card,
                "PrimaryAccountNumber": pan,
                "ValidationData": value,
                "VerificationAttributes": attributes,
            }),
        )
        .await;
    assert_eq!(
        verified.status, 200,
        "case \"an allowed verify of the value just generated\": expected HTTP 200, got \
         {verified:?}"
    );
    assert_eq!(
        verified.body["KeyArn"].as_str(),
        Some(keys.card.as_str()),
        "case \"an allowed verify\": expected the key the request named, got {verified:?}"
    );
}
