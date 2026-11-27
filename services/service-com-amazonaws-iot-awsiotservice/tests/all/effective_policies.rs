// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `GetEffectivePolicies`: the policies that have an effect
//! on one device's authorization, computed from the persisted policy-attachment relation
//! and the policy registry.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: the answer is the UNION of the policies the attachment relation
//! holds for every target the request's identity resolves to, each policy reported ONCE
//! with the registry's own `policyName`/`policyArn` and the document of the version in
//! effect; a thing group's union adds the groups ABOVE it; the read changes nothing, so
//! the answer tracks the relation as later attaches and detaches move it. An identity of
//! no supported form, an ARN principal with a pool id beside it, a Cognito identity
//! principal without one and no identity at all are refused `InvalidRequestException`
//! (400); a well-formed identity naming a resource this account does not hold is refused
//! `ResourceNotFoundException` (404) — never an empty list standing in for "no such
//! principal".
//!
//! The tables span the class the rule generalises over rather than one recorded request:
//! each of the three principal forms the modelled member documents (a certificate ARN, a
//! thing group ARN and a Cognito identity), a Cognito identity pool id, a principal with
//! no attachment at all, one policy and several, policy names and documents of several
//! forms, and every level of a three-level group hierarchy.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService as CognitoIdentityService;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// `InvalidRequestException` as this service NAMES it in `x-amzn-errortype`: the shape
/// name, a `:`, and the service's API doc namespace.
const QUALIFIED_INVALID_REQUEST: &str =
    "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region: the scope is how the router picks the claiming service, and how the service
/// picks the region its ARNs are scoped to. The signature itself is never checked here —
/// the harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// An IoT policy document allowing `action`: the documents differ row by row, so an answer
/// that reported one policy's document for another would fail the assertion.
fn policy_document(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
    })
    .to_string()
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// Whether the body carries an `effectivePolicies` list at all — present even when it
    /// is empty.
    fn has_effective_policies(&self) -> bool {
        self.body
            .get("effectivePolicies")
            .is_some_and(Value::is_array)
    }

    /// Every entry of an `effectivePolicies` answer as
    /// (`policyName`, `policyArn`, `policyDocument`), in the order it reported them.
    fn effective_policies(&self) -> Vec<(String, String, String)> {
        self.body
            .get("effectivePolicies")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        let member = |name: &str| {
                            entry
                                .get(name)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (
                            member("policyName"),
                            member("policyArn"),
                            member("policyDocument"),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `policyName` of every entry.
    fn policy_names(&self) -> Vec<String> {
        self.effective_policies()
            .into_iter()
            .map(|(name, _arn, _document)| name)
            .collect()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
///
/// Amazon Cognito Identity is mounted beside it — exactly as the assembled emulator mounts
/// them — because an identity POOL is that service's resource: whether the pool a request
/// names exists is its holder's answer, so a case about a pool creates one there.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(CognitoIdentityService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
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
    async fn create_policy(&self, policy_name: &str, document: &str) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(&json!({ "policyDocument": document })),
        )
        .await
    }

    /// `GetPolicy`: `GET /policies/{policyName}`, which reports the document of the
    /// version in effect.
    async fn get_policy(&self, policy_name: &str) -> Reply {
        self.call("GET", &format!("/policies/{policy_name}"), None)
            .await
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, which issues the
    /// certificate a policy is attached to.
    async fn create_certificate(&self, set_as_active: bool) -> Reply {
        self.call(
            "POST",
            &format!("/keys-and-certificate?setAsActive={set_as_active}"),
            None,
        )
        .await
    }

    /// `CreateThingGroup`: `POST /thing-groups/{thingGroupName}`.
    async fn create_thing_group(&self, name: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/thing-groups/{name}"), Some(request))
            .await
    }

    /// `CreateThing`: `POST /things/{thingName}`.
    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await
    }

    /// `AttachPolicy`: `PUT /target-policies/{policyName}` carrying the target.
    async fn attach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
        )
        .await
    }

    /// `DetachPolicy`: `POST /target-policies/{policyName}` carrying the target.
    async fn detach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "POST",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
        )
        .await
    }

    /// `GetEffectivePolicies`: `POST /effective-policies`, whose identities travel in the
    /// body and whose `thingName` is `@httpQuery`.
    async fn get_effective_policies(
        &self,
        principal: Option<&str>,
        cognito_identity_pool_id: Option<&str>,
        thing_name: Option<&str>,
    ) -> Reply {
        let mut request = serde_json::Map::new();
        if let Some(principal) = principal {
            request.insert("principal".to_string(), json!(principal));
        }
        if let Some(pool_id) = cognito_identity_pool_id {
            request.insert("cognitoIdentityPoolId".to_string(), json!(pool_id));
        }
        let path = match thing_name {
            Some(thing_name) => format!("/effective-policies?thingName={thing_name}"),
            None => "/effective-policies".to_string(),
        };
        self.call("POST", &path, Some(&Value::Object(request))).await
    }

    /// `ListAttachedPolicies`: `POST /attached-policies/{target}`, whose target is ONE URI
    /// path segment. Read here only to prove `GetEffectivePolicies` left the relation
    /// alone.
    async fn list_attached_policies(&self, target: &str) -> Reply {
        let segment = target
            .replace('%', "%25")
            .replace(':', "%3A")
            .replace('/', "%2F");
        self.call("POST", &format!("/attached-policies/{segment}"), None)
            .await
    }

    /// Amazon Cognito Identity's `CreateIdentityPool`, through the one caller of that API
    /// this suite tree has ([`created_identity_pool`]).
    async fn create_identity_pool(&self, identity_pool_name: &str) -> String {
        created_identity_pool(&self.router, REGION, identity_pool_name).await
    }
}

/// Amazon Cognito Identity's `CreateIdentityPool`, as a client calls it: `POST /` with
/// `X-Amz-Target` on `awsJson1_1`, signed for the `cognito-identity` scope that routes it to
/// the holder of identity pools. Answers the `IdentityPoolId` that service minted.
///
/// Shared with every sibling suite that needs a pool to exist — `automod` declares each suite
/// as a sibling module — so the one call that creates one is written once.
pub(crate) async fn created_identity_pool(
    router: &MultiServiceRouter,
    region: &str,
    identity_pool_name: &str,
) -> String {
    let body = json!({
        "IdentityPoolName": identity_pool_name,
        "AllowUnauthenticatedIdentities": true,
    });
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cognito-identity.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            "AWSCognitoIdentityService.CreateIdentityPool",
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTESTIOT/20260101/{region}/cognito-identity/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("a request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {identity_pool_name:?}: CreateIdentityPool must answer 200, got {status} \
         {answered:?}"
    );
    answered
        .get("IdentityPoolId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {identity_pool_name:?}: no IdentityPoolId in {answered:?}"))
        .to_string()
}

/// The ARN a policy created in this suite's region carries.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:policy/{policy_name}")
}

/// The ARN of a certificate in this suite's region, for a certificate id this account does
/// not hold.
fn certificate_arn(certificate_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert/{certificate_id}")
}

/// The ARN of a thing group in this suite's region.
fn thing_group_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/{name}")
}

/// Create `policy_name` with a document allowing `action`, asserting the create succeeded,
/// and answer the document it was created with.
async fn given_policy(emulator: &Emulator, policy_name: &str, action: &str) -> String {
    let document = policy_document(action);
    let created = emulator.create_policy(policy_name, &document).await;
    assert_eq!(
        created.status, 200,
        "case {policy_name:?}: CreatePolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    document
}

/// Issue a certificate, answering its `certificateArn`.
async fn given_certificate(emulator: &Emulator, set_as_active: bool) -> String {
    let issued = emulator.create_certificate(set_as_active).await;
    assert_eq!(
        issued.status, 200,
        "case setAsActive={set_as_active}: CreateKeysAndCertificate must answer 200, got \
         {} {:?}",
        issued.status, issued.body
    );
    issued
        .text("certificateArn")
        .unwrap_or_else(|| panic!("case setAsActive={set_as_active}: no certificateArn"))
}

/// Create a thing group, answering its `thingGroupArn`.
async fn given_thing_group(emulator: &Emulator, name: &str, request: &Value) -> String {
    let created = emulator.create_thing_group(name, request).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateThingGroup must answer 200, got {} {:?}",
        created.status, created.body
    );
    created
        .text("thingGroupArn")
        .unwrap_or_else(|| panic!("case {name:?}: no thingGroupArn"))
}

/// Attach `policy_name` to `target`, asserting the attach succeeded.
async fn given_attachment(emulator: &Emulator, policy_name: &str, target: &str) {
    let attached = emulator.attach_policy(policy_name, target).await;
    assert_eq!(
        attached.status, 200,
        "case {policy_name:?}->{target:?}: AttachPolicy must answer 200, got {} {:?}",
        attached.status, attached.body
    );
}

/// One row of the principal table: an identity, and the policies attached to it.
struct PrincipalCase {
    /// What the case is called in an assertion.
    label: &'static str,
    /// The identity the read asks about.
    principal: String,
    /// The pool the request names beside the principal: present exactly for the Cognito
    /// identity form, which the operation answers only with the pool that names it.
    pool_id: Option<String>,
    /// `(policyName, action)` of every policy attached to it — none, one or several.
    policies: Vec<(&'static str, &'static str)>,
}

/// The read reports exactly the policies the relation holds for the identity, each with
/// the registry's own name and ARN and the document of the version in effect.
#[tokio::test]
async fn effective_policies_are_the_policies_attached_to_the_identity() {
    let emulator = Emulator::new();

    let active_certificate = given_certificate(&emulator, true).await;
    let inactive_certificate = given_certificate(&emulator, false).await;
    let bare_certificate = given_certificate(&emulator, true).await;
    let group = given_thing_group(&emulator, "plant-floor", &json!({})).await;
    // The pool a Cognito identity is federated into, created where identity pools live.
    let pool_id = emulator.create_identity_pool("plant-floor-pool").await;

    // The principal forms `GetEffectivePolicies$principal` documents: "Valid principals
    // are CertificateArn (arn:aws:iot:region:accountId:cert/certificateId), thingGroupArn
    // (arn:aws:iot:region:accountId:thinggroup/groupName) and CognitoId (region:id)".
    let cases = vec![
        PrincipalCase {
            label: "one policy on an ACTIVE certificate",
            principal: active_certificate.clone(),
            pool_id: None,
            policies: vec![("connect", "iot:Connect")],
        },
        PrincipalCase {
            label: "three policies on an INACTIVE certificate",
            principal: inactive_certificate.clone(),
            pool_id: None,
            policies: vec![
                ("publish", "iot:Publish"),
                ("subscribe", "iot:Subscribe"),
                ("receive.messages", "iot:Receive"),
            ],
        },
        PrincipalCase {
            label: "a thing group principal",
            principal: group.clone(),
            pool_id: None,
            policies: vec![("floor+policy=1", "iot:Connect")],
        },
        PrincipalCase {
            label: "a Cognito identity principal beside its pool",
            principal: format!("{REGION}:11112222-3333-4444-5555-666677778888"),
            pool_id: Some(pool_id.clone()),
            policies: vec![("cognito-connect", "iot:Connect"), ("cognito-publish", "iot:Publish")],
        },
        PrincipalCase {
            label: "a certificate with no attachment at all",
            principal: bare_certificate.clone(),
            pool_id: None,
            policies: Vec::new(),
        },
    ];

    for case in &cases {
        for (policy_name, action) in &case.policies {
            given_policy(&emulator, policy_name, action).await;
            given_attachment(&emulator, policy_name, &case.principal).await;
        }
    }

    for case in &cases {
        let answer = emulator
            .get_effective_policies(Some(&case.principal), case.pool_id.as_deref(), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {}: GetEffectivePolicies must answer 200, got {} {:?}",
            case.label, answer.status, answer.body
        );
        assert!(
            answer.has_effective_policies(),
            "case {}: effectivePolicies must be present even when empty, got {:?}",
            case.label,
            answer.body
        );

        let reported = answer.effective_policies();
        let mut expected: Vec<(String, String, String)> = case
            .policies
            .iter()
            .map(|(policy_name, action)| {
                (
                    (*policy_name).to_string(),
                    policy_arn(policy_name),
                    policy_document(action),
                )
            })
            .collect();
        expected.sort();
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case {}: expected {expected:?}, got {reported:?}",
            case.label
        );

        // One policy, one document: what this read reports is what `GetPolicy` reports for
        // the same policy.
        for (policy_name, _arn, document) in &reported {
            let read = emulator.get_policy(policy_name).await;
            assert_eq!(
                read.text("policyDocument").as_deref(),
                Some(document.as_str()),
                "case {}/{policy_name}: GetPolicy must report the same document, got {:?}",
                case.label,
                read.body
            );
        }
    }
}

/// A thing group's effective policies add the policies attached to every group ABOVE it,
/// and a policy that governs through two paths is reported ONCE.
#[tokio::test]
async fn effective_policies_add_the_policies_of_the_groups_above() {
    let emulator = Emulator::new();

    // A three-level hierarchy: root -> middle -> leaf, one policy attached at each level,
    // so the answer differs at every level of the walk.
    let root = given_thing_group(&emulator, "root", &json!({})).await;
    let middle = given_thing_group(&emulator, "middle", &json!({ "parentGroupName": "root" })).await;
    let leaf = given_thing_group(&emulator, "leaf", &json!({ "parentGroupName": "middle" })).await;

    for (policy_name, target) in [
        ("root-policy", &root),
        ("middle-policy", &middle),
        ("leaf-policy", &leaf),
    ] {
        given_policy(&emulator, policy_name, "iot:Connect").await;
        given_attachment(&emulator, policy_name, target).await;
    }

    // One policy attached at two levels of the same chain: it has ONE effect, so the
    // answer carries one entry for it.
    given_policy(&emulator, "shared-policy", "iot:Publish").await;
    given_attachment(&emulator, "shared-policy", &root).await;
    given_attachment(&emulator, "shared-policy", &leaf).await;

    let cases = [
        ("the root group", &root, vec!["root-policy", "shared-policy"]),
        (
            "the middle group",
            &middle,
            vec!["middle-policy", "root-policy", "shared-policy"],
        ),
        (
            "the leaf group",
            &leaf,
            vec![
                "leaf-policy",
                "middle-policy",
                "root-policy",
                "shared-policy",
            ],
        ),
    ];

    for (label, principal, expected) in cases {
        let answer = emulator
            .get_effective_policies(Some(principal), None, None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label}: GetEffectivePolicies must answer 200, got {} {:?}",
            answer.status, answer.body
        );
        let reported = answer.policy_names();
        let mut sorted = reported.clone();
        sorted.sort();
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            sorted, expected,
            "case {label}: expected {expected:?}, got {reported:?}"
        );
        let deduped: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            reported.len(),
            deduped.len(),
            "case {label}: a policy that governs twice must be reported once, got \
             {reported:?}"
        );
    }
}

/// The answer is COMPUTED from the relation on every call: a later attach adds a policy, a
/// detach removes it, and the read itself changes nothing a later caller sees.
#[tokio::test]
async fn effective_policies_track_the_relation_and_change_nothing() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    given_policy(&emulator, "connect", "iot:Connect").await;
    given_policy(&emulator, "publish", "iot:Publish").await;

    let empty = emulator
        .get_effective_policies(Some(&certificate), None, None)
        .await;
    assert_eq!(
        empty.policy_names(),
        Vec::<String>::new(),
        "case before any attach: nothing governs the certificate, got {:?}",
        empty.body
    );

    given_attachment(&emulator, "connect", &certificate).await;
    given_attachment(&emulator, "publish", &certificate).await;
    let both = emulator
        .get_effective_policies(Some(&certificate), None, None)
        .await;
    assert_eq!(
        both.policy_names(),
        vec!["connect".to_string(), "publish".to_string()],
        "case after both attaches: expected both policies, got {:?}",
        both.body
    );

    // Two reads of an unchanged relation answer identically, and the relation the two
    // listings read is untouched by them: a read is a read.
    let again = emulator
        .get_effective_policies(Some(&certificate), None, None)
        .await;
    assert_eq!(
        again.effective_policies(),
        both.effective_policies(),
        "case a repeated read: the answer must not move, got {:?}",
        again.body
    );
    let listed = emulator.list_attached_policies(&certificate).await;
    assert_eq!(
        listed.status, 200,
        "case the relation after reads: ListAttachedPolicies must answer 200, got {} {:?}",
        listed.status, listed.body
    );

    let detached = emulator.detach_policy("publish", &certificate).await;
    assert_eq!(
        detached.status, 200,
        "case the detach: DetachPolicy must answer 200, got {} {:?}",
        detached.status, detached.body
    );
    let after = emulator
        .get_effective_policies(Some(&certificate), None, None)
        .await;
    assert_eq!(
        after.policy_names(),
        vec!["connect".to_string()],
        "case after the detach: the detached policy must go, got {:?}",
        after.body
    );
}

/// A Cognito identity pool is resolved against its HOLDER — identity pools live in Amazon
/// Cognito Identity — so the policies attached to a pool that exists are its effective
/// policies, and a pool nothing holds is refused as missing.
#[tokio::test]
async fn effective_policies_of_a_cognito_identity_pool_are_its_attachments() {
    let emulator = Emulator::new();

    let pool_id = emulator.create_identity_pool("governed-pool").await;
    given_policy(&emulator, "pool-connect", "iot:Connect").await;
    given_attachment(&emulator, "pool-connect", &pool_id).await;

    let answer = emulator
        .get_effective_policies(None, Some(&pool_id), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case a pool with one policy: GetEffectivePolicies must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.effective_policies(),
        vec![(
            "pool-connect".to_string(),
            policy_arn("pool-connect"),
            policy_document("iot:Connect"),
        )],
        "case a pool with one policy: expected the attached policy, got {:?}",
        answer.body
    );

    // A pool that EXISTS and has nothing attached governs nothing, which is an empty list
    // and not a refusal: "nothing governs it" is an answer.
    let ungoverned = emulator.create_identity_pool("ungoverned-pool").await;
    let none = emulator
        .get_effective_policies(None, Some(&ungoverned), None)
        .await;
    assert_eq!(
        none.status, 200,
        "case a pool with no policy: GetEffectivePolicies must answer 200, got {} {:?}",
        none.status, none.body
    );
    assert_eq!(
        none.policy_names(),
        Vec::<String>::new(),
        "case a pool with no policy: expected an empty list, got {:?}",
        none.body
    );
}

/// A `thingName` is resolved against the thing registry, and a device named beside a
/// principal does not change what that principal's attachments are.
#[tokio::test]
async fn effective_policies_resolve_the_thing_the_question_is_about() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    given_policy(&emulator, "connect", "iot:Connect").await;
    given_attachment(&emulator, "connect", &certificate).await;
    let created = emulator.create_thing("sensor-7").await;
    assert_eq!(
        created.status, 200,
        "case the thing: CreateThing must answer 200, got {} {:?}",
        created.status, created.body
    );

    let with_thing = emulator
        .get_effective_policies(Some(&certificate), None, Some("sensor-7"))
        .await;
    let without_thing = emulator
        .get_effective_policies(Some(&certificate), None, None)
        .await;
    assert_eq!(
        with_thing.status, 200,
        "case principal+thingName: GetEffectivePolicies must answer 200, got {} {:?}",
        with_thing.status, with_thing.body
    );
    assert_eq!(
        with_thing.effective_policies(),
        without_thing.effective_policies(),
        "case principal+thingName: the principal's attachments must still be reported, \
         got {:?}",
        with_thing.body
    );

    // A registered thing on its own names NO identity, so it is not a question this
    // operation answers however well the device resolves.
    let thing_only = emulator
        .get_effective_policies(None, None, Some("sensor-7"))
        .await;
    assert_eq!(
        thing_only.status, 400,
        "case thingName alone: GetEffectivePolicies must refuse with HTTP 400, got {} {:?}",
        thing_only.status, thing_only.body
    );
    assert_eq!(
        thing_only.message(),
        "Both principal and CognitoIdentityPoolId can not be null.",
        "case thingName alone: unexpected refusal message, got {:?}",
        thing_only.body
    );
}

/// One row of the refusal table: a request, the status and error code it is refused with,
/// and the message that refusal states.
struct RefusalCase {
    label: &'static str,
    principal: Option<String>,
    cognito_identity_pool_id: Option<String>,
    thing_name: Option<&'static str>,
    status: u16,
    error_code: &'static str,
    message: String,
}

/// A request naming no identity, two conflicting identities, an identity of no supported
/// form, or a well-formed identity naming a resource this account does not hold is REFUSED
/// — never answered with an empty list.
#[tokio::test]
async fn effective_policies_refuse_a_request_that_names_no_resolvable_device() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    given_thing_group(&emulator, "known-group", &json!({})).await;
    let pool_id = format!("{REGION}:11112222-3333-4444-5555-666677778888");

    // A certificate id this account holds no certificate under: 64 hex characters, the
    // modelled `CertificateId` form (`length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`).
    let unknown_certificate = certificate_arn(&"ab".repeat(32));
    let unknown_group = thing_group_arn("ghost-group");

    let cases = vec![
        RefusalCase {
            label: "both identities",
            principal: Some(certificate.clone()),
            cognito_identity_pool_id: Some(pool_id.clone()),
            thing_name: None,
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            message: "Principal must be null if CognitoIdentityPoolId is provided.".to_string(),
        },
        RefusalCase {
            label: "both identities beside a thing name",
            principal: Some(certificate.clone()),
            cognito_identity_pool_id: Some(pool_id.clone()),
            thing_name: Some("sensor-7"),
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            message: "Principal must be null if CognitoIdentityPoolId is provided.".to_string(),
        },
        RefusalCase {
            label: "no identity and no device",
            principal: None,
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            message: "Both principal and CognitoIdentityPoolId can not be null.".to_string(),
        },
        RefusalCase {
            label: "a device named with no identity beside it",
            principal: None,
            cognito_identity_pool_id: None,
            thing_name: Some("ghost-thing"),
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            message: "Both principal and CognitoIdentityPoolId can not be null.".to_string(),
        },
        RefusalCase {
            label: "a principal that is no ARN, with no pool beside it",
            principal: Some("not-a-principal".to_string()),
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            // No ARN was read out of the value, so it is the Cognito identity form, which
            // the pool that names it has to come with.
            message: "CognitoIdentityPoolId must be provided if your principal is \
                      CognitoIdentityId."
                .to_string(),
        },
        RefusalCase {
            label: "a principal naming an IoT resource of another type",
            principal: Some(format!(
                "arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/sensor-7"
            )),
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            message: format!("Invalid Target: arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/sensor-7"),
        },
        RefusalCase {
            label: "a cognitoIdentityPoolId of no supported form",
            principal: None,
            cognito_identity_pool_id: Some("not-a-pool".to_string()),
            thing_name: None,
            status: 400,
            error_code: QUALIFIED_INVALID_REQUEST,
            message: "Invalid CognitoIdentityPoolId: not-a-pool. A Cognito identity pool id \
                      has the form region:id"
                .to_string(),
        },
        RefusalCase {
            label: "a certificate this account does not hold",
            principal: Some(unknown_certificate.clone()),
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 404,
            error_code: "ResourceNotFoundException",
            message: "The certificate given in the principal does not exist.".to_string(),
        },
        RefusalCase {
            label: "a thing group this account does not hold",
            principal: Some(unknown_group.clone()),
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 404,
            error_code: "ResourceNotFoundException",
            message: format!("ThingGroup {ACCOUNT_ID}:ghost-group not found."),
        },
        RefusalCase {
            label: "a thing name no thing is registered under",
            principal: Some(certificate.clone()),
            cognito_identity_pool_id: None,
            thing_name: Some("ghost-thing"),
            status: 404,
            error_code: "ResourceNotFoundException",
            message: "ghost-thing thing doesn't exists".to_string(),
        },
    ];

    for case in &cases {
        let answer = emulator
            .get_effective_policies(
                case.principal.as_deref(),
                case.cognito_identity_pool_id.as_deref(),
                case.thing_name,
            )
            .await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected HTTP {}, got {} {:?}",
            case.label, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(case.error_code),
            "case {}: expected {}, got {:?}",
            case.label,
            case.error_code,
            answer.error_code
        );
        assert_eq!(
            answer.message(),
            case.message,
            "case {}: expected {:?}, got {:?}",
            case.label,
            case.message,
            answer.message()
        );
        assert!(
            !answer.has_effective_policies(),
            "case {}: a refusal must carry no effectivePolicies, got {:?}",
            case.label,
            answer.body
        );
    }
}
