// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! `restJson1` request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: IoT Greengrass V2 decides authorization at the IAM layer before it
//! reads or writes anything of its own, so every operation whose model declares
//! `AccessDeniedException` (HTTP 403, `OPERATIONS.md`) evaluates the identity-based IAM
//! policies in force on the principal its credentials name — against
//! `greengrass:<OperationName>` on the resource the request addresses — FIRST. A request no
//! statement allows (IAM's default deny), or one an explicit `Deny` covers, is answered 403
//! `AccessDeniedException` with IAM's own sentence and changes nothing; a request a matching
//! `Allow` covers reaches the operation's own answer.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` / `iam:PutUserPolicy` store and the principal
//! a refusal blames is the one `sts:GetCallerIdentity` reports for the credentials.
//!
//! The tables span what the rule generalises over: the operation (a component create keyed
//! on the recipe's own component name, a deployment create keyed on its IoT target, reads
//! and deletes keyed on an ARN the request carries, core-device reads and writes keyed on a
//! thing name, and listings that address no resource at all), the document in force (none,
//! another action of this service, another service's actions, an explicit deny beside an
//! allow, an allow of exactly the action, of it in another case, of the whole service and of
//! everything), the resource an allow is scoped to, and the principal (an assumed-role
//! session, an IAM user's long-term key, and the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, which is the Region every ARN this service
/// mints for it carries.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "component-publisher";
const SESSION: &str = "publish-session";
const USER_NAME: &str = "component-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The component the create rows publish, and the version they publish it at.
const COMPONENT: &str = "com.example.Probe";
const VERSION: &str = "1.0.0";

/// A router serving this service beside the IAM and STS services, over ONE shared engine, so
/// the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Call one route of this service signed with `access_key_id` — what names the principal the
/// request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("a serializable request body"))
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` member of an error body, or `"<none>"`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// Percent-encode the colons in an ARN, the way an AWS SDK encodes an `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A")
}

/// The name-level component ARN this service mints, which is the resource a component
/// create is judged against.
fn component_arn(name: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}")
}

/// The component-version ARN this service mints.
fn version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}:versions:{version}")
}

/// The deployment ARN of this service's ARN grammar (`GenericV2ARN`: `deployments:<id>`).
fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:{deployment_id}")
}

/// The core-device ARN of that same grammar (`coreDevices:<coreDeviceThingName>`).
fn core_device_arn(thing_name: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:coreDevices:{thing_name}")
}

/// An IoT thing-group ARN, which is the target a deployment is created FOR.
fn thing_group_arn(group: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/{group}")
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// The IAM user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{USER_NAME}")
}

/// A component recipe for `name`@`version`, as `inlineRecipe` carries one (a blob, so
/// base64 on the wire).
fn recipe_body(name: &str, version: &str) -> Value {
    let recipe = json!({
        "RecipeFormatVersion": "2020-01-25",
        "ComponentName": name,
        "ComponentVersion": version,
        "Manifests": [],
    })
    .to_string();
    json!({ "inlineRecipe": base64::encode(recipe.as_bytes()) })
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEPUBLISHROLE".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key id
/// to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLECOMPONENTUSER".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// One inline policy holding `document`, as `iam:PutRolePolicy` / `iam:PutUserPolicy` store
/// one.
fn inline(document: &Value) -> BTreeMap<String, iam::InlinePolicy> {
    BTreeMap::from([(
        iam::name_key(POLICY_NAME),
        iam::InlinePolicy {
            policy_name: POLICY_NAME.to_string(),
            policy_document: document.to_string(),
        },
    )])
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried.
/// `None` takes every policy off it, as `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.remove(&iam::name_key(ROLE));
    if let Some(document) = document {
        store
            .role_inline_policies
            .insert(iam::name_key(ROLE), inline(document));
    }
}

/// The same for the IAM user, as `iam:PutUserPolicy` / `iam:DeleteUserPolicy` do.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store
            .user_inline_policies
            .insert(iam::name_key(USER_NAME), inline(document));
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
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
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("iot:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of exactly this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("greengrass:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// One operation of this service, as a client addresses it.
struct Operation {
    /// The operation's own name: the action asked about is `greengrass:<name>`.
    name: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The ARN the request ADDRESSES, or `None` for an operation that addresses no resource
    /// of its own — the refusal then names no resource.
    resource: Option<String>,
}

/// Every operation whose model declares `AccessDeniedException` and that serves an IAM
/// caller, spanning the ways a request names the resource it addresses: the recipe's own
/// component name, an IoT target, an ARN the request carries, a core-device thing name, and
/// nothing at all. (`ResolveComponentCandidates` is the one operation left out: it is served
/// only to a core device authenticating with an IoT device certificate, so it refuses an IAM
/// principal outright instead of judging its policies; `GetComponentVersionArtifact` is not
/// implemented yet.)
fn operations() -> Vec<Operation> {
    let version = version_arn(COMPONENT, VERSION);
    vec![
        Operation {
            name: "CreateComponentVersion",
            method: "POST",
            path: "/greengrass/v2/createComponentVersion".to_string(),
            body: Some(recipe_body(COMPONENT, VERSION)),
            resource: Some(component_arn(COMPONENT)),
        },
        Operation {
            name: "CreateDeployment",
            method: "POST",
            path: "/greengrass/v2/deployments".to_string(),
            body: Some(json!({ "targetArn": thing_group_arn("Fleet") })),
            resource: Some(thing_group_arn("Fleet")),
        },
        Operation {
            name: "GetComponent",
            method: "GET",
            path: format!("/greengrass/v2/components/{}", encoded_label(&version)),
            body: None,
            resource: Some(version.clone()),
        },
        Operation {
            name: "DeleteComponent",
            method: "DELETE",
            path: format!("/greengrass/v2/components/{}", encoded_label(&version)),
            body: None,
            resource: Some(version.clone()),
        },
        Operation {
            name: "ListComponentVersions",
            method: "GET",
            path: format!(
                "/greengrass/v2/components/{}/versions",
                encoded_label(&component_arn(COMPONENT))
            ),
            body: None,
            resource: Some(component_arn(COMPONENT)),
        },
        Operation {
            name: "ListComponents",
            method: "GET",
            path: "/greengrass/v2/components".to_string(),
            body: None,
            resource: None,
        },
        Operation {
            name: "GetDeployment",
            method: "GET",
            path: "/greengrass/v2/deployments/d-probe".to_string(),
            body: None,
            resource: Some(deployment_arn("d-probe")),
        },
        Operation {
            name: "CancelDeployment",
            method: "POST",
            path: "/greengrass/v2/deployments/d-probe/cancel".to_string(),
            body: None,
            resource: Some(deployment_arn("d-probe")),
        },
        Operation {
            name: "ListDeployments",
            method: "GET",
            path: "/greengrass/v2/deployments".to_string(),
            body: None,
            resource: None,
        },
        Operation {
            name: "GetCoreDevice",
            method: "GET",
            path: "/greengrass/v2/coreDevices/probe-core".to_string(),
            body: None,
            resource: Some(core_device_arn("probe-core")),
        },
        Operation {
            name: "DeleteCoreDevice",
            method: "DELETE",
            path: "/greengrass/v2/coreDevices/probe-core".to_string(),
            body: None,
            resource: Some(core_device_arn("probe-core")),
        },
        Operation {
            name: "BatchAssociateClientDeviceWithCoreDevice",
            method: "POST",
            path: "/greengrass/v2/coreDevices/probe-core/associateClientDevices".to_string(),
            body: Some(json!({ "entries": [{ "thingName": "probe-client" }] })),
            resource: Some(core_device_arn("probe-core")),
        },
        Operation {
            name: "ListClientDevicesAssociatedWithCoreDevice",
            method: "GET",
            path: "/greengrass/v2/coreDevices/probe-core/associatedClientDevices".to_string(),
            body: None,
            resource: Some(core_device_arn("probe-core")),
        },
        Operation {
            name: "ListEffectiveDeployments",
            method: "GET",
            path: "/greengrass/v2/coreDevices/probe-core/effectiveDeployments".to_string(),
            body: None,
            resource: Some(core_device_arn("probe-core")),
        },
        Operation {
            name: "ListInstalledComponents",
            method: "GET",
            path: "/greengrass/v2/coreDevices/probe-core/installedComponents".to_string(),
            body: None,
            resource: Some(core_device_arn("probe-core")),
        },
    ]
}

/// The sentence IAM refuses an unauthorized request with: the principal, the action, the
/// resource the request addressed (none for an action that addresses none) and the rule that
/// refused it.
fn not_authorized(principal: &str, action: &str, resource: Option<&str>, reason: &str) -> String {
    let on_resource = resource.map_or_else(String::new, |arn| format!(" on resource: {arn}"));
    format!("User: {principal} is not authorized to perform: {action}{on_resource} {reason}")
}

/// Assert one answer is the modelled authorization refusal, worded as IAM words it.
fn assert_refused(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 403,
        "case {case:?}: expected the authorization refusal at 403, got {status} {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {code:?} {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected IAM's wording, got {message:?}"
    );
}

/// RULE: every operation that models the 403 is judged on the identity-based policies in
/// force on the calling principal, against `greengrass:<OperationName>` on the resource the
/// request addresses — refused before it reads or writes anything, served when a matching
/// `Allow` covers it.
#[tokio::test]
async fn every_authorized_operation_follows_the_identity_policies_in_force() {
    let operations = operations();
    for (index, operation) in operations.iter().enumerate() {
        let action = format!("greengrass:{}", operation.name);
        // Another action of this service, which must not stand in for this one: the next
        // operation in the table.
        let other = format!(
            "greengrass:{}",
            operations[(index + 1) % operations.len()].name
        );
        for (document_case, document, refusal) in documents_in_force(&action, &other) {
            let case = format!("{} under {document_case}", operation.name);
            // A fresh store, role and session per row: no row can pass on what another
            // left behind.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let (status, body) = call_as(
                &router,
                SESSION_KEY_ID,
                operation.method,
                &operation.path,
                operation.body.as_ref(),
            )
            .await;

            match refusal {
                Some(refusal) => assert_refused(
                    &case,
                    status,
                    &body,
                    &not_authorized(
                        &session_arn(),
                        &action,
                        operation.resource.as_deref(),
                        &refusal.reason(&action),
                    ),
                ),
                None => {
                    assert_ne!(
                        status, 403,
                        "case {case:?}: an allowed request must reach the operation, got \
                         {status} {body:?}"
                    );
                    assert_ne!(
                        error_code(&body),
                        "AccessDeniedException",
                        "case {case:?}: an allowed request must not be refused, got \
                         {status} {body:?}"
                    );
                }
            }
        }
    }
}

/// RULE (the recorded case): an assumed-role session whose role carries no permission policy
/// may not publish a component version — the call is refused 403 with IAM's sentence naming
/// the component the recipe names, and no component version is registered. Attaching an
/// `Allow` lifts exactly that, and the published version reads back.
#[tokio::test]
async fn a_role_with_no_policy_cannot_publish_a_component_version() {
    let action = "greengrass:CreateComponentVersion";
    let (router, engine) = router_with_identity();
    given_session(&engine);
    given_role_policy(&engine, None);

    let (status, body) = call_as(
        &router,
        SESSION_KEY_ID,
        "POST",
        "/greengrass/v2/createComponentVersion",
        Some(&recipe_body(COMPONENT, VERSION)),
    )
    .await;
    assert_refused(
        "a role with no attached policy",
        status,
        &body,
        &not_authorized(
            &session_arn(),
            action,
            Some(&component_arn(COMPONENT)),
            &format!("because no identity-based policy allows the {action} action"),
        ),
    );

    // The refusal registered nothing: the account's own credential, which this gate does
    // not bound, lists the private components and finds none.
    let (status, listed) = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "GET",
        "/greengrass/v2/components?scope=PRIVATE",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the listing after the refusal: expected 200, got {status} {listed:?}"
    );
    let components = listed["components"].as_array().cloned().unwrap_or_default();
    assert!(
        components.is_empty(),
        "case the listing after the refusal: a refused create must register nothing, got \
         {components:?}"
    );

    // With the action allowed, the SAME request is served and the version reads back.
    given_role_policy(&engine, Some(&allow(json!(action))));
    let (status, created) = call_as(
        &router,
        SESSION_KEY_ID,
        "POST",
        "/greengrass/v2/createComponentVersion",
        Some(&recipe_body(COMPONENT, VERSION)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the allowed create: expected 201, got {status} {created:?}"
    );
    assert_eq!(
        created["arn"].as_str(),
        Some(version_arn(COMPONENT, VERSION).as_str()),
        "case the allowed create: expected the minted version ARN, got {created:?}"
    );

    let (status, described) = call_as(
        &router,
        SESSION_KEY_ID,
        "GET",
        &format!(
            "/greengrass/v2/components/{}/metadata",
            encoded_label(&version_arn(COMPONENT, VERSION))
        ),
        None,
    )
    .await;
    // The read is a different action, which this role is not allowed: the publish is read
    // back through the credential the gate does not bound.
    assert_eq!(
        status, 403,
        "case the read under a create-only policy: expected 403, got {status} {described:?}"
    );
    let (status, described) = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "GET",
        &format!(
            "/greengrass/v2/components/{}/metadata",
            encoded_label(&version_arn(COMPONENT, VERSION))
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the published version read back: expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described["componentName"].as_str(),
        Some(COMPONENT),
        "case the published version read back: expected {COMPONENT}, got {described:?}"
    );
}

/// RULE: a component create is judged against the ARN OF THE COMPONENT the recipe names, so
/// an allow scoped to one component covers that component and no other.
#[tokio::test]
async fn a_component_create_is_judged_against_the_component_the_recipe_names() {
    const OTHER: &str = "com.example.Other";
    let action = "greengrass:CreateComponentVersion";
    // (case, the Resource the allow is scoped to, whether publishing COMPONENT is allowed)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("an allow of every resource", json!("*"), true),
        (
            "an allow of exactly this component",
            json!(component_arn(COMPONENT)),
            true,
        ),
        (
            "an allow of every component of this account",
            json!(format!(
                "arn:aws:greengrass:{REGION}:{ACCOUNT}:components:*"
            )),
            true,
        ),
        (
            "an allow of another component only",
            json!(component_arn(OTHER)),
            false,
        ),
        (
            "an allow of this account's deployments only",
            json!(format!(
                "arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:*"
            )),
            false,
        ),
    ];

    for (case, resource, allowed) in rows {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(
            &engine,
            Some(&json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": action,
                    "Resource": resource,
                }],
            })),
        );

        let (status, body) = call_as(
            &router,
            SESSION_KEY_ID,
            "POST",
            "/greengrass/v2/createComponentVersion",
            Some(&recipe_body(COMPONENT, VERSION)),
        )
        .await;
        if allowed {
            assert_eq!(
                status, 201,
                "case {case:?}: an allowed create must be served, got {status} {body:?}"
            );
            assert_eq!(
                body["componentName"].as_str(),
                Some(COMPONENT),
                "case {case:?}: expected the published component, got {body:?}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                &not_authorized(
                    &session_arn(),
                    action,
                    Some(&component_arn(COMPONENT)),
                    &format!("because no identity-based policy allows the {action} action"),
                ),
            );
            let (status, listed) = call_as(
                &router,
                ACCOUNT_KEY_ID,
                "GET",
                "/greengrass/v2/components?scope=PRIVATE",
                None,
            )
            .await;
            let components = listed["components"].as_array().cloned().unwrap_or_default();
            assert!(
                components.is_empty(),
                "case {case:?}: a refused create registers nothing, got {status} {components:?}"
            );
        }
    }
}

/// RULE: the principal a request speaks for is the one its access key id names, so an IAM
/// user's long-term key is judged on the USER's own policies and the refusal names the user.
#[tokio::test]
async fn an_iam_users_long_term_key_is_judged_on_the_users_own_policies() {
    let action = "greengrass:ListComponents";
    // (case, the document on the user, whether the call is allowed)
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, false),
        (
            "an allow of another action of this service",
            Some(allow(json!("greengrass:ListDeployments"))),
            false,
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            true,
        ),
    ];

    for (case, document, allowed) in rows {
        let (router, engine) = router_with_identity();
        given_user_key(&engine);
        given_user_policy(&engine, document.as_ref());

        let (status, body) = call_as(
            &router,
            USER_KEY_ID,
            "GET",
            "/greengrass/v2/components",
            None,
        )
        .await;
        if allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: an allowed listing must be served, got {status} {body:?}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                &not_authorized(
                    &user_arn(),
                    action,
                    None,
                    &format!("because no identity-based policy allows the {action} action"),
                ),
            );
        }
    }
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    for operation in operations() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION.
        given_role_policy(&engine, None);

        let (status, body) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            operation.method,
            &operation.path,
            operation.body.as_ref(),
        )
        .await;
        assert_ne!(
            status, 403,
            "case {:?}: the account's own credential must not be refused, got {status} {body:?}",
            operation.name
        );
    }
}
