// SPDX-License-Identifier: CC0-1.0

//! WHO may call this control plane, driven end to end over the real wire path (a restJson1
//! request through the multi-service router into the mounted service), so what is asserted
//! is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation of this service is authorized, BEFORE it reads or
//! writes a pipe, against the identity-based IAM policies in force on the principal its
//! credentials name — for the action `pipes:<OperationName>` (this service's own namespace,
//! DISJOINT from the event bus API's `events:`) on the pipe the request addresses, by the
//! one ARN form this namespace publishes for a pipe. A request no readable statement allows
//! (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException` with IAM's own sentence and changes nothing — a denied `StartPipe`
//! leaves the pipe where it was; a request a matching `Allow` covers reaches the service's
//! own verdict.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` / `iam:PutUserPolicy` store and the principal
//! a refusal blames is the one `sts:GetCallerIdentity` answers for the key the request
//! signed with.
//!
//! The tables span what the rule generalises over: the OPERATION called (all ten the model
//! binds, `smithy_query(query="digest")`), the document in force (none at all, the event bus
//! API's whole namespace, another action of this service, exactly this action, this whole
//! service, an explicit deny beside an allow), the resource an allow is scoped to (the pipe
//! addressed, another pipe, the same name in another Region, every resource), and the kind of
//! principal (an assumed-role session, an IAM user's long-term key, the account's own
//! published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_pipes_pipes::EmulatedService;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{minimal_body, reply_of, Reply};
use crate::pipe_tagging::encode;

/// The Region every request below is signed for, which is the Region of the pipe ARN a
/// decision is taken against.
const REGION: &str = "us-west-2";

/// An access key id neither store holds a record for: the account's own published
/// credential, which holds no IAM policy.
const AMBIENT_KEY_ID: &str = "AKIDPIPESAUTHAMBIENT";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEPIPESSESSION";

/// The access key id of the IAM user the long-term rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEPIPESUSER";

/// The role the session is assumed from, the session's own name, the IAM user, and the name
/// of the inline policy each of them carries.
const ROLE: &str = "pipes-control-plane-caller";
const SESSION: &str = "deny-test";
const USER: &str = "pipes-control-plane-user";
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The status and error code an unauthorized call is answered with: the one the AWS "Common
/// Errors" table gives an authorization refusal, which this model declares for no operation
/// of its own.
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The pipe every row addresses, which no call below creates — so a refusal is decided
/// without the pipe existing.
const PIPE: &str = "ordersauthorized";

/// A second pipe, for the rows that read a grant scoped to one pipe against another.
const OTHER_PIPE: &str = "ordersunrelated";

/// The ARN this service mints for the pipe `name` in `region`. Spelled out here rather than
/// built from the implementation's own helper, so a change to either fails this test.
fn pipe_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:pipes:{region}:{}:pipe/{name}",
        engine::emulator_account_id()
    )
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", engine::emulator_account_id())
}

/// The ARN the account holds the IAM user under, which a refusal names as that caller.
fn user_arn() -> String {
    format!("arn:aws:iam::{}:user/{USER}", engine::emulator_account_id())
}

/// This service over a shared engine whose IAM and STS stores this suite seeds, reached
/// through the same `MultiServiceRouter` the production router uses.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// One request signed with `access_key_id`, whose credential scope names this service's
    /// signing name (which is how the router picks the service) and `region`. No
    /// authenticator is registered in these tests, so the signature itself is never
    /// verified — the key id is what the gate resolves the principal from.
    async fn send_as(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Reply {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
            .unwrap_or_default();
        let host = format!("pipes.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/pipes/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("build the request");
        reply_of(self.router.dispatch(request).await)
    }
}

/// Every operation the model binds, and how a request to it is addressed — so one table can
/// call each of them without any row stating a wire detail twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    CreatePipe,
    DeletePipe,
    DescribePipe,
    ListPipes,
    ListTagsForResource,
    StartPipe,
    StopPipe,
    TagResource,
    UntagResource,
    UpdatePipe,
}

/// The ten operations `smithy_query(query="digest")` reports for this service.
const EVERY_OP: &[Op] = &[
    Op::CreatePipe,
    Op::DeletePipe,
    Op::DescribePipe,
    Op::ListPipes,
    Op::ListTagsForResource,
    Op::StartPipe,
    Op::StopPipe,
    Op::TagResource,
    Op::UntagResource,
    Op::UpdatePipe,
];

impl Op {
    /// The operation's own name, which is also the name of the IAM action it is performed
    /// under.
    fn name(self) -> &'static str {
        match self {
            Self::CreatePipe => "CreatePipe",
            Self::DeletePipe => "DeletePipe",
            Self::DescribePipe => "DescribePipe",
            Self::ListPipes => "ListPipes",
            Self::ListTagsForResource => "ListTagsForResource",
            Self::StartPipe => "StartPipe",
            Self::StopPipe => "StopPipe",
            Self::TagResource => "TagResource",
            Self::UntagResource => "UntagResource",
            Self::UpdatePipe => "UpdatePipe",
        }
    }

    /// The IAM action this operation is performed under: this service's namespace and the
    /// operation's own name.
    fn action(self) -> String {
        format!("pipes:{}", self.name())
    }

    /// The resource a decision about this operation is taken against: the pipe it addresses,
    /// and every resource for the listing, which addresses no pipe at all.
    fn resource(self, region: &str, name: &str) -> String {
        match self {
            Self::ListPipes => "*".to_string(),
            _ => pipe_arn(region, name),
        }
    }

    /// Call this operation on the pipe `name`, signed with `access_key_id`.
    async fn call(
        self,
        harness: &Harness,
        access_key_id: &str,
        region: &str,
        name: &str,
    ) -> Reply {
        let arn = encode(&pipe_arn(region, name));
        let (method, uri, body) = match self {
            Self::CreatePipe => (
                "POST",
                format!("/v1/pipes/{name}"),
                Some(minimal_body(
                    &format!("arn:aws:sqs:{region}:000000000000:orders"),
                    &format!("arn:aws:lambda:{region}:000000000000:function:handler"),
                )),
            ),
            Self::DeletePipe => ("DELETE", format!("/v1/pipes/{name}"), None),
            Self::DescribePipe => ("GET", format!("/v1/pipes/{name}"), None),
            Self::ListPipes => ("GET", "/v1/pipes".to_string(), None),
            Self::ListTagsForResource => ("GET", format!("/tags/{arn}"), None),
            Self::StartPipe => ("POST", format!("/v1/pipes/{name}/start"), None),
            Self::StopPipe => ("POST", format!("/v1/pipes/{name}/stop"), None),
            Self::TagResource => (
                "POST",
                format!("/tags/{arn}"),
                Some(json!({ "tags": { "env": "prod" } })),
            ),
            Self::UntagResource => ("DELETE", format!("/tags/{arn}?tagKeys=env"), None),
            Self::UpdatePipe => (
                "PUT",
                format!("/v1/pipes/{name}"),
                Some(json!({ "RoleArn": "arn:aws:iam::000000000000:role/pipes-role" })),
            ),
        };
        harness
            .send_as(access_key_id, method, &uri, body, region)
            .await
    }
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEPIPESCALLER".to_string(),
            arn: role_arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
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
                account: engine::emulator_account_id().to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it.
fn given_user(engine: &Arc<engine::InMemoryEngine>) {
    let minted = aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER.to_string(),
            user_id: "AIDAEXAMPLEPIPESUSER1".to_string(),
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
            user_key: iam::name_key(USER),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Which identity an inline policy is attached to.
#[derive(Debug, Clone, Copy)]
enum Target {
    Role,
    User,
}

/// Put `document` in force as the identity's inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` / `iam:PutUserPolicy` do. `None` takes every policy off it, as
/// the matching delete does.
fn given_inline_policy(
    engine: &Arc<engine::InMemoryEngine>,
    target: Target,
    document: Option<&Value>,
) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    let (key, attached) = match target {
        Target::Role => (iam::name_key(ROLE), &mut store.role_inline_policies),
        Target::User => (iam::name_key(USER), &mut store.user_inline_policies),
    };
    match document {
        None => {
            attached.remove(&key);
        }
        Some(document) => {
            attached.insert(
                key,
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }],
    })
}

/// A policy allowing `action` on `resource` alone.
fn allow_on(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
}

/// Which of IAM's refusals a row expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The default deny of a request no readable statement allows.
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

/// IAM's whole sentence for a refusal: who was refused, which action, on which resource, and
/// which of IAM's rules refused it. Spelled out here rather than built from the helpers the
/// implementation calls, so a change to either half of the wording fails this test.
fn denial(principal: &str, action: &str, resource: &str, refusal: Refusal) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {}",
        refusal.reason(action)
    )
}

/// Assert that `reply` is the authorization refusal naming `principal`, `action` and
/// `resource`.
fn assert_refused(
    case: &str,
    reply: &Reply,
    principal: &str,
    action: &str,
    resource: &str,
    refusal: Refusal,
) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED:?}, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        denial(principal, action, resource, refusal),
        "case {case:?}: expected IAM's own sentence, got {:?}",
        reply.message()
    );
}

/// Assert that `reply` is not an authorization refusal: the call reached the service's own
/// verdict, whatever that verdict was.
fn assert_not_refused(case: &str, reply: &Reply) {
    assert_ne!(
        (reply.status, reply.error_code.as_str()),
        ACCESS_DENIED,
        "case {case:?}: expected the call to reach the service, got {reply:?}"
    );
}

/// One document a row puts in force, and what it says about the operation under test.
struct Document {
    case: &'static str,
    /// The policy attached to the role, or `None` for a role carrying none at all.
    document: Option<Value>,
    /// How IAM refuses the call under it, or `None` for a document that allows it.
    expected: Option<Refusal>,
}

/// The documents a row puts in force, and what each says about the operation under test:
/// `None` for a row the policies allow, `Some(refusal)` for one they do not.
fn documents_in_force(op: Op) -> Vec<Document> {
    vec![
        Document {
            case: "no document at all",
            document: None,
            expected: Some(Refusal::DefaultDeny),
        },
        Document {
            case: "every action of this service",
            document: Some(allow(json!("pipes:*"))),
            expected: None,
        },
        Document {
            case: "exactly this action",
            document: Some(allow(json!(op.action()))),
            expected: None,
        },
        Document {
            case: "another action of this service",
            document: Some(allow(json!(other_action(op)))),
            expected: Some(Refusal::DefaultDeny),
        },
        Document {
            case: "every action of the event bus API",
            document: Some(allow(json!("events:*"))),
            expected: Some(Refusal::DefaultDeny),
        },
        Document {
            case: "an explicit deny beside an allow of the namespace",
            document: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "pipes:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": op.action(), "Resource": "*" },
                ],
            })),
            expected: Some(Refusal::ExplicitDeny),
        },
    ]
}

/// An action of this service that is NOT the one under test, so a grant of it allows the row
/// nothing.
fn other_action(op: Op) -> String {
    EVERY_OP
        .iter()
        .find(|other| **other != op)
        .expect("the service binds more than one operation")
        .action()
}

/// RULE: each operation is authorized under its OWN `pipes:` action on the pipe the request
/// addresses, so a document that allows another action of this service, or every action of
/// another service, allows none of them — and an explicit deny refuses whatever else the
/// principal carries.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_pipes_action() {
    let harness = Harness::new();
    given_session(&harness.engine);

    for op in EVERY_OP {
        let action = op.action();
        let resource = op.resource(REGION, PIPE);
        for row in documents_in_force(*op) {
            let case = format!("{} / {}", op.name(), row.case);
            given_inline_policy(&harness.engine, Target::Role, row.document.as_ref());
            let reply = op.call(&harness, SESSION_KEY_ID, REGION, PIPE).await;
            match row.expected {
                Some(refusal) => assert_refused(
                    &case,
                    &reply,
                    &session_arn(),
                    &action,
                    &resource,
                    refusal,
                ),
                None => assert_not_refused(&case, &reply),
            }
        }
    }
}

/// RULE: the decision is taken against the PIPE the request addresses, so a grant scoped to
/// one pipe's ARN covers that pipe and no other — not another pipe of the same account and
/// Region, and not the same name in another Region — and a call that addresses no pipe at
/// all is covered by neither.
#[tokio::test]
async fn a_grant_is_scoped_to_the_pipe_the_request_addresses() {
    let harness = Harness::new();
    given_session(&harness.engine);
    let granted = pipe_arn(REGION, PIPE);
    given_inline_policy(
        &harness.engine,
        Target::Role,
        Some(&allow_on("pipes:*", &granted)),
    );

    // (case, the operation called, the Region it is signed for, the pipe it addresses,
    //  whether the grant covers it)
    let rows: &[(&str, Op, &str, &str, bool)] = &[
        (
            "the pipe the grant names",
            Op::DescribePipe,
            REGION,
            PIPE,
            true,
        ),
        (
            "another operation on the pipe the grant names",
            Op::StartPipe,
            REGION,
            PIPE,
            true,
        ),
        (
            "another pipe of the same account and Region",
            Op::DescribePipe,
            REGION,
            OTHER_PIPE,
            false,
        ),
        (
            "the same pipe name in another Region",
            Op::DescribePipe,
            "eu-central-1",
            PIPE,
            false,
        ),
        (
            "a call that addresses no pipe at all",
            Op::ListPipes,
            REGION,
            PIPE,
            false,
        ),
    ];

    for (case, op, region, name, covered) in rows {
        let reply = op.call(&harness, SESSION_KEY_ID, region, name).await;
        if *covered {
            assert_not_refused(case, &reply);
        } else {
            assert_refused(
                case,
                &reply,
                &session_arn(),
                &op.action(),
                &op.resource(region, name),
                Refusal::DefaultDeny,
            );
        }
    }
}

/// RULE: the principal a request is judged on is the one its access key id speaks for — an
/// assumed-role session on the ROLE's documents, an IAM user's long-term key on the user's
/// own, and the account's own published credential on nothing, because the IAM store holds
/// no policy for it.
#[tokio::test]
async fn each_kind_of_principal_is_judged_on_its_own_policies() {
    let harness = Harness::new();
    given_session(&harness.engine);
    given_user(&harness.engine);

    // One row: the credential a request presents, the policy in force on the identity it
    // speaks for, and the principal a refusal must name (`None` for a row that is served).
    struct Row {
        case: &'static str,
        /// The access key id the request signs with.
        key_id: &'static str,
        /// The identity the row's policy is attached to, and what it allows — `None` for a
        /// credential no identity of this account holds a policy for.
        target: Option<Target>,
        document: Option<Value>,
        /// The principal IAM blames, for a row the policies do not allow.
        refused_principal: Option<String>,
    }

    let rows = vec![
        Row {
            case: "the account's own published credential, which carries no policy",
            key_id: AMBIENT_KEY_ID,
            target: None,
            document: None,
            refused_principal: None,
        },
        Row {
            case: "an assumed-role session whose role allows the action",
            key_id: SESSION_KEY_ID,
            target: Some(Target::Role),
            document: Some(allow(json!("pipes:DescribePipe"))),
            refused_principal: None,
        },
        Row {
            case: "an assumed-role session whose role allows nothing of this service",
            key_id: SESSION_KEY_ID,
            target: Some(Target::Role),
            document: None,
            refused_principal: Some(session_arn()),
        },
        Row {
            case: "an IAM user's long-term key, allowed by the user's own policy",
            key_id: USER_KEY_ID,
            target: Some(Target::User),
            document: Some(allow(json!("pipes:*"))),
            refused_principal: None,
        },
        Row {
            case: "an IAM user's long-term key with no policy at all",
            key_id: USER_KEY_ID,
            target: Some(Target::User),
            document: None,
            refused_principal: Some(user_arn()),
        },
    ];

    for row in rows {
        if let Some(target) = row.target {
            given_inline_policy(&harness.engine, target, row.document.as_ref());
        }
        let reply = Op::DescribePipe
            .call(&harness, row.key_id, REGION, PIPE)
            .await;
        match row.refused_principal {
            None => assert_not_refused(row.case, &reply),
            Some(principal) => assert_refused(
                row.case,
                &reply,
                &principal,
                &Op::DescribePipe.action(),
                &pipe_arn(REGION, PIPE),
                Refusal::DefaultDeny,
            ),
        }
    }
}

/// RULE, the half a status code cannot show: a denied call performs NOTHING, and granting the
/// action serves the very call that was refused — so the state a client reads back is the
/// state the authorized calls left.
#[tokio::test]
async fn a_denied_call_changes_nothing_and_a_granted_one_is_served() {
    let harness = Harness::new();
    given_session(&harness.engine);
    let case = "a pipe created, then started by a role the grant is added to";

    // The pipe itself is created by the account's own credential, which carries no policy.
    let name = "authorizedroundtrip";
    let created = Op::CreatePipe
        .call(&harness, AMBIENT_KEY_ID, REGION, name)
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected CreatePipe to answer 200, got {created:?}"
    );

    // The session's role allows another action of this service and nothing else, so its
    // tagging call is refused — and writes no tag.
    given_inline_policy(
        &harness.engine,
        Target::Role,
        Some(&allow(json!("pipes:DescribePipe"))),
    );
    let refused = Op::TagResource
        .call(&harness, SESSION_KEY_ID, REGION, name)
        .await;
    assert_refused(
        case,
        &refused,
        &session_arn(),
        &Op::TagResource.action(),
        &pipe_arn(REGION, name),
        Refusal::DefaultDeny,
    );
    let described = Op::DescribePipe
        .call(&harness, AMBIENT_KEY_ID, REGION, name)
        .await;
    assert_eq!(
        described.member("Tags"),
        &json!({}),
        "case {case:?}: expected the refused tagging call to write nothing, got {described:?}"
    );

    // The same call, once the role is granted the action: served, and what it wrote is
    // what a later reader sees.
    given_inline_policy(
        &harness.engine,
        Target::Role,
        Some(&allow(json!(["pipes:DescribePipe", "pipes:TagResource"]))),
    );
    let tagged = Op::TagResource
        .call(&harness, SESSION_KEY_ID, REGION, name)
        .await;
    assert_not_refused(case, &tagged);
    let described = Op::DescribePipe
        .call(&harness, SESSION_KEY_ID, REGION, name)
        .await;
    assert_eq!(
        described.member("Tags"),
        &json!({ "env": "prod" }),
        "case {case:?}: expected the granted tagging call's own write, got {described:?}"
    );
}
