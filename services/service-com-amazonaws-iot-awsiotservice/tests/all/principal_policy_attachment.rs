// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `AttachPrincipalPolicy`, the DEPRECATED spelling of
//! `AttachPolicy`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name, with the principal travelling in
//! the `x-amzn-iot-principal` header the model binds it to — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: the legacy call records the pair (policy, principal) in the one
//! persisted policy-attachment relation the current spelling writes, so every reader of
//! that relation (`ListAttachedPolicies` over the principal, `ListTargetsForPolicy` over
//! the policy) reports what it recorded and `DetachPolicy` removes it; the attach is
//! idempotent; and a pair is only recorded for resources that exist, so a policy name the
//! registry does not hold and a well-formed principal naming a resource this account does
//! not hold are refused `ResourceNotFoundException` (404) while a principal of no
//! supported form is refused `InvalidRequestException` (400) — never a 200 recording an
//! attachment to nothing.
//!
//! The tables span the class the rule generalises over rather than one recorded request:
//! each principal form the modelled member documents (a certificate ARN, a thing group
//! ARN and a Cognito identity), a certificate that is ACTIVE and one that is INACTIVE,
//! and policy names of several forms the modelled `PolicyName` pattern `^[\w+=,.@-]+$`
//! admits.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";
/// An IoT policy document every policy in this suite is created with: the document is not
/// what this suite is about, so one valid document serves every case.
const POLICY_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:*","Resource":"*"}]}"#;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never checked
/// here — the harness turns request authentication off, exactly as an unauthenticated
/// emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The `message` of an error body.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `policyName` of every entry of a `ListAttachedPolicies` answer, in the order
    /// the listing reported them.
    fn policy_names(&self) -> Vec<String> {
        self.entries("policies")
            .iter()
            .map(|entry| {
                entry
                    .get("policyName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `policyArn` of every entry of a `ListAttachedPolicies` answer.
    fn policy_arns(&self) -> Vec<String> {
        self.entries("policies")
            .iter()
            .map(|entry| {
                entry
                    .get("policyArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `targets` of a `ListTargetsForPolicy` answer.
    fn targets(&self) -> Vec<String> {
        self.entries("targets")
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// One list member of the body, or an empty list when it is absent.
    fn entries(&self, name: &str) -> Vec<Value> {
        self.body
            .get(name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the body carries no member at all — what an operation whose output shape
    /// is `Unit` answers with.
    fn is_empty_body(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// One request, with any number of extra headers — the legacy pair carries its
    /// principal in one of them.
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreatePolicy`: `POST /policies/{policyName}`.
    async fn create_policy(&self, policy_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(&json!({ "policyDocument": POLICY_DOCUMENT })),
            &[],
        )
        .await
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, which issues the
    /// certificate a policy is attached to.
    async fn create_certificate(&self, set_as_active: bool) -> Reply {
        self.call(
            "POST",
            &format!("/keys-and-certificate?setAsActive={set_as_active}"),
            None,
            &[],
        )
        .await
    }

    /// `CreateThingGroup`: `POST /thing-groups/{thingGroupName}`.
    async fn create_thing_group(&self, name: &str) -> Reply {
        self.call("POST", &format!("/thing-groups/{name}"), Some(&json!({})), &[])
            .await
    }

    /// `AttachPrincipalPolicy`: `PUT /principal-policies/{policyName}` carrying the
    /// principal in the `x-amzn-iot-principal` header the model binds it to.
    async fn attach_principal_policy(&self, policy_name: &str, principal: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/principal-policies/{policy_name}"),
            None,
            &[("x-amzn-iot-principal", principal)],
        )
        .await
    }

    /// `AttachPolicy`: `PUT /target-policies/{policyName}` carrying the target in the
    /// body — the CURRENT spelling of the same effect.
    async fn attach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
            &[],
        )
        .await
    }

    /// `DetachPolicy`: `POST /target-policies/{policyName}` carrying the target.
    async fn detach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "POST",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
            &[],
        )
        .await
    }

    /// `ListAttachedPolicies`: `POST /attached-policies/{target}`, whose target is ONE
    /// path segment ([`as_path_segment`]).
    async fn list_attached_policies(&self, target: &str) -> Reply {
        self.call(
            "POST",
            &format!("/attached-policies/{}", as_path_segment(target)),
            None,
            &[],
        )
        .await
    }

    /// `ListTargetsForPolicy`: `POST /policy-targets/{policyName}`.
    async fn list_targets_for_policy(&self, policy_name: &str) -> Reply {
        self.call("POST", &format!("/policy-targets/{policy_name}"), None, &[])
            .await
    }
}

/// A target as ONE URI path segment: the `:` and `/` of an ARN travel as `%3A` and `%2F`,
/// or the request addresses a different route.
fn as_path_segment(target: &str) -> String {
    target
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The ARN a policy created in this suite's region carries.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:policy/{policy_name}")
}

/// The ARN of a thing group created in this suite's region.
fn thing_group_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/{name}")
}

/// The ARN of a certificate in this suite's region, for a certificate id this account
/// does not hold.
fn certificate_arn(certificate_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert/{certificate_id}")
}

/// Create `policy_name`, asserting the create succeeded: every attachment case starts
/// from a policy that exists.
async fn given_policy(emulator: &Emulator, policy_name: &str) {
    let created = emulator.create_policy(policy_name).await;
    assert_eq!(
        created.status, 200,
        "creating the policy {policy_name} must succeed, got {} {:?}",
        created.status, created.body
    );
}

/// Issue a certificate, answering its `certificateArn`.
async fn given_certificate(emulator: &Emulator, set_as_active: bool) -> String {
    let issued = emulator.create_certificate(set_as_active).await;
    assert_eq!(
        issued.status, 200,
        "issuing a certificate must succeed, got {} {:?}",
        issued.status, issued.body
    );
    issued
        .body
        .get("certificateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create a thing group, answering its `thingGroupArn`.
async fn given_thing_group(emulator: &Emulator, name: &str) -> String {
    let created = emulator.create_thing_group(name).await;
    assert_eq!(
        created.status, 200,
        "creating the thing group {name} must succeed, got {} {:?}",
        created.status, created.body
    );
    created
        .body
        .get("thingGroupArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The legacy attach records the pair both readers of the relation report, for each
/// principal form the modelled member documents: a certificate ARN (ACTIVE and INACTIVE),
/// a thing group ARN and a Cognito identity. The policy names span several forms the
/// modelled `PolicyName` pattern `^[\w+=,.@-]+$` admits.
#[tokio::test]
async fn attaching_a_policy_to_a_principal_records_the_pair_both_listings_report() {
    let emulator = Emulator::new();

    let active_certificate = given_certificate(&emulator, true).await;
    let inactive_certificate = given_certificate(&emulator, false).await;
    let group = given_thing_group(&emulator, "legacy-group").await;

    // One row per principal form, each with a policy name of its own form.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "an ACTIVE certificate ARN",
            active_certificate.clone(),
            "LegacyPolicy",
        ),
        (
            "an INACTIVE certificate ARN",
            inactive_certificate.clone(),
            "legacy_policy-2",
        ),
        ("a thing group ARN", group.clone(), "legacy+policy=3,.@-"),
        (
            "a Cognito identity",
            format!("{REGION}:0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"),
            "legacy.policy.4",
        ),
    ];

    for (label, principal, policy_name) in &cases {
        given_policy(&emulator, policy_name).await;

        let attached = emulator.attach_principal_policy(policy_name, principal).await;
        assert_eq!(
            attached.status, 200,
            "case {label}: AttachPrincipalPolicy must answer 200, got {} {:?}",
            attached.status, attached.body
        );
        assert!(
            attached.is_empty_body(),
            "case {label}: the modelled output is Unit, so the body carries no member, \
             got {:?}",
            attached.body
        );

        // Read the attachment back through the relation's own reader, over the principal.
        let listed = emulator.list_attached_policies(principal).await;
        assert_eq!(
            listed.status, 200,
            "case {label}: ListAttachedPolicies must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.policy_names(),
            vec![(*policy_name).to_string()],
            "case {label}: expected the attached policy to be reported, got {:?}",
            listed.policy_names()
        );
        assert_eq!(
            listed.policy_arns(),
            vec![policy_arn(policy_name)],
            "case {label}: expected the policy registry's own ARN, got {:?}",
            listed.policy_arns()
        );

        // And over the policy, which reports the principal as the account holds it.
        let targets = emulator.list_targets_for_policy(policy_name).await;
        assert_eq!(
            targets.status, 200,
            "case {label}: ListTargetsForPolicy must answer 200, got {} {:?}",
            targets.status, targets.body
        );
        assert_eq!(
            targets.targets(),
            vec![principal.clone()],
            "case {label}: expected the principal to be reported as a target, got {:?}",
            targets.targets()
        );
    }
}

/// The legacy spelling and the current one write ONE relation: a pair recorded by either
/// is reported once however many times it is attached, and `DetachPolicy` removes it.
#[tokio::test]
async fn the_legacy_and_current_spellings_write_one_relation() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    given_policy(&emulator, "shared-policy").await;
    given_policy(&emulator, "legacy-only-policy").await;

    // Attaching the same pair through both spellings, twice each: the relation holds one
    // row, so the listing reports the policy once.
    for (label, through_legacy) in [
        ("the legacy spelling", true),
        ("the legacy spelling again", true),
        ("the current spelling", false),
        ("the legacy spelling after the current one", true),
    ] {
        let attached = if through_legacy {
            emulator
                .attach_principal_policy("shared-policy", &certificate)
                .await
        } else {
            emulator.attach_policy("shared-policy", &certificate).await
        };
        assert_eq!(
            attached.status, 200,
            "case {label}: the attach must answer 200, got {} {:?}",
            attached.status, attached.body
        );
        let listed = emulator.list_attached_policies(&certificate).await;
        assert_eq!(
            listed.policy_names(),
            vec!["shared-policy".to_string()],
            "case {label}: the pair is recorded once, got {:?}",
            listed.policy_names()
        );
    }

    // A second policy attached through the legacy spelling joins the same target's rows.
    let attached = emulator
        .attach_principal_policy("legacy-only-policy", &certificate)
        .await;
    assert_eq!(
        attached.status, 200,
        "attaching a second policy must answer 200, got {} {:?}",
        attached.status, attached.body
    );
    let listed = emulator.list_attached_policies(&certificate).await;
    assert_eq!(
        listed.policy_names(),
        vec![
            "legacy-only-policy".to_string(),
            "shared-policy".to_string()
        ],
        "both policies of the principal must be reported, got {:?}",
        listed.policy_names()
    );

    // What the legacy call recorded, the current spelling's detach removes: one relation.
    let detached = emulator
        .detach_policy("legacy-only-policy", &certificate)
        .await;
    assert_eq!(
        detached.status, 200,
        "DetachPolicy must answer 200, got {} {:?}",
        detached.status, detached.body
    );
    let listed = emulator.list_attached_policies(&certificate).await;
    assert_eq!(
        listed.policy_names(),
        vec!["shared-policy".to_string()],
        "the detached pair must be gone and the other left, got {:?}",
        listed.policy_names()
    );
}

/// A pair is only recorded for resources that exist: a policy name the registry does not
/// hold and a well-formed principal naming a resource this account does not hold are
/// refused 404, a principal of no supported form 400 — and nothing is recorded either
/// way.
#[tokio::test]
async fn an_attachment_to_a_principal_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    given_policy(&emulator, "known-policy").await;
    given_thing_group(&emulator, "known-group").await;

    // A certificate id this account holds no certificate under: 64 hex characters, the
    // modelled `CertificateId` form (`length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`).
    let unknown_certificate = certificate_arn(&"ab".repeat(32));

    let not_found: Vec<(&str, &str, String, String)> = vec![
        (
            "an unknown policy name",
            "no-such-policy",
            certificate.clone(),
            "Policy no-such-policy does not exist".to_string(),
        ),
        (
            "an unknown certificate principal",
            "known-policy",
            unknown_certificate,
            "The certificate given in the principal does not exist.".to_string(),
        ),
        (
            "an unknown thing group principal",
            "known-policy",
            thing_group_arn("no-such-group"),
            format!("ThingGroup {ACCOUNT_ID}:no-such-group not found."),
        ),
    ];

    for (label, policy_name, principal, message) in &not_found {
        let refused = emulator.attach_principal_policy(policy_name, principal).await;
        assert_eq!(
            refused.status, 404,
            "case {label}: AttachPrincipalPolicy must refuse it 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            &refused.message(),
            message,
            "case {label}: expected the message {message:?}, got {:?}",
            refused.message()
        );
    }

    // Strings that name no principal of this account's policies at all.
    let invalid = vec![
        ("a bare name", "plant-floor".to_string()),
        ("a path with no ARN prefix", "cert/abc".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/device"),
        ),
        (
            "an IoT ARN of a resource no policy attaches to",
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/sensor-1"),
        ),
        (
            "a certificate ARN with no id",
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert/"),
        ),
        (
            "an IoT ARN with no resource type",
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert"),
        ),
        ("a Cognito identity with no id", format!("{REGION}:")),
        ("a Cognito identity with no region", ":abcdef".to_string()),
    ];

    for (label, principal) in &invalid {
        let refused = emulator
            .attach_principal_policy("known-policy", principal)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label}: AttachPrincipalPolicy must refuse it 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label}: expected InvalidRequestException, got {:?}",
            refused.error_code
        );
        assert!(
            refused.message().contains("policy target"),
            "case {label}: the refusal must name what a principal may be, got {:?}",
            refused.message()
        );
    }

    // Nothing was recorded by any refusal: the policy that exists holds no target, and
    // the certificate and the group that exist hold no policy.
    let targets = emulator.list_targets_for_policy("known-policy").await;
    assert!(
        targets.targets().is_empty(),
        "a refused attach records nothing, got the targets {:?}",
        targets.targets()
    );
    for (label, principal) in [
        ("the certificate", certificate),
        ("the thing group", thing_group_arn("known-group")),
    ] {
        let listed = emulator.list_attached_policies(&principal).await;
        assert!(
            listed.policy_names().is_empty(),
            "case {label}: a refused attach records nothing, got {:?}",
            listed.policy_names()
        );
    }
}
