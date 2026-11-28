// SPDX-License-Identifier: CC0-1.0

//! What `InitializeService` ESTABLISHES, end to end over HTTP, with the service that OWNS
//! those resources mounted beside this one.
//!
//! The rule under test: initializing an account is the creation of the IAM resources this
//! service's own machinery runs under — its service-linked role, and the instance profiles
//! its replication and conversion servers run under — so `InitializeService` establishes
//! them in the account's IAM and records the account as initialized only once IAM reports
//! every one of them; an attempt IAM refuses records nothing and is answered with the
//! modelled `ValidationException`: HTTP 400, the `x-amzn-errortype` header naming that
//! shape, a body carrying `__type`, the modelled `reason`, and the sentence whose variable
//! part names WHICH of those resources could not be created.
//!
//! The body's FRAMING is the transport's: a response that carries content keeps the body
//! the protocol rendered and the length the HTTP writer states for it, so the assertions
//! below check that the refusal carries a body and that any length the front door states
//! for it is that body's own — the same division this crate's `empty_output_success_status`
//! suite notes, where the no-content answer is the one a service frames itself.
//!
//! The rows differ along what the rule generalises over: which kinds of resource IAM can
//! hold (both, neither, only the roles, only the profiles) and therefore which of them the
//! attempt establishes — the recorded AWS answer, an initialization that established
//! neither, is one row. The obstructions are preconditions no sequence of calls reaches
//! cheaply (an IAM store already at one of its own quotas), so they are seeded straight
//! into IAM's store exactly as this crate's other suites seed a full store of their own.
//!
//! Every assertion goes through the router's `dispatch`, for both services, so it
//! exercises the same deserialize → handle → serialize path a client drives and reads the
//! established resources back through the API of the service that owns them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    InstanceProfile, Role,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    SERVER_INSTANCE_PROFILES, SERVICE_PRINCIPAL,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test requests sign for.
const REGION: &str = "us-west-2";

/// The modelled error an initialization that could not establish what it needs is
/// answered with, and the status the model states for it
/// (`smithy_query(query="shape",shape="ValidationException")`: `errorKind` client,
/// `httpError` 400).
const VALIDATION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` value such a failure carries. Source:
/// `smithy_query(query="shape",shape="ValidationExceptionReason")`, whose declared values
/// are `unknownOperation`, `cannotParse`, `fieldValidationFailed` and this one — the one
/// for a failure that is about neither the operation, nor parsing, nor a field.
const REASON_OTHER: &str = "other";

/// The account gate every other operation answers while the account is not initialized.
const UNINITIALIZED: &str = "UninitializedAccountException";

/// One answer, as the wire carries it.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: Option<String>,
    content_length: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    /// The body as JSON (`Value::Null` for an empty one).
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!("the answer's body is not JSON ({error}): {:?}", self.text())
        })
    }

    /// The body as text, for a message naming what actually arrived.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One emulator holding BOTH services on one engine: this service, and the IAM service
/// that owns the resources an initialization establishes.
pub(crate) struct Emulator {
    /// The router both services are mounted on, shared with the sibling suite that SERVES
    /// it on a socket to read the bytes a client really gets.
    pub(crate) router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mgn_applicationmigrationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// `POST /<operation>` against this service, signed for [`REGION`].
    async fn post(&self, operation: &str, body: &Value) -> Answer {
        self.post_signed_for(REGION, operation, body).await
    }

    /// [`Emulator::post`], with the Region the request's SigV4 credential scope names
    /// stated per call: the only place a request carries the Region it addressed, and the
    /// one knob a row about a caller-supplied Region turns.
    async fn post_signed_for(&self, region: &str, operation: &str, body: &Value) -> Answer {
        let payload = serde_json::to_vec(body).expect("the request body serializes");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://mgn.{REGION}.amazonaws.com/{operation}"
            ))
            .header("host", format!("mgn.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/{region}/mgn/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string)
        };
        let error_type = header("x-amzn-errortype");
        let content_length = header("content-length");
        Answer {
            status,
            error_type,
            content_length,
            body: response.into_body().to_vec(),
        }
    }

    /// `InitializeService` for the caller's own account.
    async fn initialize(&self) -> Answer {
        self.post("InitializeService", &json!({})).await
    }

    /// One awsQuery call against the IAM service beside this one, answered with the status
    /// and the XML body.
    async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
        (status, body)
    }

    /// The roles IAM reports under the path it files this service's service-linked role
    /// under, by ARN — the question that needs no role name, since the name is IAM's to
    /// pick.
    async fn service_linked_roles(&self) -> Vec<String> {
        let path = iam::service_linked_role_path(SERVICE_PRINCIPAL);
        let (status, body) = self.iam("ListRoles", &[("PathPrefix", &path)]).await;
        assert_eq!(
            status, 200,
            "ListRoles under {path:?} expected 200, got {status} with {body}"
        );
        elements(&body, "Arn")
    }

    /// The roles IAM reports the instance profile `name` as holding, by name — empty when
    /// it holds none, and `None` when IAM holds no such profile.
    async fn instance_profile_roles(&self, name: &str) -> Option<Vec<String>> {
        let (status, body) = self
            .iam("GetInstanceProfile", &[("InstanceProfileName", name)])
            .await;
        if status == 404 {
            return None;
        }
        assert_eq!(
            status, 200,
            "GetInstanceProfile {name:?} expected 200 or 404, got {status} with {body}"
        );
        Some(elements(&body, "RoleName"))
    }

    /// IAM's own store, for seeding a precondition no sequence of calls reaches cheaply:
    /// an account already at one of IAM's quotas.
    fn iam_store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
    > {
        iam::service_state(&*self.engine).expect("IAM has a store in this engine")
    }

    /// Fill the account's IAM roles to [`iam::MAX_ROLES_PER_ACCOUNT`], so IAM refuses
    /// every further role — the quota its own `CreateRole` and `CreateServiceLinkedRole`
    /// refuse past. `held` names roles to put among them, for a profile that must hold
    /// one.
    pub(crate) fn fill_role_quota(&self, held: &[&str]) {
        let store = self.iam_store();
        let mut store = store.lock();
        // The account already holds the roles AWS created WITH it — the service-linked role
        // of each of its account-wide services — and the quota counts every role the account
        // holds, so the fill tops it up TO the quota rather than adding that many beside
        // them. `held` is seeded first, so a profile that must hold one of those roles still
        // finds it.
        for (index, name) in held
            .iter()
            .map(|name| (*name).to_string())
            .chain((0..iam::MAX_ROLES_PER_ACCOUNT).map(|index| format!("filler-role-{index}")))
            .enumerate()
        {
            if store.roles.len() >= iam::MAX_ROLES_PER_ACCOUNT {
                break;
            }
            store
                .roles
                .insert(iam::name_key(&name), seeded_role(&name, index));
        }
        assert_eq!(
            store.roles.len(),
            iam::MAX_ROLES_PER_ACCOUNT,
            "the seeded account must sit exactly at IAM's role quota"
        );
    }

    /// Fill the account's IAM instance profiles to
    /// [`iam::MAX_INSTANCE_PROFILES_PER_ACCOUNT`], so IAM refuses every further profile.
    pub(crate) fn fill_instance_profile_quota(&self) {
        let store = self.iam_store();
        let mut store = store.lock();
        for index in 0..iam::MAX_INSTANCE_PROFILES_PER_ACCOUNT {
            let name = format!("filler-profile-{index}");
            store
                .instance_profiles
                .insert(iam::name_key(&name), seeded_instance_profile(&name, index, &[]));
        }
        assert_eq!(
            store.instance_profiles.len(),
            iam::MAX_INSTANCE_PROFILES_PER_ACCOUNT,
            "the seeded account must sit exactly at IAM's instance profile quota"
        );
    }

    /// Empty the account's IAM instance profiles, so what an initialization could not
    /// establish becomes establishable.
    fn clear_instance_profiles(&self) {
        let store = self.iam_store();
        store.lock().instance_profiles.clear();
    }

    /// Put an instance profile holding the role of the same name into IAM's store, as if
    /// an earlier initialization had established it.
    pub(crate) fn seed_instance_profiles(&self, names: &[&str]) {
        let store = self.iam_store();
        let mut store = store.lock();
        for (index, name) in names.iter().enumerate() {
            let held = vec![iam::name_key(name)];
            store.instance_profiles.insert(
                iam::name_key(name),
                seeded_instance_profile(name, index, &held),
            );
        }
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only
/// RFC 3986's unreserved characters literal.
///
/// Shared with the sibling suites that drive IAM's awsQuery wire, so one encoding rule
/// serves every one of them (`request_authorization.rs`).
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

/// The text of every `<tag>` element of an awsQuery XML answer.
///
/// Shared with the sibling suites that read one back, for the same reason
/// [`form_encode`] is.
pub(crate) fn elements(body: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

/// One seeded IAM role: everything the modelled shape carries, with nothing optional set.
fn seeded_role(name: &str, index: usize) -> Role {
    Role {
        path: "/".to_string(),
        role_name: name.to_string(),
        role_id: format!("AROASEEDEDROLE{index:07}"),
        arn: iam::role_arn("/", name),
        create_date: ::aws_smithy_types::DateTime::from_secs(0),
        assume_role_policy_document: None,
        description: None,
        max_session_duration: None,
        permissions_boundary: None,
        tags: None,
        role_last_used: None,
        source_role_template: None,
    }
}

/// One seeded IAM instance profile, holding the roles `held` names by store key.
fn seeded_instance_profile(
    name: &str,
    index: usize,
    held: &[String],
) -> iam::InstanceProfileRecord {
    iam::InstanceProfileRecord {
        profile: InstanceProfile {
            path: "/".to_string(),
            instance_profile_name: name.to_string(),
            instance_profile_id: format!("AIPASEEDEDPROF{index:07}"),
            arn: iam::instance_profile_arn("/", name),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            // The binding is `role_keys`; every reader fills this member from it.
            roles: Vec::new(),
            tags: None,
        },
        role_keys: held.to_vec(),
    }
}

/// The `__type` of an answer's error body, which restJson1 resolves the error shape from.
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

/// Assert that `answer` is the modelled refusal carrying `expected_message`: the status,
/// the error-type header, the body's own `__type`, the modelled reason, the sentence, and
/// a body whose length any stated framing tells the truth about.
fn assert_refusal(case: &str, answer: &Answer, expected_message: &str) {
    let body = answer.json();
    assert_eq!(
        answer.status, VALIDATION_STATUS,
        "case {case:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(VALIDATION),
        "case {case:?}: expected x-amzn-errortype {VALIDATION}, got {:?}",
        answer.error_type
    );
    assert_eq!(
        error_code(&body),
        VALIDATION,
        "case {case:?}: expected __type {VALIDATION}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected_message),
        "case {case:?}: expected message {expected_message:?}, got {:?}",
        body["message"]
    );
    assert_eq!(
        body["reason"].as_str(),
        Some(REASON_OTHER),
        "case {case:?}: expected reason {REASON_OTHER:?}, got {:?}",
        body["reason"]
    );
    assert!(
        !answer.body.is_empty(),
        "case {case:?}: a refusal must carry its error body, got none"
    );
    assert!(
        answer
            .content_length
            .as_deref()
            .is_none_or(|stated| stated == answer.body.len().to_string()),
        "case {case:?}: a stated content-length must be the error body's {} bytes, got {:?}",
        answer.body.len(),
        answer.content_length
    );
}

/// Assert the account was NOT initialized: an operation whose model declares the account
/// gate still answers it.
async fn assert_uninitialized(case: &str, emulator: &Emulator) {
    let answer = emulator
        .post("CreateApplication", &json!({"name": "after-a-failed-init"}))
        .await;
    let body = answer.json();
    assert_eq!(
        (answer.status, error_code(&body)),
        (VALIDATION_STATUS, UNINITIALIZED),
        "case {case:?}: a refused initialization must leave the account uninitialized, got \
         {} with {body:?}",
        answer.status
    );
}

/// A successful initialization establishes the account's resources in IAM — read back
/// through IAM's own API — and is idempotent: a repeat establishes nothing further and
/// still answers 204.
#[tokio::test]
async fn initialization_establishes_the_accounts_iam_resources_and_is_idempotent() {
    let emulator = Emulator::new();

    let answer = emulator.initialize().await;
    assert_eq!(
        answer.status,
        204,
        "InitializeService expected 204, got {} with {:?}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.body.is_empty(),
        "the modelled 204 answer carries no body, got {:?}",
        answer.text()
    );

    // The service-linked role is IAM's record, reported by IAM's own read.
    let roles = emulator.service_linked_roles().await;
    assert_eq!(
        roles.len(),
        1,
        "IAM must report exactly one role under this service's service-linked role path, \
         got {roles:?}"
    );

    // Each instance profile an initialized account holds exists AND holds its role, so an
    // instance launched with it has something to assume.
    for name in SERVER_INSTANCE_PROFILES {
        let held = emulator.instance_profile_roles(name).await;
        assert_eq!(
            held.as_deref(),
            Some([(*name).to_string()].as_slice()),
            "IAM must report instance profile {name:?} holding the role of that name, got \
             {held:?}"
        );
    }

    // The account is initialized: a gated operation now serves.
    let created = emulator
        .post("CreateApplication", &json!({"name": "initialized-account"}))
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateApplication expected 201 once the account is initialized, got {} with {:?}",
        created.status,
        created.text()
    );

    // A repeat changes nothing: the same 204, and no second role or profile.
    let repeat = emulator.initialize().await;
    assert_eq!(
        repeat.status,
        204,
        "the repeated InitializeService expected 204, got {} with {:?}",
        repeat.status,
        repeat.text()
    );
    let roles_again = emulator.service_linked_roles().await;
    assert_eq!(
        roles_again, roles,
        "a repeated initialization must establish no second service-linked role, got \
         {roles_again:?} after {roles:?}"
    );
}

/// The Region the request's credential scope names does not decide what an
/// initialization establishes: IAM is partition-global, so a scope naming no Region
/// label at all still establishes the account's resources rather than concluding a
/// success that established nothing.
#[tokio::test]
async fn initialization_establishes_them_whatever_region_the_request_names() {
    // Rows differ along the FORM of the caller-supplied Region: another Region, a value
    // carrying delimiters that may not reach an endpoint authority, an over-long label,
    // and an empty scope.
    for region in ["eu-central-1", "us-west-2/../iam", &"r".repeat(64), ""] {
        let emulator = Emulator::new();
        let answer = emulator
            .post_signed_for(region, "InitializeService", &json!({}))
            .await;
        assert_eq!(
            answer.status,
            204,
            "case {region:?}: InitializeService expected 204, got {} with {:?}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            emulator.service_linked_roles().await.len(),
            1,
            "case {region:?}: the service-linked role must be established"
        );
        for name in SERVER_INSTANCE_PROFILES {
            let held = emulator.instance_profile_roles(name).await;
            assert_eq!(
                held.as_deref(),
                Some([(*name).to_string()].as_slice()),
                "case {region:?}: instance profile {name:?} must hold its role, got {held:?}"
            );
        }
    }
}

/// Which kinds of resource IAM cannot hold, as a precondition seeded into its store.
#[derive(Debug, Clone, Copy)]
enum Obstruction {
    /// The account is at IAM's role quota, so neither the service-linked role nor the
    /// role an instance profile must hold can be created.
    RoleQuotaReached,
    /// The account is at IAM's instance profile quota, so no profile can be created while
    /// the service-linked role still can.
    InstanceProfileQuotaReached,
    /// The account is at IAM's role quota, and already holds the server instance profiles
    /// an earlier initialization established — so only the service-linked role is left to
    /// create, and cannot be.
    RoleQuotaReachedWithProfilesHeld,
}

/// An initialization IAM refuses answers the modelled `ValidationException`, whose
/// sentence names which resources could not be created, and records nothing.
#[tokio::test]
async fn an_initialization_iam_refuses_answers_the_modelled_validation_exception() {
    let cases: [(&str, Obstruction, &str); 3] = [
        (
            "neither kind can be created",
            Obstruction::RoleQuotaReached,
            "Failed to create SLR or instance profiles",
        ),
        (
            "the instance profiles cannot be created",
            Obstruction::InstanceProfileQuotaReached,
            "Failed to create instance profiles",
        ),
        (
            "only the service-linked role is left to create",
            Obstruction::RoleQuotaReachedWithProfilesHeld,
            "Failed to create SLR",
        ),
    ];

    for (case, obstruction, expected_message) in cases {
        let emulator = Emulator::new();
        match obstruction {
            Obstruction::RoleQuotaReached => emulator.fill_role_quota(&[]),
            Obstruction::InstanceProfileQuotaReached => emulator.fill_instance_profile_quota(),
            Obstruction::RoleQuotaReachedWithProfilesHeld => {
                emulator.fill_role_quota(SERVER_INSTANCE_PROFILES);
                emulator.seed_instance_profiles(SERVER_INSTANCE_PROFILES);
            }
        }

        let answer = emulator.initialize().await;
        assert_refusal(case, &answer, expected_message);
        assert_uninitialized(case, &emulator).await;

        // What the attempt could not establish is what the sentence names: with the role
        // quota reached, no instance profile holds a role; with the profile quota reached,
        // IAM holds no such profile at all while the service-linked role WAS established.
        match obstruction {
            Obstruction::RoleQuotaReached => {
                for name in SERVER_INSTANCE_PROFILES {
                    let held = emulator.instance_profile_roles(name).await;
                    assert!(
                        held.is_none_or(|held| held.is_empty()),
                        "case {case:?}: instance profile {name:?} must hold no role"
                    );
                }
                assert!(
                    emulator.service_linked_roles().await.is_empty(),
                    "case {case:?}: no service-linked role can have been established"
                );
            }
            Obstruction::InstanceProfileQuotaReached => {
                for name in SERVER_INSTANCE_PROFILES {
                    let held = emulator.instance_profile_roles(name).await;
                    assert!(
                        held.is_none(),
                        "case {case:?}: IAM must hold no instance profile {name:?}, got \
                         {held:?}"
                    );
                }
                assert_eq!(
                    emulator.service_linked_roles().await.len(),
                    1,
                    "case {case:?}: the service-linked role IAM could hold was established"
                );
            }
            Obstruction::RoleQuotaReachedWithProfilesHeld => {
                assert!(
                    emulator.service_linked_roles().await.is_empty(),
                    "case {case:?}: no service-linked role can have been established"
                );
            }
        }
    }
}

/// The failure is driven by the state the attempt establishes, not by the request: the
/// same call that was refused serves once IAM can hold what it could not before, and the
/// retry completes what the refused attempt had established.
#[tokio::test]
async fn an_initialization_serves_once_iam_can_hold_what_it_could_not() {
    let emulator = Emulator::new();
    emulator.fill_instance_profile_quota();

    let refused = emulator.initialize().await;
    assert_refusal(
        "the instance profiles cannot be created",
        &refused,
        "Failed to create instance profiles",
    );
    assert_uninitialized("the instance profiles cannot be created", &emulator).await;

    emulator.clear_instance_profiles();

    let answer = emulator.initialize().await;
    assert_eq!(
        answer.status,
        204,
        "the identical call must serve once IAM can hold the profiles, got {} with {:?}",
        answer.status,
        answer.text()
    );
    for name in SERVER_INSTANCE_PROFILES {
        let held = emulator.instance_profile_roles(name).await;
        assert_eq!(
            held.as_deref(),
            Some([(*name).to_string()].as_slice()),
            "instance profile {name:?} must now hold the role of that name, got {held:?}"
        );
    }
    let created = emulator
        .post("CreateApplication", &json!({"name": "initialized-after-retry"}))
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateApplication expected 201 once the retry initialized the account, got {} with \
         {:?}",
        created.status,
        created.text()
    );
}
