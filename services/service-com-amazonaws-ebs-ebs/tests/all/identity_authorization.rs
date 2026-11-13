// SPDX-License-Identifier: CC0-1.0

//! WHO may call the EBS direct APIs, driven end to end over the real wire path (a restJson1
//! request through the multi-service router into the mounted service), so what is asserted
//! is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation of this service is authorized, BEFORE it reads or
//! writes anything, against the identity-based IAM policies in force on the principal its
//! credentials name — for the action `ebs:<OperationName>` (this service's own namespace,
//! DISJOINT from the EC2 control plane's `ec2:`) on the snapshot the request addresses,
//! whose ARN is the EC2 snapshot's account-less `arn:aws:ec2:<region>::snapshot/<id>`. A
//! request no readable statement allows (IAM's default deny), or one an explicit `Deny`
//! covers, is answered HTTP 403 `AccessDeniedException` with IAM's own sentence and changes
//! nothing — a denied write leaves no block behind; a request a matching `Allow` covers
//! reaches the service's own verdict.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` / `iam:PutUserPolicy` store and the principal
//! a refusal blames is the one `sts:GetCallerIdentity` answers for the key the request
//! signed with.
//!
//! The tables span what the rule generalises over: the OPERATION called (all six the model
//! binds, `smithy_query(query="digest")`), the document in force (none at all, the EC2
//! control plane's whole namespace, another action of this service, exactly this action in
//! either case, this whole service, an explicit deny beside an allow), the resource an allow
//! is scoped to (the snapshot addressed, another snapshot, the same id in another Region,
//! every snapshot of the Region), the number of snapshots the request addresses, and the
//! kind of principal (an assumed-role session, an IAM user's long-term key, the account's
//! own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ebs_ebs::EmulatedService;

use crate::snapshot_block_loop::{block_bytes, checksum, listed_blocks, member, Reply, BLOCK_SIZE};

/// The Region every request below is signed for, which is the Region the snapshot ARN a
/// decision is taken against names.
const REGION: &str = "us-east-1";

/// An access key id neither store holds a record for: the account's own published
/// credential, which holds no IAM policy.
const AMBIENT_KEY_ID: &str = "AKIDEBSAUTHAMBIENT";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEEBSSESSION";

/// The access key id of the IAM user the long-term rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEEBSUSER";

/// The role the session is assumed from, the session's own name, the IAM user, and the name
/// of the inline policy each of them carries.
const ROLE: &str = "ebs-direct-api-writer";
const SESSION: &str = "deny-test";
const USER: &str = "ebs-direct-api-user";
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The status and error code the authorization layer answers an unauthorized call with,
/// which is `AccessDeniedException`'s modelled `@httpError`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#AccessDeniedException")`).
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The only `AccessDeniedExceptionReason` an authorization refusal carries: the caller is
/// not authorized, as against a refusal by a service this one depends on.
const UNAUTHORIZED_ACCOUNT: &str = "UNAUTHORIZED_ACCOUNT";

/// A snapshot id of the modelled `^snap-[0-9a-f]+$` form that no call below ever creates,
/// so a refusal is decided without the snapshot existing.
const ABSENT_SNAPSHOT_ID: &str = "snap-00000000000000001";

/// The ARN of an EC2 snapshot of this Region, which is the resource the EBS direct-API
/// actions are judged against: the account segment is EMPTY ("Resource types defined by
/// Amazon Elastic Block Store direct APIs", IAM Service Authorization Reference, which
/// spells the `snapshot` type `arn:${Partition}:ec2:${Region}::snapshot/${SnapshotId}`).
/// Spelled out here rather than built from the implementation's own helper, so a change to
/// either fails this test.
fn snapshot_arn(snapshot_id: &str) -> String {
    format!("arn:aws:ec2:{REGION}::snapshot/{snapshot_id}")
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

/// The emulated service over a shared engine the IAM and STS stores of this test are seeded
/// into, reached through the same `MultiServiceRouter` the production router uses.
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

    fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// One request signed with `access_key_id`, whose credential scope names this
    /// service's signing name (which is how the router picks the service) and [`REGION`].
    /// No authenticator is registered in these tests, so the signature itself is never
    /// verified — the key id is what the gate resolves the principal from.
    async fn send_as(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/ebs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00000000000000000000000000000000"
        );
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("ebs.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        for (name, value) in headers {
            builder = builder.header(*name, value.as_str());
        }
        let request = builder.body(Bytes::from(body)).expect("a valid request");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Reply {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body: body.to_vec(),
        }
    }
}

/// Every operation the model binds to this service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    StartSnapshot,
    PutSnapshotBlock,
    CompleteSnapshot,
    GetSnapshotBlock,
    ListSnapshotBlocks,
    ListChangedBlocks,
}

/// Every operation, in the order `smithy_query(query="digest")` reports them bound.
const EVERY_OP: [Op; 6] = [
    Op::StartSnapshot,
    Op::PutSnapshotBlock,
    Op::CompleteSnapshot,
    Op::GetSnapshotBlock,
    Op::ListSnapshotBlocks,
    Op::ListChangedBlocks,
];

impl Op {
    /// The operation's own name, which is what the action is built from.
    fn name(self) -> &'static str {
        match self {
            Self::StartSnapshot => "StartSnapshot",
            Self::PutSnapshotBlock => "PutSnapshotBlock",
            Self::CompleteSnapshot => "CompleteSnapshot",
            Self::GetSnapshotBlock => "GetSnapshotBlock",
            Self::ListSnapshotBlocks => "ListSnapshotBlocks",
            Self::ListChangedBlocks => "ListChangedBlocks",
        }
    }

    /// The IAM action a call of this operation is performed under.
    fn action(self) -> String {
        format!("ebs:{}", self.name())
    }

    /// The resource a refusal of this operation names: the snapshot the request addresses,
    /// or — for the create, whose snapshot has no id until it succeeds — every snapshot of
    /// the Region.
    fn resource(self, snapshot_id: &str) -> String {
        match self {
            Self::StartSnapshot => snapshot_arn("*"),
            _ => snapshot_arn(snapshot_id),
        }
    }

    /// Call this operation against `snapshot_id`, signed with `access_key_id`.
    ///
    /// The payloads are the smallest the route accepts: every member the model `@required`
    /// is present (an absent one is refused by the boundary before the operation runs), and
    /// nothing else about the request is what these tests vary.
    async fn call(self, harness: &Harness, access_key_id: &str, snapshot_id: &str) -> Reply {
        match self {
            Self::StartSnapshot => {
                harness
                    .send_as(
                        access_key_id,
                        "POST",
                        "/snapshots",
                        &[("content-type", "application/json".to_string())],
                        serde_json::to_vec(&json!({ "VolumeSize": 1 }))
                            .expect("a serializable body"),
                    )
                    .await
            }
            Self::PutSnapshotBlock => {
                let data = vec![0_u8; 1];
                harness
                    .send_as(
                        access_key_id,
                        "PUT",
                        &format!("/snapshots/{snapshot_id}/blocks/0"),
                        &[
                            ("content-type", "application/octet-stream".to_string()),
                            ("x-amz-Data-Length", BLOCK_SIZE.to_string()),
                            ("x-amz-Checksum", checksum(&data)),
                            ("x-amz-Checksum-Algorithm", "SHA256".to_string()),
                        ],
                        data,
                    )
                    .await
            }
            Self::CompleteSnapshot => {
                harness
                    .send_as(
                        access_key_id,
                        "POST",
                        &format!("/snapshots/completion/{snapshot_id}"),
                        &[("x-amz-ChangedBlocksCount", "0".to_string())],
                        Vec::new(),
                    )
                    .await
            }
            Self::GetSnapshotBlock => {
                harness
                    .send_as(
                        access_key_id,
                        "GET",
                        &format!("/snapshots/{snapshot_id}/blocks/0?blockToken=dG9rZW4="),
                        &[],
                        Vec::new(),
                    )
                    .await
            }
            Self::ListSnapshotBlocks => {
                harness
                    .send_as(
                        access_key_id,
                        "GET",
                        &format!("/snapshots/{snapshot_id}/blocks"),
                        &[],
                        Vec::new(),
                    )
                    .await
            }
            Self::ListChangedBlocks => {
                harness
                    .send_as(
                        access_key_id,
                        "GET",
                        &format!("/snapshots/{snapshot_id}/changedblocks"),
                        &[],
                        Vec::new(),
                    )
                    .await
            }
        }
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
            role_id: "AROAEXAMPLEEBSDIRECTAPI".to_string(),
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
            user_id: "AIDAEXAMPLEEBSDIRECTAPI1".to_string(),
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
        (reply.status, reply.error_code().as_str()),
        ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED:?}, got {} with body {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let stated = member(reply, "Message");
    assert_eq!(
        stated,
        denial(principal, action, resource, refusal),
        "case {case:?}: expected IAM's wording, got {stated:?}"
    );
    let reason = member(reply, "Reason");
    assert_eq!(
        reason, UNAUTHORIZED_ACCOUNT,
        "case {case:?}: expected the modelled reason {UNAUTHORIZED_ACCOUNT}, got {reason:?}"
    );
}

/// Assert that `reply` is NOT an authorization refusal: an allowed call reaches the
/// service's own verdict, whatever that verdict is.
fn assert_not_refused(case: &str, reply: &Reply) {
    assert_ne!(
        reply.status, ACCESS_DENIED.0,
        "case {case:?}: an allowed call is not refused, got {} with body {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_ne!(
        reply.error_code(),
        ACCESS_DENIED.1,
        "case {case:?}: an allowed call is not an authorization refusal, got body {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

/// The documents a row puts in force for `op`, and which of IAM's refusals the request then
/// meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(op: Op) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    let action = op.action();
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            // The recorded arm's Allow leg: the EC2 control plane's whole namespace, which
            // publishes no action of this service.
            "every action of the EC2 control plane",
            Some(allow(json!("ec2:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            // The right operation name under the wrong service prefix.
            "this operation's name under the EC2 namespace",
            Some(allow(json!(format!("ec2:{}", op.name())))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "a prefix that only starts like this service's",
            Some(allow(json!("ebsx:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            // The recorded arm's Deny leg.
            "an explicit deny of this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of the action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "ebs:*", "Resource": "*" },
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
            Some(allow(json!("ebs:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// THE RULE, over every operation the model binds: the verdict depends on the caller's
/// identity-based policies, judged against `ebs:<OperationName>` — a grant of the EC2
/// control plane's actions covers none of it — and a refusal is IAM's 403 naming the
/// principal, that action and the snapshot the request addressed. An allowed call falls
/// through to the service's own verdict.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_ebs_action() {
    for op in EVERY_OP {
        for (row, document, refused) in documents_in_force(op) {
            let case = format!("{} / {row}", op.name());
            // A fresh engine per row: no row can pass on another's write.
            let harness = Harness::new();
            given_session(harness.engine());
            given_inline_policy(harness.engine(), Target::Role, document.as_ref());

            let reply = op.call(&harness, SESSION_KEY_ID, ABSENT_SNAPSHOT_ID).await;
            match refused {
                Some(refusal) => assert_refused(
                    &case,
                    &reply,
                    &session_arn(),
                    &op.action(),
                    &op.resource(ABSENT_SNAPSHOT_ID),
                    refusal,
                ),
                None => assert_not_refused(&case, &reply),
            }
        }
    }
}

/// RULE: the decision is taken against the SNAPSHOT the request addresses, so an allow
/// scoped to one snapshot ARN covers that snapshot and no other — and a request addressing
/// SEVERAL snapshots needs the action allowed on each of them, being refused on the first it
/// is not.
#[tokio::test]
async fn an_allow_is_scoped_to_the_snapshots_the_request_addresses() {
    let addressed = "snap-0123456789abcdef0";
    let other = "snap-abcdef01234567890";
    let action = Op::ListSnapshotBlocks.action();

    // (case, the Resource the allow is scoped to, whether it covers the addressed snapshot)
    let rows: Vec<(&str, String, bool)> = vec![
        ("every resource", "*".to_string(), true),
        (
            "the snapshot the request addresses",
            snapshot_arn(addressed),
            true,
        ),
        (
            "every snapshot of this Region",
            snapshot_arn("*"),
            true,
        ),
        (
            "another snapshot of the same Region",
            snapshot_arn(other),
            false,
        ),
        (
            "the same snapshot id in another Region",
            format!("arn:aws:ec2:eu-west-3::snapshot/{addressed}"),
            false,
        ),
        (
            // The EC2 control plane's own snapshot ARNs carry an account segment; the
            // resource of these actions does not, so such a value covers nothing here.
            "that snapshot's ARN with an account segment",
            format!(
                "arn:aws:ec2:{REGION}:{}:snapshot/{addressed}",
                engine::emulator_account_id()
            ),
            false,
        ),
    ];

    for (case, resource, covers) in rows {
        let harness = Harness::new();
        given_session(harness.engine());
        given_inline_policy(
            harness.engine(),
            Target::Role,
            Some(&allow_on(&action, &resource)),
        );

        let reply = Op::ListSnapshotBlocks
            .call(&harness, SESSION_KEY_ID, addressed)
            .await;
        if covers {
            assert_not_refused(case, &reply);
        } else {
            assert_refused(
                case,
                &reply,
                &session_arn(),
                &action,
                &snapshot_arn(addressed),
                Refusal::DefaultDeny,
            );
        }
    }

    // `ListChangedBlocks` addresses TWO snapshots, and the action has to be allowed on both:
    // an allow covering only one of them refuses the call on the other.
    let changed = Op::ListChangedBlocks.action();
    // (case, the snapshot the allow names, the snapshot the refusal must name)
    let pairs: [(&str, &str, Option<&str>); 3] = [
        ("both snapshots allowed", "*", None),
        (
            "only the snapshot the URI names",
            addressed,
            Some(other),
        ),
        ("only the other snapshot", other, Some(addressed)),
    ];
    for (case, allowed, refused_on) in pairs {
        let harness = Harness::new();
        given_session(harness.engine());
        given_inline_policy(
            harness.engine(),
            Target::Role,
            Some(&allow_on(&changed, &snapshot_arn(allowed))),
        );
        let reply = harness
            .send_as(
                SESSION_KEY_ID,
                "GET",
                &format!("/snapshots/{addressed}/changedblocks?firstSnapshotId={other}"),
                &[],
                Vec::new(),
            )
            .await;
        match refused_on {
            None => assert_not_refused(case, &reply),
            Some(refused_on) => assert_refused(
                case,
                &reply,
                &session_arn(),
                &changed,
                &snapshot_arn(refused_on),
                Refusal::DefaultDeny,
            ),
        }
    }
}

/// RULE: the rule holds for every kind of principal a request can present — an assumed-role
/// session is judged on the ROLE's policies, an IAM user's long-term key on the USER's, and
/// the account's own published credential (a key neither store holds a record for) is not a
/// principal the IAM store holds policies for and is served.
#[tokio::test]
async fn each_kind_of_principal_is_judged_on_its_own_policies() {
    let op = Op::PutSnapshotBlock;

    // (case, the key id the call signs with, the principal a refusal names)
    let principals: [(&str, &str, Option<String>); 3] = [
        (
            "an assumed-role session",
            SESSION_KEY_ID,
            Some(session_arn()),
        ),
        ("an IAM user's long-term key", USER_KEY_ID, Some(user_arn())),
        (
            "the account's own published credential",
            AMBIENT_KEY_ID,
            None,
        ),
    ];

    for (case, key_id, principal) in principals {
        // Nothing is granted to either identity, so only the kind of principal differs.
        let harness = Harness::new();
        given_session(harness.engine());
        given_user(harness.engine());
        given_inline_policy(harness.engine(), Target::Role, None);
        given_inline_policy(harness.engine(), Target::User, None);

        let reply = op.call(&harness, key_id, ABSENT_SNAPSHOT_ID).await;
        match &principal {
            Some(principal) => assert_refused(
                case,
                &reply,
                principal,
                &op.action(),
                &op.resource(ABSENT_SNAPSHOT_ID),
                Refusal::DefaultDeny,
            ),
            None => assert_not_refused(case, &reply),
        }
    }

    // The same two identities, each with the action granted to the ROLE alone: the grant in
    // force is the one attached to the principal that signed, not to the other.
    for (case, key_id, served) in [
        ("the role granted, signed by the session", SESSION_KEY_ID, true),
        ("the role granted, signed by the user", USER_KEY_ID, false),
    ] {
        let harness = Harness::new();
        given_session(harness.engine());
        given_user(harness.engine());
        given_inline_policy(harness.engine(), Target::Role, Some(&allow(json!("ebs:*"))));
        given_inline_policy(harness.engine(), Target::User, None);

        let reply = op.call(&harness, key_id, ABSENT_SNAPSHOT_ID).await;
        if served {
            assert_not_refused(case, &reply);
        } else {
            assert_refused(
                case,
                &reply,
                &user_arn(),
                &op.action(),
                &op.resource(ABSENT_SNAPSHOT_ID),
                Refusal::DefaultDeny,
            );
        }
    }
}

/// The policy the recorded client held: the EC2 read allowed, this service's write
/// explicitly denied.
fn ec2_read_allowed_write_denied() -> Value {
    json!({
        "Statement": [
            { "Effect": "Allow", "Action": "ec2:DescribeSnapshots", "Resource": "*" },
            { "Effect": "Deny", "Action": "ebs:PutSnapshotBlock", "Resource": "*" },
        ],
    })
}

/// Start a snapshot as the account's own credential, which this gate does not bound, so only
/// the operation under test is decided by a policy.
async fn given_snapshot(harness: &Harness) -> String {
    let started = Op::StartSnapshot
        .call(harness, AMBIENT_KEY_ID, ABSENT_SNAPSHOT_ID)
        .await;
    assert_eq!(
        started.status, 201,
        "case \"the snapshot is started\": expected 201, got {} with body {:?}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let snapshot_id = member(&started, "SnapshotId");
    assert!(
        snapshot_id.starts_with("snap-"),
        "case \"the snapshot is started\": expected a minted snapshot id, got {snapshot_id:?}"
    );
    snapshot_id
}

/// `PutSnapshotBlock` of `data` at block 0 of `snapshot_id`, signed with `access_key_id`.
async fn put_block_as(
    harness: &Harness,
    access_key_id: &str,
    snapshot_id: &str,
    data: &[u8],
) -> Reply {
    harness
        .send_as(
            access_key_id,
            "PUT",
            &format!("/snapshots/{snapshot_id}/blocks/0"),
            &[
                ("content-type", "application/octet-stream".to_string()),
                ("x-amz-Data-Length", BLOCK_SIZE.to_string()),
                ("x-amz-Checksum", checksum(data)),
                ("x-amz-Checksum-Algorithm", "SHA256".to_string()),
            ],
            data.to_vec(),
        )
        .await
}

/// Seal `snapshot_id` as the account's own credential, so its blocks become readable.
async fn seal(harness: &Harness, snapshot_id: &str) {
    let sealed = harness
        .send_as(
            AMBIENT_KEY_ID,
            "POST",
            &format!("/snapshots/completion/{snapshot_id}"),
            &[("x-amz-ChangedBlocksCount", "1".to_string())],
            Vec::new(),
        )
        .await;
    assert_eq!(
        sealed.status, 202,
        "case \"the snapshot is sealed\": expected 202, got {} with body {:?}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
}

/// RULE: a refused call performs NOTHING — the denied write leaves no block behind, which
/// the sealed snapshot's own listing reports.
#[tokio::test]
async fn a_denied_write_persists_no_block() {
    let harness = Harness::new();
    given_session(harness.engine());
    given_inline_policy(
        harness.engine(),
        Target::Role,
        Some(&ec2_read_allowed_write_denied()),
    );
    let snapshot_id = given_snapshot(&harness).await;

    let refused = put_block_as(&harness, SESSION_KEY_ID, &snapshot_id, &block_bytes(7)).await;
    assert_refused(
        "the explicit deny",
        &refused,
        &session_arn(),
        "ebs:PutSnapshotBlock",
        &snapshot_arn(&snapshot_id),
        Refusal::ExplicitDeny,
    );

    seal(&harness, &snapshot_id).await;
    let listed = Op::ListSnapshotBlocks
        .call(&harness, AMBIENT_KEY_ID, &snapshot_id)
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"the refused write is listed\": expected 200, got {} with body {:?}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
    assert!(
        listed_blocks(&listed).is_empty(),
        "case \"the refused write is listed\": a denied write must persist no block, got {:?}",
        listed_blocks(&listed)
    );
}

/// RULE: the decision is re-read per request, so granting the action serves the very call
/// that was refused — and the block that call writes is then read back off the snapshot byte
/// for byte, which is what makes the served write a real one.
#[tokio::test]
async fn granting_the_write_serves_the_very_call_that_was_refused() {
    let harness = Harness::new();
    given_session(harness.engine());
    given_inline_policy(
        harness.engine(),
        Target::Role,
        Some(&ec2_read_allowed_write_denied()),
    );
    let snapshot_id = given_snapshot(&harness).await;
    let data = block_bytes(11);

    let refused = put_block_as(&harness, SESSION_KEY_ID, &snapshot_id, &data).await;
    assert_refused(
        "the explicit deny",
        &refused,
        &session_arn(),
        "ebs:PutSnapshotBlock",
        &snapshot_arn(&snapshot_id),
        Refusal::ExplicitDeny,
    );

    given_inline_policy(
        harness.engine(),
        Target::Role,
        Some(&allow(json!("ebs:PutSnapshotBlock"))),
    );
    let served = put_block_as(&harness, SESSION_KEY_ID, &snapshot_id, &data).await;
    assert_eq!(
        served.status, 201,
        "case \"the action granted\": the identical call must be served, got {} with body {:?}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    assert_eq!(
        served.header("x-amz-checksum"),
        checksum(&data),
        "case \"the action granted\": expected the checksum of the written bytes in the \
         x-amz-checksum header, got {:?}",
        served.header("x-amz-checksum")
    );

    // Read back through the service's own reads: the block is listed once, and its bytes
    // come back exactly as they were written.
    seal(&harness, &snapshot_id).await;
    let listed = Op::ListSnapshotBlocks
        .call(&harness, AMBIENT_KEY_ID, &snapshot_id)
        .await;
    let blocks = listed_blocks(&listed);
    assert_eq!(
        blocks.len(),
        1,
        "case \"the served write is listed\": expected exactly the one written block, got \
         {blocks:?}"
    );
    let (index, token) = blocks.into_iter().next().expect("the one listed block");
    assert_eq!(
        index, 0,
        "case \"the served write is listed\": expected block index 0, got {index}"
    );
    let read = harness
        .send_as(
            AMBIENT_KEY_ID,
            "GET",
            &format!("/snapshots/{snapshot_id}/blocks/0?blockToken={token}"),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the served write is read back\": expected 200, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert!(
        read.body == data,
        "case \"the served write is read back\": expected the written bytes, got {} bytes",
        read.body.len()
    );

    // Revoked again: the decision is read per request, not cached from the served one.
    given_inline_policy(harness.engine(), Target::Role, None);
    let refused_again = put_block_as(&harness, SESSION_KEY_ID, &snapshot_id, &data).await;
    assert_refused(
        "the grant revoked",
        &refused_again,
        &session_arn(),
        "ebs:PutSnapshotBlock",
        &snapshot_arn(&snapshot_id),
        Refusal::DefaultDeny,
    );
}

/// RULE: a refusal says nothing about whether the snapshot exists, so it cannot be read as
/// an answer about what the emulator holds.
#[tokio::test]
async fn a_denial_says_nothing_about_whether_the_snapshot_exists() {
    let harness = Harness::new();
    given_session(harness.engine());
    given_inline_policy(harness.engine(), Target::Role, Some(&allow(json!("ec2:*"))));

    // One snapshot the service really holds, sealed so every read path resolves it.
    let started = Op::StartSnapshot
        .call(&harness, AMBIENT_KEY_ID, ABSENT_SNAPSHOT_ID)
        .await;
    let held = member(&started, "SnapshotId");
    let sealed = harness
        .send_as(
            AMBIENT_KEY_ID,
            "POST",
            &format!("/snapshots/completion/{held}"),
            &[("x-amz-ChangedBlocksCount", "0".to_string())],
            Vec::new(),
        )
        .await;
    assert_eq!(
        sealed.status, 202,
        "case \"the held snapshot is sealed\": expected 202, got {}",
        sealed.status
    );

    for op in EVERY_OP {
        if op == Op::StartSnapshot {
            // The create addresses no existing snapshot, so there is no such pair for it.
            continue;
        }
        let on_held = op.call(&harness, SESSION_KEY_ID, &held).await;
        let on_absent = op.call(&harness, SESSION_KEY_ID, ABSENT_SNAPSHOT_ID).await;
        assert_refused(
            &format!("{} / a snapshot the service holds", op.name()),
            &on_held,
            &session_arn(),
            &op.action(),
            &snapshot_arn(&held),
            Refusal::DefaultDeny,
        );
        assert_refused(
            &format!("{} / a snapshot nothing holds", op.name()),
            &on_absent,
            &session_arn(),
            &op.action(),
            &snapshot_arn(ABSENT_SNAPSHOT_ID),
            Refusal::DefaultDeny,
        );
        let held_body = String::from_utf8_lossy(&on_held.body).replace(held.as_str(), "<snapshot>");
        let absent_body =
            String::from_utf8_lossy(&on_absent.body).replace(ABSENT_SNAPSHOT_ID, "<snapshot>");
        assert_eq!(
            held_body,
            absent_body,
            "case {:?}: the two refusals must not differ by anything but the snapshot they \
             name, got {held_body} and {absent_body}",
            op.name()
        );
    }
}
