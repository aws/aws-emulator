// SPDX-License-Identifier: CC0-1.0

//! What the CREDENTIAL a `PutAuditEvents` request signed with was scoped to, and the ceiling
//! in force on the principal it speaks for, driven end to end over the real wire path.
//!
//! THE RULE UNDER TEST: the call is decided against the EFFECTIVE permissions of the
//! principal its credentials name — the identity-based policy documents in force on that
//! principal, INTERSECTED with the session policies the mint of that credential passed ("the
//! resulting session's permissions are the intersection of the role's identity-based policy
//! and the session policies … you cannot use session policies to grant more permissions than
//! those allowed by the identity-based policy of the role that is being assumed",
//! `AssumeRoleRequest$PolicyArns`) and with the permissions boundary that caps them ("a
//! permissions boundary sets the maximum permissions that an identity-based policy can
//! grant"). Each cap takes permissions away and grants none of its own, so a call a cap does
//! not allow is refused even where the identity's own documents allow it, and a call only a
//! cap allows is refused as if that cap had allowed nothing. The refusal is the authorization
//! layer's HTTP 403 `AccessDeniedException` naming the principal, the action under this
//! service's own namespace and the channel the request carried, closing on the document that
//! withheld the grant — a different fact, needing a different fix, from an explicit `Deny` in
//! it — and it carries neither `successful` nor `failed` and stores nothing.
//!
//! The sibling suite `identity_authorization.rs` pins the identity-based half of the same
//! decision; the role, the session, their policies and the managed policies a scope or a
//! boundary names live in the IAM and STS stores of the SAME shared engine, so what is under
//! test is what `iam:PutRolePolicy`, `iam:CreatePolicy` and `sts:AssumeRole` record.
//!
//! The tables span what the rule generalises over: how the scope was passed (not at all, as
//! an inline document, as a managed policy ARN, as an ARN naming a policy this account does
//! not hold), what it says (this action, this service's whole namespace, every action, the
//! CloudTrail control plane's namespace, this operation's name under that wrong namespace,
//! another service, an allow beside an explicit deny), which resource it scopes the grant to
//! (every resource, the channel the request names, another channel), where the grant it caps
//! comes from (the role's own policy, nowhere), when the document it stands for was written
//! (before the mint, and after it), and the same questions for the boundary.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    AttachedPermissionsBoundary, Policy, PolicyVersion,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::identity_authorization::{
    allow, assert_refused, given_role_policy, given_scoped_session, role_arn, session_arn,
    Refusal, ACTION, GOOD_EVENT, SESSION_KEY_ID,
};
use crate::put_audit_events::{channel_arn, ev, Harness};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// The channel every row addresses, registered here so an allowed call reaches the ingest and
/// a refused one can be told apart from a resolution failure.
const ADDRESSED: &str = "8c7b6a59-4837-4261-95f4-e3d2c1b0a998";

/// Another channel of the same account and Region, for a scope that names the wrong one.
const OTHER: &str = "9d8c7b6a-5948-4372-86a5-f4e3d2c1b0a9";

/// The name the account holds the seeded managed policy under.
const MANAGED: &str = "audit-event-scope";

/// A policy allowing this operation's action on `resource` alone.
fn allow_on(resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": ACTION, "Resource": resource }]
    })
}

/// A policy allowing this operation's action and denying it in the same breath — the pair
/// that makes an explicit deny decide the call.
fn allow_and_deny() -> Value {
    json!({
        "Statement": [
            { "Effect": "Allow", "Action": ACTION, "Resource": "*" },
            { "Effect": "Deny", "Action": ACTION, "Resource": "*" },
        ]
    })
}

/// The session policies of a mint that passed `document` as its INLINE session policy, which
/// is `AssumeRoleRequest$Policy`.
fn inline_scope(document: &Value) -> sts::SessionPolicies {
    sts::SessionPolicies {
        inline: Some(document.to_string()),
        managed_arns: Vec::new(),
        ..sts::SessionPolicies::default()
    }
}

/// The session policies of a mint that named managed policies by ARN, which is
/// `AssumeRoleRequest$PolicyArns`.
fn managed_scope(policy_arns: &[String]) -> sts::SessionPolicies {
    sts::SessionPolicies {
        inline: None,
        managed_arns: policy_arns.to_vec(),
        ..sts::SessionPolicies::default()
    }
}

/// Record a customer-managed policy named `name` whose DEFAULT version carries `document`, as
/// `iam:CreatePolicy` records one, and answer the ARN the account holds it under. Called again
/// with the same name, it replaces that version's document — what `iam:CreatePolicyVersion`
/// plus `iam:SetDefaultPolicyVersion` leave behind.
fn given_managed_policy(
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    document: &Value,
) -> String {
    let arn = iam::policy_arn("/", name);
    let version = iam::FIRST_POLICY_VERSION_ID.to_string();
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().policies.insert(
        iam::name_key(name),
        iam::ManagedPolicy {
            policy: Policy {
                policy_name: Some(name.to_string()),
                policy_id: Some("ANPAEXAMPLECTDATASCOPE1".to_string()),
                arn: Some(arn.clone()),
                path: Some("/".to_string()),
                default_version_id: Some(version.clone()),
                is_attachable: true,
                ..Policy::default()
            },
            versions: BTreeMap::from([(
                version.clone(),
                PolicyVersion {
                    document: Some(document.to_string()),
                    version_id: Some(version),
                    is_default_version: true,
                    ..PolicyVersion::default()
                },
            )]),
        },
    );
    arn
}

/// Set the managed policy `policy_arn` names as the role's permissions boundary, as
/// `iam:PutRolePermissionsBoundary` does; `None` takes the boundary off it.
fn given_role_boundary(engine: &Arc<engine::InMemoryEngine>, policy_arn: Option<&str>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    let role = store
        .roles
        .get_mut(&iam::name_key(&role_name()))
        .expect("the seeded role");
    role.permissions_boundary = policy_arn.map(|arn| AttachedPermissionsBoundary {
        permissions_boundary_type: Some(iam::PERMISSIONS_BOUNDARY_TYPE.to_string()),
        permissions_boundary_arn: Some(arn.to_string()),
    });
}

/// The name of the role the seeded session is a session of, read out of the ARN the suite
/// holds it under so the two cannot disagree.
fn role_name() -> String {
    let arn = role_arn();
    arn.rsplit('/')
        .next()
        .expect("the role ARN names a role")
        .to_string()
}

/// A fresh engine carrying the role, a session of it scoped with `scope`, the inline policy
/// `granted` on the role, and the addressed channel registered.
fn given(scope: sts::SessionPolicies, granted: Option<&Value>) -> (Harness, String) {
    let harness = Harness::new();
    given_scoped_session(harness.engine(), scope);
    given_role_policy(harness.engine(), granted);
    let arn = channel_arn(ADDRESSED);
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));
    (harness, arn)
}

/// One row of the managed-session-policy table: the document the account's held policy
/// carries (or `None` to hold no such policy at all), the ARNs the mint named, and which of
/// IAM's refusals the call then meets.
struct ManagedScope {
    case: &'static str,
    document: Option<Value>,
    named: Vec<String>,
    refusal: Option<Refusal>,
}

/// One row of the permissions-boundary table: the document the boundary policy carries (or
/// `None` to hold no such policy), the boundary ARN set on the role, and the refusal.
struct Ceiling {
    case: &'static str,
    document: Option<Value>,
    boundary: String,
    refusal: Option<Refusal>,
}

/// One row of the table over WHICH document the decision rested on: what each of the three
/// documents in force says about the action, and the refusal that names the deciding one.
struct Documents {
    case: &'static str,
    /// The role's own inline policy.
    granted: Value,
    /// The inline session policy the mint passed.
    scope: Value,
    /// The document the managed policy the role's boundary names carries.
    ceiling: Value,
    refusal: Refusal,
}

/// Drive the one call every row makes: the same single-event batch on the addressed channel,
/// signed with the seeded session's access key id.
async fn put(harness: &Harness, arn: &str) -> crate::put_audit_events::Reply {
    harness
        .put_as(SESSION_KEY_ID, arn, None, &[ev("evt-1", GOOD_EVENT)])
        .await
}

/// Assert the row's outcome: either the service's own verdict for an allowed call (HTTP 200,
/// the event stored once) or the authorization layer's refusal, which stores nothing.
async fn assert_outcome(case: &str, harness: &Harness, arn: &str, refused: Option<Refusal>) {
    let reply = put(harness, arn).await;
    match refused {
        Some(refusal) => {
            assert_refused(case, &reply, &session_arn(), arn, refusal);
            assert!(
                harness.ingest_log(arn).is_empty(),
                "case {case:?}: a denied call must persist nothing, got {:?}",
                harness.ingest_log(arn)
            );
        }
        None => {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the effective permissions allow this call, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                harness.ingest_log(arn).len(),
                1,
                "case {case:?}: the served call must be stored once, got {:?}",
                harness.ingest_log(arn)
            );
        }
    }
}

/// THE RULE: the session policies the credential was minted with CAP what the role's own
/// policy grants. The role allows the action on every resource in every row, so only the
/// scope decides, and a refusal closes on the scope rather than on the role's policy.
#[tokio::test]
async fn an_inline_session_policy_caps_what_the_role_grants() {
    let granted = allow(json!(ACTION));
    // (case, the inline session policy the mint passed, which refusal the call meets)
    let rows: Vec<(&str, Option<Value>, Option<Refusal>)> = vec![
        ("no session policy at all", None, None),
        (
            "a scope allowing exactly this action",
            Some(allow(json!(ACTION))),
            None,
        ),
        (
            "a scope allowing every action of this service",
            Some(allow(json!("cloudtrail-data:*"))),
            None,
        ),
        ("a scope allowing every action", Some(allow(json!("*"))), None),
        (
            "a scope allowing this action in lower case",
            Some(allow(json!(ACTION.to_lowercase()))),
            None,
        ),
        (
            // The namespace of the CloudTrail control plane, which publishes no action of
            // this service.
            "a scope allowing every action of the CloudTrail control plane",
            Some(allow(json!("cloudtrail:*"))),
            Some(Refusal::SessionWithheld),
        ),
        (
            "a scope allowing this operation's name under the control plane's namespace",
            Some(allow(json!("cloudtrail:PutAuditEvents"))),
            Some(Refusal::SessionWithheld),
        ),
        (
            "a scope allowing another service entirely",
            Some(allow(json!("s3:*"))),
            Some(Refusal::SessionWithheld),
        ),
        (
            "a scope scoped to the channel the request names",
            Some(allow_on(&channel_arn(ADDRESSED))),
            None,
        ),
        (
            "a scope scoped to another channel of the same account",
            Some(allow_on(&channel_arn(OTHER))),
            Some(Refusal::SessionWithheld),
        ),
        (
            "a scope denying this action beside allowing it",
            Some(allow_and_deny()),
            Some(Refusal::SessionExplicitDeny),
        ),
        (
            "a scope denying this whole service beside allowing the action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": ACTION, "Resource": "*" },
                    { "Effect": "Deny", "Action": "cloudtrail-data:*", "Resource": "*" },
                ],
            })),
            Some(Refusal::SessionExplicitDeny),
        ),
    ];

    for (case, scope, refused) in rows {
        let passed = scope.as_ref().map(inline_scope).unwrap_or_default();
        let (harness, arn) = given(passed, Some(&granted));
        assert_outcome(case, &harness, &arn, refused).await;
    }
}

/// RULE: a session policy grants NOTHING of its own, so however wide it is the call is still
/// refused where the principal's own documents do not allow it — and the refusal names the
/// document that withheld the grant, which is then the role's policy and not the scope.
#[tokio::test]
async fn a_session_policy_grants_nothing_of_its_own() {
    let widest = allow(json!("*"));
    // (case, the role's own inline policy, which refusal the call meets)
    let rows: Vec<(&str, Option<Value>, Option<Refusal>)> = vec![
        ("the role carries no policy", None, Some(Refusal::DefaultDeny)),
        (
            "the role is granted the control plane's namespace only",
            Some(allow(json!("cloudtrail:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "the role denies the action beside allowing it",
            Some(allow_and_deny()),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "the role is granted the action",
            Some(allow(json!(ACTION))),
            None,
        ),
    ];

    for (case, granted, refused) in rows {
        let (harness, arn) = given(inline_scope(&widest), granted.as_ref());
        assert_outcome(case, &harness, &arn, refused).await;
    }
}

/// RULE: a managed session policy named by ARN narrows exactly as an inline document does,
/// by what its DEFAULT version says at the instant of the request — and an ARN naming a
/// policy this account does not hold is a scope that cannot be read, which caps nothing
/// rather than refusing on evidence the emulator does not have.
#[tokio::test]
async fn a_managed_session_policy_scopes_by_the_document_in_force() {
    let granted = allow(json!(ACTION));
    let held = iam::policy_arn("/", MANAGED);
    let absent = iam::policy_arn("/", "no-such-scope");

    let rows: Vec<ManagedScope> = vec![
        ManagedScope {
            case: "a managed scope allowing this action",
            document: Some(allow(json!(ACTION))),
            named: vec![held.clone()],
            refusal: None,
        },
        ManagedScope {
            case: "a managed scope allowing another service",
            document: Some(allow(json!("s3:*"))),
            named: vec![held.clone()],
            refusal: Some(Refusal::SessionWithheld),
        },
        ManagedScope {
            case: "a managed scope denying this action",
            document: Some(allow_and_deny()),
            named: vec![held.clone()],
            refusal: Some(Refusal::SessionExplicitDeny),
        },
        ManagedScope {
            case: "a managed scope this account does not hold",
            document: None,
            named: vec![absent.clone()],
            refusal: None,
        },
        ManagedScope {
            case: "a managed scope this account does not hold beside one that denies the action",
            document: Some(allow_and_deny()),
            named: vec![absent.clone(), held.clone()],
            refusal: Some(Refusal::SessionExplicitDeny),
        },
    ];

    for row in rows {
        let (harness, arn) = given(managed_scope(&row.named), Some(&granted));
        if let Some(document) = &row.document {
            given_managed_policy(harness.engine(), MANAGED, document);
        }
        assert_outcome(row.case, &harness, &arn, row.refusal).await;
    }

    // The document is read at the instant of the REQUEST, not at the instant of the mint: the
    // very session that was served is refused once the policy's default version no longer
    // allows the action.
    let (harness, arn) = given(managed_scope(&[held]), Some(&granted));
    given_managed_policy(harness.engine(), MANAGED, &allow(json!(ACTION)));
    let served = put(&harness, &arn).await;
    assert_eq!(
        served.status, 200,
        "case \"the scope allows the action\": the call must be served, got {} with body {}",
        served.status, served.body
    );
    given_managed_policy(harness.engine(), MANAGED, &allow(json!("cloudtrail:*")));
    let refused = put(&harness, &arn).await;
    assert_refused(
        "the scope rewritten to the control plane's namespace",
        &refused,
        &session_arn(),
        &arn,
        Refusal::SessionWithheld,
    );
    assert_eq!(
        harness.ingest_log(&arn).len(),
        1,
        "case \"the scope rewritten to the control plane's namespace\": the refused call must \
         store nothing more, got {:?}",
        harness.ingest_log(&arn)
    );
}

/// RULE: the permissions boundary in force on the principal caps the grant the same way, and
/// is read the same way — by the default version of the managed policy it names, with a
/// boundary whose document this account does not hold capping nothing.
#[tokio::test]
async fn the_permissions_boundary_caps_the_grant_too() {
    let granted = allow(json!(ACTION));
    let held = iam::policy_arn("/", MANAGED);
    let absent = iam::policy_arn("/", "no-such-boundary");

    let rows: Vec<Ceiling> = vec![
        Ceiling {
            case: "a boundary allowing this action",
            document: Some(allow(json!(ACTION))),
            boundary: held.clone(),
            refusal: None,
        },
        Ceiling {
            case: "a boundary allowing every action of this service",
            document: Some(allow(json!("cloudtrail-data:*"))),
            boundary: held.clone(),
            refusal: None,
        },
        Ceiling {
            case: "a boundary allowing the control plane's namespace only",
            document: Some(allow(json!("cloudtrail:*"))),
            boundary: held.clone(),
            refusal: Some(Refusal::BoundaryWithheld),
        },
        Ceiling {
            case: "a boundary scoped to another channel of the same account",
            document: Some(allow_on(&channel_arn(OTHER))),
            boundary: held.clone(),
            refusal: Some(Refusal::BoundaryWithheld),
        },
        Ceiling {
            case: "a boundary denying this action",
            document: Some(allow_and_deny()),
            boundary: held.clone(),
            refusal: Some(Refusal::BoundaryExplicitDeny),
        },
        Ceiling {
            case: "a boundary this account does not hold",
            document: None,
            boundary: absent.clone(),
            refusal: None,
        },
    ];

    for row in rows {
        let (harness, arn) = given(sts::SessionPolicies::default(), Some(&granted));
        if let Some(document) = &row.document {
            given_managed_policy(harness.engine(), MANAGED, document);
        }
        given_role_boundary(harness.engine(), Some(&row.boundary));
        assert_outcome(row.case, &harness, &arn, row.refusal).await;
    }

    // A role carrying no boundary at all is capped by nothing, which is the control for every
    // row above.
    let (harness, arn) = given(sts::SessionPolicies::default(), Some(&granted));
    given_role_boundary(harness.engine(), None);
    assert_outcome("the role carries no boundary", &harness, &arn, None).await;
}

/// RULE: an explicit `Deny` in a document that can be read refuses the call whatever any
/// grant says, and the refusal names the DOCUMENT the deny was stated in — the identity's own
/// policies first, then the scope the credential was minted with, then the boundary — because
/// each needs a different fix.
#[tokio::test]
async fn the_refusal_names_the_document_the_decision_rested_on() {
    let denied = allow_and_deny();
    let boundary = iam::policy_arn("/", MANAGED);

    let rows: Vec<Documents> = vec![
        Documents {
            case: "every document denies the action",
            granted: denied.clone(),
            scope: denied.clone(),
            ceiling: denied.clone(),
            refusal: Refusal::ExplicitDeny,
        },
        Documents {
            case: "the scope and the boundary deny it",
            granted: allow(json!(ACTION)),
            scope: denied.clone(),
            ceiling: denied.clone(),
            refusal: Refusal::SessionExplicitDeny,
        },
        Documents {
            case: "the boundary alone denies it",
            granted: allow(json!(ACTION)),
            scope: allow(json!(ACTION)),
            ceiling: denied.clone(),
            refusal: Refusal::BoundaryExplicitDeny,
        },
        Documents {
            case: "nothing denies it and nothing but the scope and the boundary allow it",
            granted: allow(json!("cloudtrail:*")),
            scope: allow(json!(ACTION)),
            ceiling: allow(json!(ACTION)),
            refusal: Refusal::DefaultDeny,
        },
        Documents {
            case: "the role allows it and the scope states nothing about it",
            granted: allow(json!(ACTION)),
            scope: allow(json!("cloudtrail:*")),
            ceiling: allow(json!(ACTION)),
            refusal: Refusal::SessionWithheld,
        },
        Documents {
            case: "the role and the scope allow it and the boundary states nothing",
            granted: allow(json!(ACTION)),
            scope: allow(json!(ACTION)),
            ceiling: allow(json!("cloudtrail:*")),
            refusal: Refusal::BoundaryWithheld,
        },
    ];

    for row in rows {
        let (harness, arn) = given(inline_scope(&row.scope), Some(&row.granted));
        given_managed_policy(harness.engine(), MANAGED, &row.ceiling);
        given_role_boundary(harness.engine(), Some(&boundary));
        assert_outcome(row.case, &harness, &arn, Some(row.refusal)).await;
    }
}
