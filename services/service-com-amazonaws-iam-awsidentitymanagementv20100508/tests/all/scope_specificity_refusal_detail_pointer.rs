// SPDX-License-Identifier: CC0-1.0

//! Whether a role SESSION's refusal for want of an identity-based grant closes with the
//! pointer at the authorization detail of its decision, decided by WHICH grant the decision
//! read about the action.
//!
//! The rule under test: such a refusal is the sentence alone exactly when the decision was
//! already explained by a grant that is not about that very action — one of the ROLE's own
//! documents stating an `Allow` covering it, which was read and did not apply (its
//! `Condition` did not hold, its `Resource` did not cover the one addressed), or a session
//! policy allowing it WITHOUT spelling it, a scope written about everything the session may
//! do. It closes with the pointer whenever the only grant about the action is a session
//! policy that SPELLS it — a scope written about that action, which grants nothing of its
//! own, so the role's silence is what withheld the call — and whenever no readable `Allow`
//! was about the action at all.
//!
//! No trait of the model governs the text of an `AccessDenied` — iam#ListUsers models only
//! `ServiceFailureException`
//! (`smithy_query(query="operation",operation="ListUsers")`) — so this is recorded behaviour
//! throughout: the sentence itself is the table in `tests/all/access_denied_message_wording.rs`,
//! `tests/all/session_refusal_authorization_detail_pointer.rs` is the table for the line drawn
//! at the CREDENTIAL, and this suite is the table for the line drawn at the grant's
//! specificity.
//!
//! The rows differ along what the rule generalises over: how the scope names the action (the
//! action itself, among several values, case-folded, every action of the service, every action
//! there is, a prefix run), which form the mint named the scope in (inline, managed), which
//! operation is refused and the resource form its request addresses (a collection, one named
//! entity), and where the grant the decision read sits (the scope, the role's own document,
//! nowhere).
//!
//! The awsQuery harness and the session fixture are the ones the suites beside this file own,
//! so a request is built, signed and read exactly one way.

use crate::access_denied_message_wording::{Scoped, session_scoped_caller};
use crate::identity_policy_enforcement::statement;
use crate::permissions_boundary_ceiling::statements;
use crate::role_lifecycle::borrowed;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, assert_access_denied_sentence, call_signed_by,
    element, error_code, router_with_engine,
};

/// The reason clause of a call the identity's own documents did not allow — the refusal every
/// row here reaches, a session policy granting nothing of its own.
fn no_identity_allow(iam_action: &str) -> String {
    format!("because no identity-based policy allows the {iam_action} action")
}

/// An instant every call this suite makes is made BEFORE, so a `DateGreaterThan` clause over
/// it does not hold at the instant of the call: such a statement is in force, spells the
/// action, and does not apply.
const FUTURE_INSTANT: &str = "2099-01-01T00:00:00Z";

/// A document whose one statement `Allow`s `action` on every resource from `instant` onwards.
fn grant_from(action: &str, instant: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{action}","Resource":"*","Condition":{{"DateGreaterThan":{{"aws:CurrentTime":"{instant}"}}}}}}]}}"#
    )
}

/// One row: the documents in force on the session, the call it makes, and whether the refusal
/// closes with the pointer at the detail of its decision.
struct Row {
    case: &'static str,
    /// The inline document put on the role the session is of.
    role_policy: String,
    /// What the mint scoped the session's credential with.
    scope: Scoped,
    /// The awsQuery `Action` the session calls, and the IAM action it is decided on.
    action: &'static str,
    iam_action: &'static str,
    /// Whether the call addresses the role its own session is of — the one entity each row's
    /// fixture creates — which the refusal then names as the resource.
    addresses_its_own_role: bool,
    /// The resource as the refusal NAMES it, for a call that addresses a collection.
    named_as: String,
    /// Whether the message closes with the pointer at the detail of the decision.
    points_at_its_detail: bool,
}

impl Row {
    /// The resource the refusal of this row names, for a session of `role_name`.
    fn names(&self, role_name: &str) -> String {
        if self.addresses_its_own_role {
            return format!("role {role_name}");
        }
        self.named_as.clone()
    }

    /// The form parameters this row's call carries.
    fn parameters(&self, role_name: &str) -> Vec<(String, String)> {
        if self.addresses_its_own_role {
            return vec![("RoleName".to_string(), role_name.to_string())];
        }
        Vec::new()
    }
}

/// The rows the rule runs over.
fn rows() -> Vec<Row> {
    let account = account_id();
    let user_root = format!("arn:aws:iam::{account}:user/");
    let role_root = format!("arn:aws:iam::{account}:role/");
    let group_root = format!("arn:aws:iam::{account}:group/");
    let another_user = format!("arn:aws:iam::{account}:user/elsewhere");
    vec![
        // The scope SPELLS the action among the actions it names, the role naming another:
        // the only grant about the call is a session policy, which grants nothing, so the
        // role's silence withheld it and the refusal points at its detail.
        Row {
            case: "a session policy spelling the action beside another",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Inline(statements(&[
                ("Allow", "iam:GetRole", "*"),
                ("Allow", "iam:ListRolePolicies", "*"),
            ])),
            action: "ListRolePolicies",
            iam_action: "iam:ListRolePolicies",
            addresses_its_own_role: true,
            named_as: String::new(),
            points_at_its_detail: true,
        },
        // The scope spelling the action and NOTHING else, on a call addressing a collection:
        // another operation, another resource form, the same family.
        Row {
            case: "a session policy spelling the action alone",
            role_policy: statement("Allow", "iam:GetRole", "*"),
            scope: Scoped::Inline(statement("Allow", "iam:ListUsers", "*")),
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            points_at_its_detail: true,
        },
        // The same through a MANAGED session policy, so the form the mint named the scope in
        // moves nothing.
        Row {
            case: "a managed session policy spelling the action",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Managed(statement("Allow", "iam:ListRoles", "*")),
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root.clone(),
            points_at_its_detail: true,
        },
        // The scope spelling the action CASE-FOLDED: IAM compares an action name without
        // regard to case, so this is the same grant spelled differently.
        Row {
            case: "a session policy spelling the action in another case",
            role_policy: statement("Allow", "iam:GetRole", "*"),
            scope: Scoped::Inline(statement("Allow", "iam:listgroups", "*")),
            action: "ListGroups",
            iam_action: "iam:ListGroups",
            addresses_its_own_role: false,
            named_as: group_root,
            points_at_its_detail: true,
        },
        // The scope spelling the action beside a wildcard over ANOTHER service: the wildcard
        // is about no IAM action, so the grant read about this call is the one that spells it.
        Row {
            case: "a session policy spelling the action beside another service's wildcard",
            role_policy: statement("Allow", "iam:GetRole", "*"),
            scope: Scoped::Inline(statements(&[
                ("Allow", "s3:*", "*"),
                ("Allow", "iam:ListRoles", "*"),
            ])),
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root.clone(),
            points_at_its_detail: true,
        },
        // NOTHING in force is about the action — the narrow role a workload carries, scoped
        // by no session policy at all: the refusal points at its detail.
        Row {
            case: "a role granting another service's action only, no scope",
            role_policy: statement("Allow", "cloudwatch:ListMetrics", "*"),
            scope: Scoped::Nothing,
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            points_at_its_detail: true,
        },
        // The scope allowing EVERY action there is: a grant written about everything the
        // session may do, which spells no action, so the refusal is the sentence alone.
        Row {
            case: "a session policy allowing every action there is",
            role_policy: statement("Allow", "s3:ListAllMyBuckets", "*"),
            scope: Scoped::Inline(statement("Allow", "*", "*")),
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            points_at_its_detail: false,
        },
        // The scope allowing every action of the SERVICE: the same wholesale grant, one
        // namespace narrower.
        Row {
            case: "a session policy allowing every action of the service",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Inline(statement("Allow", "iam:*", "*")),
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root,
            points_at_its_detail: false,
        },
        // The scope allowing a PREFIX run of actions: it covers the call without spelling it,
        // so it is the same family as the two above.
        Row {
            case: "a session policy allowing a prefix run of actions",
            role_policy: statement("Allow", "iam:GetRole", "*"),
            scope: Scoped::Inline(statement("Allow", "iam:List*", "*")),
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            points_at_its_detail: false,
        },
        // The ROLE's own document spells the action and does not apply — a date clause that
        // has not come true — with no scope at all: the grant the decision read is the
        // identity's own, so the refusal is the sentence alone.
        Row {
            case: "a role grant spelling the action that is not yet in effect",
            role_policy: grant_from("iam:ListUsers", FUTURE_INSTANT),
            scope: Scoped::Nothing,
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            points_at_its_detail: false,
        },
        // The role's own document spells the action on ANOTHER resource while the scope
        // spells it too: the identity's own grant was read, which is what the wording turns
        // on, whatever the scope says.
        Row {
            case: "a role grant spelling the action on another resource, the scope spelling it",
            role_policy: statement("Allow", "iam:ListUsers", &another_user),
            scope: Scoped::Inline(statement("Allow", "iam:ListUsers", "*")),
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root,
            points_at_its_detail: false,
        },
    ]
}

/// RULE: a session's refusal for want of an identity-based grant closes with the pointer at
/// the detail of its decision exactly when the grant the decision read about the action is a
/// session policy SPELLING it, or when no `Allow` was about the action at all; it is the
/// sentence alone when the grant read was the identity's own, or a scope that allows the call
/// without spelling it.
#[tokio::test]
async fn which_grant_the_decision_read_decides_whether_the_refusal_points_at_its_detail() {
    let (router, engine) = router_with_engine();

    for (index, row) in rows().into_iter().enumerate() {
        let role_name = format!("specific{index}");
        let (access_key_id, session_arn) = session_scoped_caller(
            &router,
            &engine,
            row.case,
            &role_name,
            &row.role_policy,
            &row.scope,
        )
        .await;
        let params = row.parameters(&role_name);

        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), row.action, &borrowed(&params)).await;
        assert_eq!(
            status, 403,
            "case {:?}: expected HTTP 403, got {status}: {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {:?}: expected the error code AccessDenied, got {:?}",
            row.case,
            error_code(&body)
        );

        let message = element(&body, "Message");
        let named_as = row.names(&role_name);
        let reason = no_identity_allow(row.iam_action);
        if row.points_at_its_detail {
            assert_access_denied_message(
                row.case,
                &message,
                &session_arn,
                row.iam_action,
                &named_as,
                &reason,
            );
        } else {
            assert_access_denied_sentence(
                row.case,
                &message,
                &session_arn,
                row.iam_action,
                &named_as,
                &reason,
            );
        }
    }
}

/// RULE: the wording turns on the DECISION and not on the fixture — the call both the role's
/// documents and a scope spelling it allow is served, while the one only the scope spells is
/// refused and points at its detail.
#[tokio::test]
async fn the_call_the_intersection_allows_is_served_beside_the_one_only_the_scope_spells() {
    let (router, engine) = router_with_engine();
    let role_name = "intersected";
    let case = "a scope spelling two actions, the role allowing one of them";
    let (access_key_id, session_arn) = session_scoped_caller(
        &router,
        &engine,
        case,
        role_name,
        &statement("Allow", "iam:ListRoleTags", "*"),
        &Scoped::Inline(statements(&[
            ("Allow", "iam:ListRoleTags", "*"),
            ("Allow", "iam:ListRolePolicies", "*"),
        ])),
    )
    .await;

    let (status, served) = call_signed_by(
        &router,
        Some(&access_key_id),
        "ListRoleTags",
        &[("RoleName", role_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the call both documents allow must be served, got {status}: {served}"
    );

    let (status, body) = call_signed_by(
        &router,
        Some(&access_key_id),
        "ListRolePolicies",
        &[("RoleName", role_name)],
    )
    .await;
    assert_eq!(
        status, 403,
        "case {case:?}: the call only the scope spells must be refused, got {status}: {body}"
    );
    assert_access_denied_message(
        case,
        &element(&body, "Message"),
        &session_arn,
        "iam:ListRolePolicies",
        &format!("role {role_name}"),
        &no_identity_allow("iam:ListRolePolicies"),
    );
}
