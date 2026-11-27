// SPDX-License-Identifier: CC0-1.0

//! WHICH refused IAM calls close with the pointer at the authorization detail of their
//! decision, and which are the sentence alone.
//!
//! The rule under test: a refusal of a request signed with a role SESSION's temporary
//! credential, reached although a grant the decision read is not about the very action asked
//! for — the role's own identity-based documents stating an `Allow` covering it, or a session
//! policy the mint passed allowing it WITHOUT spelling it — is answered as the decision's
//! sentence and nothing else. The grant was read and did not apply (the session-policy
//! intersection withheld it, a condition the session does not satisfy, or a resource it does
//! not cover), and those are the refusals whose recordings carry no pointer. Every OTHER
//! refusal this gate words closes with it: one no readable `Allow` about the action was in
//! force for, one a statement about the request explicitly denied, one withheld by the session
//! policies rather than by the identity's own documents, one whose only grant about the action
//! is a session policy SPELLING it (`tests/all/scope_specificity_refusal_detail_pointer.rs` is
//! the table for that line), and every refusal of a LONG-TERM credential — including the
//! long-term caller whose own grant names the action and does not apply, which is the shape
//! the session rows here are answered WITHOUT a pointer, and so is why the line is drawn at
//! the credential and not at the clause alone.
//!
//! No trait of the model governs the text of an `AccessDenied` — iam#ListUsers models only
//! `ServiceFailureException` (`smithy_query(query="operation",operation="ListUsers")`) — so
//! this is recorded behaviour throughout: the sentence itself is the table in
//! `tests/all/access_denied_message_wording.rs`, and this suite is the table for which family
//! a refusal of a session belongs to when the grant the decision read was the identity's own
//! or a wholesale scope.
//!
//! The rows differ along what the rule generalises over: which operation is refused and the
//! resource form its request addresses (a collection, a path-scoped collection, one named
//! entity), WHERE the grant naming the action sits (a session policy, the identity's own
//! document), why it did not apply (the intersection, an unsatisfied condition, another
//! resource), which document decided the refusal, and which kind of credential signed it.
//!
//! The awsQuery harness and the session fixture are the ones the suites beside this file own,
//! so a request is built, signed and read exactly one way.

use crate::access_denied_message_wording::{Scoped, session_scoped_caller};
use crate::identity_policy_enforcement::{principal, statement};
use crate::permissions_boundary_ceiling::statements;
use crate::role_lifecycle::borrowed;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, assert_access_denied_sentence, call_signed_by,
    element, error_code, router, router_with_engine,
};

/// The reason clause of a call nothing in force allowed — the implicit identity-based deny.
fn no_identity_allow(iam_action: &str) -> String {
    format!("because no identity-based policy allows the {iam_action} action")
}

/// The reason clause of a call the identity's documents allowed and the session policies in
/// force did not.
fn no_session_allow(iam_action: &str) -> String {
    format!("because no session policy allows the {iam_action} action")
}

/// The reason clause of a call a statement of an identity-based document denied.
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The reason clause of a call a statement of a session policy denied.
const SESSION_DENY: &str = "with an explicit deny in a session policy";

/// An instant every call this suite makes is made BEFORE, so a `DateGreaterThan` clause over
/// it does not hold at the instant of the call: such a statement is in force, names the
/// action, and does not apply.
const FUTURE_INSTANT: &str = "2099-01-01T00:00:00Z";

/// A document whose one statement `Allow`s `action` on every resource from `instant` onwards.
fn grant_from(action: &str, instant: &str) -> String {
    let condition = format!(r#"{{"DateGreaterThan":{{"aws:CurrentTime":"{instant}"}}}}"#);
    conditioned_grant(action, &condition)
}

/// A document whose one statement `Allow`s `action` on every resource only for a principal
/// carrying the tag `key`=`value` — a grant the sessions below, which carry no tags at all,
/// do not satisfy.
fn grant_to_tagged(action: &str, key: &str, value: &str) -> String {
    let condition = format!(r#"{{"StringEquals":{{"aws:PrincipalTag/{key}":"{value}"}}}}"#);
    conditioned_grant(action, &condition)
}

/// A document whose one statement `Allow`s `action` on every resource under `condition`.
fn conditioned_grant(action: &str, condition: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{action}","Resource":"*","Condition":{condition}}}]}}"#
    )
}

/// One row: the documents in force on the session, the call it makes, and the message the
/// refusal must be — the sentence, and whether the pointer closes it.
struct Row {
    case: &'static str,
    /// The inline document put on the role the session is of.
    role_policy: String,
    /// What the mint scoped the session's credential with.
    scope: Scoped,
    /// The awsQuery `Action` the session calls and the IAM action the decision is made on.
    action: &'static str,
    /// The call's form parameters, besides the `RoleName` of [`Row::addresses_its_own_role`].
    params: Vec<(String, String)>,
    iam_action: &'static str,
    /// Whether the call addresses the role its own session is of — the one entity each row's
    /// fixture creates — which the refusal then names as the resource.
    addresses_its_own_role: bool,
    /// The resource as the refusal NAMES it, for a call that addresses a collection.
    named_as: String,
    /// The clause closing the sentence.
    reason: String,
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

    /// The form parameters this row's call carries, for a session of `role_name`.
    fn parameters(&self, role_name: &str) -> Vec<(String, String)> {
        let mut params = self.params.clone();
        if self.addresses_its_own_role {
            params.push(("RoleName".to_string(), role_name.to_string()));
        }
        params
    }
}

/// The rows the session arm of the rule runs over.
fn rows() -> Vec<Row> {
    let account = account_id();
    let user_root = format!("arn:aws:iam::{account}:user/");
    let role_root = format!("arn:aws:iam::{account}:role/");
    let group_prefix = format!("arn:aws:iam::{account}:group/eng/");
    let another_group = format!("arn:aws:iam::{account}:group/engineering");
    let every_iam_action = statement("Allow", "iam:*", "*");
    vec![
        // A session policy allowing every IAM action over a role that allows another one: the
        // grant covering the call sits in the SCOPE, spelling no action and granting nothing
        // of its own, so the call is withheld by the identity's documents although a grant
        // covering it was read.
        Row {
            case: "a session policy naming every IAM action, the role naming another",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Inline(every_iam_action.clone()),
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            reason: no_identity_allow("iam:ListUsers"),
            points_at_its_detail: false,
        },
        // The same with a scope naming every action of EVERY service and a role granting only
        // another service's: another operation, another resource form.
        Row {
            case: "a session policy naming every action, the role naming another service's",
            role_policy: statement("Allow", "s3:ListAllMyBuckets", "*"),
            scope: Scoped::Inline(statement("Allow", "*", "*")),
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root.clone(),
            reason: no_identity_allow("iam:ListRoles"),
            points_at_its_detail: false,
        },
        // The same withholding reached through a MANAGED session policy, so the form the mint
        // named the scope in moves nothing.
        Row {
            case: "a managed session policy naming every IAM action",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Managed(every_iam_action.clone()),
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root.clone(),
            reason: no_identity_allow("iam:ListRoles"),
            points_at_its_detail: false,
        },
        // The grant naming the action sits in the ROLE's own document and is conditioned on a
        // principal tag this session does not carry: read, and not applicable.
        Row {
            case: "a role grant conditioned on a principal tag the session does not carry",
            role_policy: grant_to_tagged("iam:ListRoles", "Team", "platform"),
            scope: Scoped::Nothing,
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root.clone(),
            reason: no_identity_allow("iam:ListRoles"),
            points_at_its_detail: false,
        },
        // The same conditioned grant on a call addressing ONE named entity: the resource form
        // moves nothing either.
        Row {
            case: "a conditioned role grant, on a call naming one role",
            role_policy: grant_to_tagged("iam:ListRoleTags", "Team", "platform"),
            scope: Scoped::Nothing,
            action: "ListRoleTags",
            params: Vec::new(),
            iam_action: "iam:ListRoleTags",
            addresses_its_own_role: true,
            named_as: String::new(),
            reason: no_identity_allow("iam:ListRoleTags"),
            points_at_its_detail: false,
        },
        // A grant naming the action whose DATE clause has not come true yet: the other way a
        // statement that was read does not apply, on a path-scoped collection.
        Row {
            case: "a role grant naming the action that is not yet in effect",
            role_policy: grant_from("iam:ListGroups", FUTURE_INSTANT),
            scope: Scoped::Nothing,
            action: "ListGroups",
            params: vec![("PathPrefix".to_string(), "/eng/".to_string())],
            iam_action: "iam:ListGroups",
            addresses_its_own_role: false,
            named_as: group_prefix,
            reason: no_identity_allow("iam:ListGroups"),
            points_at_its_detail: false,
        },
        // A grant naming the action scoped to ANOTHER resource: read, and not about this
        // request.
        Row {
            case: "a role grant naming the action on another group",
            role_policy: statement("Allow", "iam:GetGroup", &another_group),
            scope: Scoped::Nothing,
            action: "GetGroup",
            params: vec![("GroupName".to_string(), "payroll".to_string())],
            iam_action: "iam:GetGroup",
            addresses_its_own_role: false,
            named_as: "group payroll".to_string(),
            reason: no_identity_allow("iam:GetGroup"),
            points_at_its_detail: false,
        },
        // NOTHING in force names the action — a grant for another service's action only,
        // which is the narrow role a workload carries: the refusal points at its detail.
        Row {
            case: "a role granting another service's action only",
            role_policy: statement("Allow", "cloudwatch:ListMetrics", "*"),
            scope: Scoped::Nothing,
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            reason: no_identity_allow("iam:ListUsers"),
            points_at_its_detail: true,
        },
        // Nothing names the action again, the only grant in force naming another IAM action:
        // the same family, so the same pointer.
        Row {
            case: "a role granting another IAM action only",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Nothing,
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            addresses_its_own_role: false,
            named_as: role_root,
            reason: no_identity_allow("iam:ListRoles"),
            points_at_its_detail: true,
        },
        // A statement about the request saying `Deny` over a grant of everything: a grant
        // DOES name the action, and this refusal is not the withheld one — it is pointed at.
        Row {
            case: "a role statement denying the action beside a grant of everything",
            role_policy: statements(&[("Allow", "iam:*", "*"), ("Deny", "iam:ListUsers", "*")]),
            scope: Scoped::Nothing,
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            reason: IDENTITY_DENY.to_string(),
            points_at_its_detail: true,
        },
        // The call the role allows and the SCOPE does not: the other withholding, which
        // carries the pointer.
        Row {
            case: "a session policy naming another action, the role allowing everything",
            role_policy: every_iam_action.clone(),
            scope: Scoped::Inline(statement("Allow", "iam:ListRoleTags", "*")),
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root.clone(),
            reason: no_session_allow("iam:ListUsers"),
            points_at_its_detail: true,
        },
        // A statement of the SESSION POLICY denying the action it otherwise allows.
        Row {
            case: "a session policy denying the action it otherwise allows",
            role_policy: every_iam_action,
            scope: Scoped::Inline(statements(&[
                ("Allow", "iam:*", "*"),
                ("Deny", "iam:ListUsers", "*"),
            ])),
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            addresses_its_own_role: false,
            named_as: user_root,
            reason: SESSION_DENY.to_string(),
            points_at_its_detail: true,
        },
    ]
}

/// RULE: a session's refusal reached although the decision read a grant that is not about the
/// very action asked for — the identity's own `Allow` covering it, or a scope allowing it
/// without spelling it — is the sentence alone; every other refusal of a session closes with
/// the pointer at the detail of its decision.
#[tokio::test]
async fn which_session_refusals_point_at_the_detail_of_their_decision() {
    let (router, engine) = router_with_engine();

    for (index, row) in rows().into_iter().enumerate() {
        let role_name = format!("pointer{index}");
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
        if row.points_at_its_detail {
            assert_access_denied_message(
                row.case,
                &message,
                &session_arn,
                row.iam_action,
                &named_as,
                &row.reason,
            );
        } else {
            assert_access_denied_sentence(
                row.case,
                &message,
                &session_arn,
                row.iam_action,
                &named_as,
                &row.reason,
            );
        }
    }
}

/// One row of the long-term arm: the documents the user holds, the call it makes and the
/// resource the refusal names.
struct LongTerm {
    case: &'static str,
    policies: Vec<(String, String)>,
    action: &'static str,
    params: Vec<(String, String)>,
    iam_action: &'static str,
    named_as: String,
}

/// RULE: the line is drawn at the CREDENTIAL — a LONG-TERM caller's refusal closes with the
/// pointer even where its own grant names the action and did not apply, which is the shape
/// the session rows above are answered without one.
///
/// The rows are the two ways a grant that WAS read does not apply (a condition that does not
/// hold at the instant of the call, a resource it does not cover), beside the withholding no
/// grant about the action was in force for at all.
#[tokio::test]
async fn a_long_term_callers_refusal_points_at_its_detail_whatever_grant_it_holds() {
    let router = router();
    let account = account_id();
    let rows = vec![
        LongTerm {
            case: "a grant naming the action that is not yet in effect",
            policies: vec![(
                "later".to_string(),
                grant_from("iam:ListUsers", FUTURE_INSTANT),
            )],
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
        },
        LongTerm {
            case: "a grant naming the action on another group",
            policies: vec![(
                "elsewhere".to_string(),
                statement(
                    "Allow",
                    "iam:GetGroup",
                    &format!("arn:aws:iam::{account}:group/engineering"),
                ),
            )],
            action: "GetGroup",
            params: vec![("GroupName".to_string(), "payroll".to_string())],
            iam_action: "iam:GetGroup",
            named_as: "group payroll".to_string(),
        },
        LongTerm {
            case: "a grant naming another action entirely",
            policies: vec![(
                "another".to_string(),
                statement("Allow", "iam:ListRoleTags", "*"),
            )],
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            named_as: format!("arn:aws:iam::{account}:role/"),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let user_name = format!("longterm{index}");
        let access_key_id = principal(&router, row.case, &user_name, &row.policies).await;

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            row.action,
            &borrowed(&row.params),
        )
        .await;
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
        assert_access_denied_message(
            row.case,
            &message,
            &format!("arn:aws:iam::{account}:user/{user_name}"),
            row.iam_action,
            &row.named_as,
            &no_identity_allow(row.iam_action),
        );
    }
}

/// RULE: the wording is a function of the DECISION and not of the call being repeated — a
/// refusal answered as the sentence alone is answered as that same sentence every time, with
/// no authorization id appearing on a later attempt, and the call the documents in force do
/// allow is served throughout.
#[tokio::test]
async fn a_sentence_only_refusal_is_the_same_sentence_every_time() {
    let (router, engine) = router_with_engine();
    let role_name = "repeatedpointer";
    let case = "a session policy naming every IAM action, repeated";
    // The role allows one action and the scope allows every IAM action, so the role's own
    // documents are what withhold every other call — a grant naming it having been read.
    let (access_key_id, session_arn) = session_scoped_caller(
        &router,
        &engine,
        case,
        role_name,
        &statement("Allow", "iam:ListRoles", "*"),
        &Scoped::Inline(statement("Allow", "iam:*", "*")),
    )
    .await;

    let mut answered: Vec<String> = Vec::new();
    for attempt in 0..3 {
        let (status, body) = call_signed_by(&router, Some(&access_key_id), "ListUsers", &[]).await;
        assert_eq!(
            status, 403,
            "case {case:?} attempt {attempt}: expected HTTP 403, got {status}: {body}"
        );
        answered.push(element(&body, "Message"));

        // The call both documents allow stays served, so every refusal above is the
        // documents' doing and not the fixture's.
        let (status, served) = call_signed_by(&router, Some(&access_key_id), "ListRoles", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?} attempt {attempt}: the call both documents allow must be served, \
             got {status}: {served}"
        );
    }

    let expected = assert_access_denied_sentence(
        case,
        &answered[0],
        &session_arn,
        "iam:ListUsers",
        &format!("arn:aws:iam::{}:user/", account_id()),
        &no_identity_allow("iam:ListUsers"),
    );
    for (attempt, message) in answered.iter().enumerate() {
        assert_eq!(
            message, &expected,
            "case {case:?} attempt {attempt}: expected the identical sentence {expected:?}, got \
             {message:?}"
        );
    }
}
