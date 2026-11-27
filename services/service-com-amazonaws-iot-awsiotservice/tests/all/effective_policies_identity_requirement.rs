// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what `GetEffectivePolicies` requires of a request before it
//! computes anything, and of the document it projects for a policy whose version in effect
//! has moved.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see.
//!
//! The rules under test:
//!
//! * an IDENTITY is required, and the requirement is read before anything is resolved: a
//!   request carrying neither `principal` nor `cognitoIdentityPoolId` is refused HTTP 400
//!   `InvalidRequestException` whatever `thingName` it names and whether or not that thing
//!   exists;
//! * which FORM the principal takes decides whether `cognitoIdentityPoolId` belongs beside
//!   it — a principal that is no ARN is the Cognito identity form and is refused without
//!   the pool that names it, while an ARN principal is refused WITH one;
//! * `thingName` is resolved against the thing registry, and a name no thing is registered
//!   under is refused HTTP 404 `ResourceNotFoundException` naming that name and nothing
//!   else;
//! * the reported `policyDocument` is the document of the policy's CURRENT default
//!   version, so it follows the pointer whichever operation moved it.
//!
//! Each table spans the class its rule generalises over rather than one request: the
//! identity members in every combination, a thing that exists and one that does not,
//! non-ARN principals of several forms (a bare certificate id, a bare word, a Cognito
//! identity, a value carrying colons), thing names of several lengths and character
//! classes, and both operations that move the default-version pointer.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService as CognitoIdentityService;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// `InvalidRequestException` as this service NAMES it in `x-amzn-errortype`: the shape
/// name, a `:`, and the service's API doc namespace.
const QUALIFIED_INVALID_REQUEST: &str =
    "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// How the operation states that neither identity member was supplied.
const MISSING_IDENTITY: &str = "Both principal and CognitoIdentityPoolId can not be null.";

/// How the operation states that a non-ARN principal came without its pool.
const POOL_REQUIRED: &str =
    "CognitoIdentityPoolId must be provided if your principal is CognitoIdentityId.";

/// How the operation states that an ARN principal came with a pool beside it.
const CONFLICTING_IDENTITY: &str = "Principal must be null if CognitoIdentityPoolId is provided.";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region. The signature itself is never checked here — the harness turns request
/// authentication off, exactly as an unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// An IoT policy document allowing `action` on `resource`, so two documents of one policy
/// differ in exactly the element the case changed.
fn policy_document(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
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

    /// Every entry of an `effectivePolicies` answer as (`policyName`, `policyDocument`).
    fn effective_policies(&self) -> Vec<(String, String)> {
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
                        (member("policyName"), member("policyDocument"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
///
/// Amazon Cognito Identity is mounted beside it — as the assembled emulator mounts them —
/// because an identity POOL is that service's resource: a case naming one creates it there.
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

    /// Amazon Cognito Identity's `CreateIdentityPool`, through the one caller of that API this
    /// suite tree has (`crate::effective_policies::created_identity_pool`).
    async fn create_identity_pool(&self, identity_pool_name: &str) -> String {
        crate::effective_policies::created_identity_pool(&self.router, REGION, identity_pool_name)
            .await
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

    /// `CreatePolicyVersion`: `POST /policies/{policyName}/version`, whose `setAsDefault`
    /// is `@httpQuery`.
    async fn create_policy_version(
        &self,
        policy_name: &str,
        document: &str,
        set_as_default: bool,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}/version?setAsDefault={set_as_default}"),
            Some(&json!({ "policyDocument": document })),
        )
        .await
    }

    /// `SetDefaultPolicyVersion`:
    /// `PATCH /policies/{policyName}/version/{policyVersionId}`.
    async fn set_default_policy_version(&self, policy_name: &str, version_id: &str) -> Reply {
        self.call(
            "PATCH",
            &format!("/policies/{policy_name}/version/{version_id}"),
            None,
        )
        .await
    }

    /// `GetPolicy`: `GET /policies/{policyName}`, which reports the document of the version
    /// in effect.
    async fn get_policy(&self, policy_name: &str) -> Reply {
        self.call("GET", &format!("/policies/{policy_name}"), None)
            .await
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`.
    async fn create_certificate(&self) -> Reply {
        self.call("POST", "/keys-and-certificate?setAsActive=true", None)
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
}

/// Issue a certificate, answering its `certificateArn`.
async fn given_certificate(emulator: &Emulator) -> String {
    let issued = emulator.create_certificate().await;
    assert_eq!(
        issued.status, 200,
        "CreateKeysAndCertificate must answer 200, got {} {:?}",
        issued.status, issued.body
    );
    issued
        .text("certificateArn")
        .expect("an issued certificate carries its ARN")
}

/// Register `thing_name`, asserting the create succeeded.
async fn given_thing(emulator: &Emulator, thing_name: &str) {
    let created = emulator.create_thing(thing_name).await;
    assert_eq!(
        created.status, 200,
        "case {thing_name}: CreateThing must answer 200, got {} {:?}",
        created.status, created.body
    );
}

/// One row of the identity table: what the request carries, and how it is answered.
struct IdentityCase {
    /// What the case is called in an assertion.
    label: &'static str,
    principal: Option<String>,
    cognito_identity_pool_id: Option<String>,
    thing_name: Option<&'static str>,
    /// The status the request is answered with — 200 for one this operation answers.
    status: u16,
    /// The `message` of the refusal, or `None` for a case that is answered.
    message: Option<&'static str>,
}

/// An IDENTITY is required, and the requirement is read BEFORE anything is resolved: with
/// neither identity member the request is refused whatever `thingName` it names and
/// whether or not that thing exists, and the principal's form then decides whether the
/// pool id belongs beside it.
#[tokio::test]
async fn effective_policies_require_an_identity_before_resolving_anything() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator).await;
    given_thing(&emulator, "registered-thing").await;
    // The pool an identity is federated into, created where identity pools live.
    let pool_id = emulator.create_identity_pool("identity-requirement").await;

    let cases = vec![
        IdentityCase {
            label: "no identity, no thing name",
            principal: None,
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 400,
            message: Some(MISSING_IDENTITY),
        },
        IdentityCase {
            label: "no identity, a thing this account HOLDS",
            principal: None,
            cognito_identity_pool_id: None,
            thing_name: Some("registered-thing"),
            status: 400,
            message: Some(MISSING_IDENTITY),
        },
        IdentityCase {
            label: "no identity, a thing this account does NOT hold",
            principal: None,
            cognito_identity_pool_id: None,
            thing_name: Some("unregistered-thing"),
            status: 400,
            message: Some(MISSING_IDENTITY),
        },
        IdentityCase {
            label: "an ARN principal beside a pool id",
            principal: Some(certificate.clone()),
            cognito_identity_pool_id: Some(pool_id.clone()),
            thing_name: None,
            status: 400,
            message: Some(CONFLICTING_IDENTITY),
        },
        IdentityCase {
            label: "an ARN principal alone",
            principal: Some(certificate.clone()),
            cognito_identity_pool_id: None,
            thing_name: None,
            status: 200,
            message: None,
        },
        IdentityCase {
            label: "an ARN principal beside a registered thing",
            principal: Some(certificate.clone()),
            cognito_identity_pool_id: None,
            thing_name: Some("registered-thing"),
            status: 200,
            message: None,
        },
        IdentityCase {
            label: "a pool id alone",
            principal: None,
            cognito_identity_pool_id: Some(pool_id.clone()),
            thing_name: None,
            status: 200,
            message: None,
        },
        IdentityCase {
            label: "a Cognito identity principal beside its pool",
            principal: Some(format!("{REGION}:11112222-3333-4444-5555-666677778888")),
            cognito_identity_pool_id: Some(pool_id.clone()),
            thing_name: None,
            status: 200,
            message: None,
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
        match case.message {
            Some(message) => {
                assert_eq!(
                    answer.error_code.as_deref(),
                    Some(QUALIFIED_INVALID_REQUEST),
                    "case {}: expected {QUALIFIED_INVALID_REQUEST}, got {:?}",
                    case.label,
                    answer.error_code
                );
                assert_eq!(
                    answer.message(),
                    message,
                    "case {}: expected {message:?}, got {:?}",
                    case.label,
                    answer.message()
                );
                assert!(
                    !answer.has_effective_policies(),
                    "case {}: a refusal must carry no effectivePolicies, got {:?}",
                    case.label,
                    answer.body
                );
            }
            None => assert!(
                answer.has_effective_policies(),
                "case {}: an answered request must carry effectivePolicies, got {:?}",
                case.label,
                answer.body
            ),
        }
    }
}

/// A `principal` that is no ARN is the Cognito identity form, whatever else it looks like,
/// so without the pool that names it the request is refused with that fixed sentence —
/// which echoes nothing of the value — and the refusal is read before the `thingName` is
/// resolved.
#[tokio::test]
async fn effective_policies_refuse_a_non_arn_principal_without_its_pool() {
    let emulator = Emulator::new();
    given_thing(&emulator, "registered-thing").await;

    // Non-ARN principal forms: a bare certificate id (the 64-hex `CertificateId` form
    // without its ARN wrapper), a bare word, the `region:id` Cognito identity form the
    // member documents, and a value carrying colons that names no region.
    let principals = [
        ("a bare 64-hex certificate id", "ab".repeat(32)),
        ("a bare word", "not-a-principal".to_string()),
        (
            "the documented Cognito identity form",
            format!("{REGION}:11112222-3333-4444-5555-666677778888"),
        ),
        ("a value carrying colons", "NOT:A:REGION:id".to_string()),
    ];

    // The thing name a row names, if any: the refusal takes precedence over resolving a
    // device whether it exists, does not exist, or was not named at all.
    let devices = [None, Some("registered-thing"), Some("unregistered-thing")];

    for (label, principal) in &principals {
        for thing_name in devices {
            let answer = emulator
                .get_effective_policies(Some(principal), None, thing_name)
                .await;
            assert_eq!(
                answer.status, 400,
                "case {label}/{thing_name:?}: expected HTTP 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some(QUALIFIED_INVALID_REQUEST),
                "case {label}/{thing_name:?}: expected {QUALIFIED_INVALID_REQUEST}, got {:?}",
                answer.error_code
            );
            assert_eq!(
                answer.message(),
                POOL_REQUIRED,
                "case {label}/{thing_name:?}: expected {POOL_REQUIRED:?}, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(principal.as_str()),
                "case {label}/{thing_name:?}: the refusal must echo no value of the request, \
                 got {:?}",
                answer.message()
            );
        }
    }
}

/// A `thingName` an identity names beside itself is resolved against the thing registry: a
/// name no thing is registered under is refused HTTP 404 `ResourceNotFoundException`
/// stating that name and nothing else, and a name this account holds is answered.
#[tokio::test]
async fn effective_policies_refuse_a_thing_name_no_thing_is_registered_under() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    // Thing names spanning the modelled `ThingName` form (`length 1..128`, pattern
    // `^[a-zA-Z0-9:_-]+$`): one character, the longest the model admits, each character
    // class the pattern allows, and a name that looks like another resource's address.
    let absent = [
        "x".to_string(),
        "a".repeat(128),
        "Sensor_7-floor:2".to_string(),
        "0123456789".to_string(),
    ];

    for thing_name in &absent {
        let answer = emulator
            .get_effective_policies(Some(&certificate), None, Some(thing_name))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {thing_name}: expected HTTP 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {thing_name}: expected ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        assert_eq!(
            answer.message(),
            format!("{thing_name} thing doesn't exists"),
            "case {thing_name}: unexpected refusal message, got {:?}",
            answer.message()
        );
    }

    // The same names, once registered, are resolved: the refusal is about the registry and
    // not about the member.
    for thing_name in &absent {
        given_thing(&emulator, thing_name).await;
        let answer = emulator
            .get_effective_policies(Some(&certificate), None, Some(thing_name))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {thing_name} registered: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// One row of the default-version table: how the pointer is moved, and the document that
/// then has an effect.
struct DefaultVersionCase {
    label: &'static str,
    /// The policy the row writes.
    policy_name: &'static str,
    /// The action the first version allows, and the one the second allows: the two
    /// documents differ in exactly that element.
    first_action: &'static str,
    second_action: &'static str,
    /// Whether the second version is made default by the create itself; otherwise the
    /// pointer is moved afterwards by `SetDefaultPolicyVersion`.
    default_on_create: bool,
}

/// The reported `policyDocument` is the document of the policy's CURRENT default version:
/// it moves with the pointer whichever operation moved it, and the superseded version's
/// document is never reported again.
#[tokio::test]
async fn effective_policies_project_the_current_default_version() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    let cases = [
        DefaultVersionCase {
            label: "the create makes the new version default",
            policy_name: "promoted-on-create",
            first_action: "iot:Connect",
            second_action: "iot:Publish",
            default_on_create: true,
        },
        DefaultVersionCase {
            label: "the pointer is moved after the create",
            policy_name: "promoted-afterwards",
            first_action: "iot:Subscribe",
            second_action: "iot:Receive",
            default_on_create: false,
        },
    ];

    for case in &cases {
        let first = policy_document(case.first_action, "*");
        let second = policy_document(case.second_action, "arn:aws:iot:*:*:topic/floor/2");

        let created = emulator.create_policy(case.policy_name, &first).await;
        assert_eq!(
            created.status, 200,
            "case {}: CreatePolicy must answer 200, got {} {:?}",
            case.label, created.status, created.body
        );
        let attached = emulator
            .attach_policy(case.policy_name, &certificate)
            .await;
        assert_eq!(
            attached.status, 200,
            "case {}: AttachPolicy must answer 200, got {} {:?}",
            case.label, attached.status, attached.body
        );

        let before = emulator
            .get_effective_policies(Some(&certificate), None, None)
            .await;
        assert!(
            before
                .effective_policies()
                .contains(&(case.policy_name.to_string(), first.clone())),
            "case {}: expected the first version's document, got {:?}",
            case.label,
            before.effective_policies()
        );

        let version = emulator
            .create_policy_version(case.policy_name, &second, case.default_on_create)
            .await;
        assert_eq!(
            version.status, 200,
            "case {}: CreatePolicyVersion must answer 200, got {} {:?}",
            case.label, version.status, version.body
        );
        let version_id = version
            .text("policyVersionId")
            .expect("a created version carries its id");
        if !case.default_on_create {
            // Until the pointer moves, the version in effect is still the first one.
            let interim = emulator
                .get_effective_policies(Some(&certificate), None, None)
                .await;
            assert!(
                interim
                    .effective_policies()
                    .contains(&(case.policy_name.to_string(), first.clone())),
                "case {}: a non-default version must have no effect, got {:?}",
                case.label,
                interim.effective_policies()
            );
            let promoted = emulator
                .set_default_policy_version(case.policy_name, &version_id)
                .await;
            assert_eq!(
                promoted.status, 200,
                "case {}: SetDefaultPolicyVersion must answer 200, got {} {:?}",
                case.label, promoted.status, promoted.body
            );
        }

        let after = emulator
            .get_effective_policies(Some(&certificate), None, None)
            .await;
        assert!(
            after
                .effective_policies()
                .contains(&(case.policy_name.to_string(), second.clone())),
            "case {}: expected the new default version's document, got {:?}",
            case.label,
            after.effective_policies()
        );
        assert!(
            !after
                .effective_policies()
                .contains(&(case.policy_name.to_string(), first.clone())),
            "case {}: the superseded document must not be reported, got {:?}",
            case.label,
            after.effective_policies()
        );

        // One policy, one document: what this read reports is what `GetPolicy` reports.
        let read = emulator.get_policy(case.policy_name).await;
        assert_eq!(
            read.text("policyDocument").as_deref(),
            Some(second.as_str()),
            "case {}: GetPolicy must report the same document, got {:?}",
            case.label,
            read.body
        );

        // Pointing the policy back at the first version puts that document back in effect,
        // so the projection follows the pointer in both directions.
        let back = emulator
            .set_default_policy_version(case.policy_name, "1")
            .await;
        assert_eq!(
            back.status, 200,
            "case {}: SetDefaultPolicyVersion must answer 200, got {} {:?}",
            case.label, back.status, back.body
        );
        let restored = emulator
            .get_effective_policies(Some(&certificate), None, None)
            .await;
        assert!(
            restored
                .effective_policies()
                .contains(&(case.policy_name.to_string(), first.clone())),
            "case {}: expected the first version's document again, got {:?}",
            case.label,
            restored.effective_policies()
        );
    }
}
