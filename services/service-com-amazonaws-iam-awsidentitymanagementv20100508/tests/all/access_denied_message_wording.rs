// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `AccessDenied` message IAM answers a refused call with.
//!
//! The rule under test: the message names what the decision was made about — `User: ` +
//! the ARN of the principal that signed the request + ` is not authorized to perform: ` +
//! the IAM action the call was decided on + ` on resource: ` + the resource it was decided
//! against + ` ` + why the decision went that way — and then POINTS at the detail of that
//! decision: the console address of its authorization details and the API to read them
//! from, naming an authorization id minted for that one request. That sentence plus that
//! pointer is the WHOLE message of every refusal this service answers from the
//! authorization gate: nothing else is appended to it — not by which action was asked for,
//! not by the Region the request addressed, not by which documents the caller held, not by
//! the resource form, not by whether the call reads or writes, and not by how often it is
//! made. The ONE refusal whose recordings close at the sentence instead is a SESSION
//! credential's refusal for want of an identity-based grant where the decision read a grant
//! that is not about that very action — the identity's own `Allow` covering it, or a scope
//! allowing it without spelling it; `tests/all/session_refusal_authorization_detail_pointer.rs`
//! and `tests/all/scope_specificity_refusal_detail_pointer.rs` are the tables for that line,
//! and the row below that reaches it asserts the sentence alone.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation is refused, whether the resource is an ARN, a path-scoped collection, the
//! account-wide `*` or a named entity, which of the refusals it is (an explicit `Deny`
//! about the request, or no `Allow` for it, in whichever of the documents in force), and
//! whether the caller held any document at all — including the refusals of a caller whose
//! credential a MINT SCOPED, where the document that withheld the call is a session policy
//! rather than one of the identity's own.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    self, RoleSessionRecord, SessionPolicies,
};

use crate::identity_policy_enforcement::{principal, statement};
use crate::permissions_boundary_ceiling::statements;
use crate::role_lifecycle::borrowed;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, assert_access_denied_sentence,
    authorization_details_id, call, call_signed_by, call_signed_in_region, element, error_code,
    router, router_with_engine,
};

/// The reason clause for a call no statement in force allows — the implicit deny.
fn no_allow(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The reason clause for a call a statement about it explicitly denies.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// An `Allow` for every IAM action on every resource: what a row about an explicit `Deny`
/// is layered on top of, so the row demonstrates the `Deny` and not a missing grant.
const ALLOW_EVERYTHING: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:*","Resource":"*"}]}"#;

/// An instant no call this suite makes is made after, so a `DateGreaterThan` clause over
/// it does not hold at the time of the call.
const FUTURE_INSTANT: &str = "2099-01-01T00:00:00Z";

/// An instant every call this suite makes is made after, so a `DateGreaterThan` clause
/// over it holds at the time of the call.
const PAST_INSTANT: &str = "2020-01-01T00:00:00Z";

/// A document whose statements each `Allow` one action on every resource from the instant
/// beside it: a statement whose instant has not arrived IS in force and does NOT apply,
/// because a date clause is decided against the instant of the call
/// (`tests/all/request_condition_context.rs` is the table for the operators themselves).
/// A refusal reached through one of those is the same withholding a caller holding no
/// document at all reaches.
fn grants_after(clauses: &[(&str, &str)]) -> String {
    let statements: Vec<String> = clauses
        .iter()
        .map(|(action, instant)| {
            format!(
                r#"{{"Effect":"Allow","Action":"{action}","Resource":"*","Condition":{{"DateGreaterThan":{{"aws:CurrentTime":"{instant}"}}}}}}"#
            )
        })
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        statements.join(",")
    )
}

/// [`grants_after`] with one clause: a grant for `action` that becomes true at `instant`.
fn grant_after(action: &str, instant: &str) -> String {
    grants_after(&[(action, instant)])
}

/// The user the rows that address ONE entity are refused on, created once as a fixture:
/// the refusals in this suite are about the caller's own permissions, so the entity they
/// are refused on is never the caller.
const TARGET: &str = "subject";

/// One row: the documents the calling user holds, the awsQuery call it makes, and the
/// parts the refusal must name.
struct Case {
    case: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs put on the caller in this order.
    policies: Vec<(String, String)>,
    /// The awsQuery `Action` and its form parameters.
    action: &'static str,
    params: Vec<(String, String)>,
    /// The IAM action name the decision is made on.
    iam_action: &'static str,
    /// The resource as the refusal NAMES it: an ARN for a request that addresses a
    /// collection, the entity's type and name for one that addresses a single entity.
    named_as: String,
    /// The clause closing the sentence.
    reason: String,
}

/// The rows. Each runs against its own fresh IAM user, so the documents a row puts are
/// the only ones in force when it calls.
fn cases(policy_arn: &str) -> Vec<Case> {
    let account = account_id();
    let target = format!("user {TARGET}");
    let tagged = vec![
        ("Tags.member.1.Key".to_string(), "env".to_string()),
        ("Tags.member.1.Value".to_string(), "prod".to_string()),
    ];
    vec![
        // A collection read with no parameters: the account's users at the root path.
        Case {
            case: "implicit deny on a collection read",
            policies: Vec::new(),
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
        },
        // The same read with a page size: a parameter that does not touch the decision
        // must not touch the message either.
        Case {
            case: "implicit deny on a collection read asking for one item",
            policies: Vec::new(),
            action: "ListUsers",
            params: vec![("MaxItems".to_string(), "1".to_string())],
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
        },
        // A path-scoped collection: the resource the refusal names is the prefix the
        // request addressed, not the root.
        Case {
            case: "implicit deny on a path-scoped collection read",
            policies: Vec::new(),
            action: "ListGroups",
            params: vec![("PathPrefix".to_string(), "/eng/tools/".to_string())],
            iam_action: "iam:ListGroups",
            named_as: format!("arn:aws:iam::{account}:group/eng/tools/"),
            reason: no_allow("iam:ListGroups"),
        },
        // Another collection and another action, to show the resource form moves nothing
        // but the resource the refusal names — this is the action an assumed-role session is
        // refused on in
        // `core/router/tests/all/eks_pod_identity_credentials_are_scoped_to_the_role.rs`.
        Case {
            case: "implicit deny listing roles",
            policies: Vec::new(),
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            named_as: format!("arn:aws:iam::{account}:role/"),
            reason: no_allow("iam:ListRoles"),
        },
        // A request that addresses the account's whole configuration: the resource is
        // the account-wide one, so the message carries no ARN at all.
        Case {
            case: "implicit deny on an account-wide read",
            policies: Vec::new(),
            action: "GetAccountAuthorizationDetails",
            params: Vec::new(),
            iam_action: "iam:GetAccountAuthorizationDetails",
            named_as: "*".to_string(),
            reason: no_allow("iam:GetAccountAuthorizationDetails"),
        },
        // A request that addresses ONE entity by name: the refusal names the entity.
        Case {
            case: "implicit deny reading one group by name",
            policies: Vec::new(),
            action: "GetGroup",
            params: vec![("GroupName".to_string(), "payroll".to_string())],
            iam_action: "iam:GetGroup",
            named_as: "group payroll".to_string(),
            reason: no_allow("iam:GetGroup"),
        },
        // A WRITE refused on one entity, worded like the reads above: what the call does
        // moves nothing in the refusal either.
        Case {
            case: "implicit deny removing a tag from one policy",
            policies: Vec::new(),
            action: "UntagPolicy",
            params: vec![
                ("PolicyArn".to_string(), policy_arn.to_string()),
                ("TagKeys.member.1".to_string(), "env".to_string()),
            ],
            iam_action: "iam:UntagPolicy",
            named_as: format!("policy {policy_arn}"),
            reason: no_allow("iam:UntagPolicy"),
        },
        // More actions on the resource forms their requests take: a read of one user, a
        // read of that user's keys, a write of its tags, a wildcard collection and the
        // account singleton.
        Case {
            case: "implicit deny reading one user by name",
            policies: Vec::new(),
            action: "GetUser",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:GetUser",
            named_as: target.clone(),
            reason: no_allow("iam:GetUser"),
        },
        Case {
            case: "implicit deny listing one user's access keys",
            policies: Vec::new(),
            action: "ListAccessKeys",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListAccessKeys",
            named_as: target.clone(),
            reason: no_allow("iam:ListAccessKeys"),
        },
        Case {
            case: "implicit deny tagging one user",
            policies: Vec::new(),
            action: "TagUser",
            params: [vec![("UserName".to_string(), TARGET.to_string())], tagged].concat(),
            iam_action: "iam:TagUser",
            named_as: target.clone(),
            reason: no_allow("iam:TagUser"),
        },
        Case {
            case: "implicit deny listing the account's OIDC providers",
            policies: Vec::new(),
            action: "ListOpenIDConnectProviders",
            params: Vec::new(),
            iam_action: "iam:ListOpenIDConnectProviders",
            named_as: format!("arn:aws:iam::{account}:oidc-provider/*"),
            reason: no_allow("iam:ListOpenIDConnectProviders"),
        },
        Case {
            case: "implicit deny listing one user's tags",
            policies: Vec::new(),
            action: "ListUserTags",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListUserTags",
            named_as: target.clone(),
            reason: no_allow("iam:ListUserTags"),
        },
        Case {
            case: "implicit deny reading one user's embedded policy",
            policies: Vec::new(),
            action: "GetUserPolicy",
            params: vec![
                ("UserName".to_string(), TARGET.to_string()),
                ("PolicyName".to_string(), "wording".to_string()),
            ],
            iam_action: "iam:GetUserPolicy",
            named_as: target.clone(),
            reason: no_allow("iam:GetUserPolicy"),
        },
        Case {
            case: "implicit deny listing one user's embedded policies",
            policies: Vec::new(),
            action: "ListUserPolicies",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListUserPolicies",
            named_as: target.clone(),
            reason: no_allow("iam:ListUserPolicies"),
        },
        // The same two reads of a ROLE: a request that addresses one entity of another
        // type, so neither the entity type nor the resource form moves the refusal.
        Case {
            case: "implicit deny listing one role's tags",
            policies: Vec::new(),
            action: "ListRoleTags",
            params: vec![("RoleName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListRoleTags",
            named_as: format!("role {TARGET}"),
            reason: no_allow("iam:ListRoleTags"),
        },
        Case {
            case: "implicit deny listing one role's embedded policies",
            policies: Vec::new(),
            action: "ListRolePolicies",
            params: vec![("RoleName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListRolePolicies",
            named_as: format!("role {TARGET}"),
            reason: no_allow("iam:ListRolePolicies"),
        },
        Case {
            case: "implicit deny writing an account property",
            policies: Vec::new(),
            action: "PutAccountProperties",
            params: vec![
                (
                    "Properties.entry.1.key".to_string(),
                    "RoleManager/Enabled".to_string(),
                ),
                ("Properties.entry.1.value".to_string(), "true".to_string()),
            ],
            iam_action: "iam:PutAccountProperties",
            named_as: "*".to_string(),
            reason: no_allow("iam:PutAccountProperties"),
        },
        // The two halves of the account's credential report: both are decided on the
        // account-wide resource, like the account property above — the read as well as the
        // write, so nothing about the direction of the call moves the refusal.
        Case {
            case: "implicit deny generating the account's credential report",
            policies: Vec::new(),
            action: "GenerateCredentialReport",
            params: Vec::new(),
            iam_action: "iam:GenerateCredentialReport",
            named_as: "*".to_string(),
            reason: no_allow("iam:GenerateCredentialReport"),
        },
        Case {
            case: "implicit deny reading the account's credential report",
            policies: Vec::new(),
            action: "GetCredentialReport",
            params: Vec::new(),
            iam_action: "iam:GetCredentialReport",
            named_as: "*".to_string(),
            reason: no_allow("iam:GetCredentialReport"),
        },
        // The account's password policy is decided on the same account-wide resource, and
        // its refusal is worded exactly as the two above are.
        Case {
            case: "implicit deny reading the account's password policy",
            policies: Vec::new(),
            action: "GetAccountPasswordPolicy",
            params: Vec::new(),
            iam_action: "iam:GetAccountPasswordPolicy",
            named_as: "*".to_string(),
            reason: no_allow("iam:GetAccountPasswordPolicy"),
        },
        // A caller holding a document that grants something ELSE: the same withholding,
        // reached with a document in force rather than none, so these two rows differ from
        // their no-document twins above in nothing at all.
        Case {
            case: "withheld collection read, the caller holding a grant for another action",
            policies: vec![(
                "allow-listroles".to_string(),
                statement("Allow", "iam:ListRoles", "*"),
            )],
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
        },
        Case {
            case: "withheld key listing, the caller holding a grant for another action",
            policies: vec![(
                "allow-listroles".to_string(),
                statement("Allow", "iam:ListRoles", "*"),
            )],
            action: "ListAccessKeys",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListAccessKeys",
            named_as: target.clone(),
            reason: no_allow("iam:ListAccessKeys"),
        },
        // A grant for the refused action itself whose CONDITION does not hold at the
        // instant of the call: the statement was read and does not apply, which is again
        // the withholding — so these two rows are worded exactly as their no-document
        // twins above are.
        Case {
            case: "withheld tag listing, the caller's grant not yet in effect",
            policies: vec![(
                "allow-listusertags-later".to_string(),
                grant_after("iam:ListUserTags", FUTURE_INSTANT),
            )],
            action: "ListUserTags",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListUserTags",
            named_as: target.clone(),
            reason: no_allow("iam:ListUserTags"),
        },
        Case {
            case: "withheld collection read, the caller's grant not yet in effect",
            policies: vec![(
                "allow-listusers-later".to_string(),
                grant_after("iam:ListUsers", FUTURE_INSTANT),
            )],
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
        },
        // A grant for the same action on ANOTHER resource: the statement was read and
        // does not cover the request, which is the same withholding.
        Case {
            case: "withheld read, the caller's grant scoped to another resource",
            policies: vec![(
                "allow-other-group".to_string(),
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
            reason: no_allow("iam:GetGroup"),
        },
        // An explicit `Deny` over a grant for everything: the other reason clause, on a
        // collection read. An evaluation decided it too, so it is pointed at like the rest.
        Case {
            case: "explicit deny on a collection read",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-listusers".to_string(),
                    statement("Deny", "iam:ListUsers", "*"),
                ),
            ],
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: EXPLICIT_DENY.to_string(),
        },
        // An explicit `Deny` on a request that addresses one entity by name.
        Case {
            case: "explicit deny reading one group by name",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-getgroup".to_string(),
                    statement("Deny", "iam:GetGroup", "*"),
                ),
            ],
            action: "GetGroup",
            params: vec![("GroupName".to_string(), "payroll".to_string())],
            iam_action: "iam:GetGroup",
            named_as: "group payroll".to_string(),
            reason: EXPLICIT_DENY.to_string(),
        },
        // An explicit `Deny` of a read of one user: the pointer closes this one as it
        // closes the withheld refusals above, since the evaluation it names ran whatever
        // the decision was.
        Case {
            case: "explicit deny reading one user by name",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-getuser".to_string(),
                    statement("Deny", "iam:GetUser", "*"),
                ),
            ],
            action: "GetUser",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:GetUser",
            named_as: target.clone(),
            reason: EXPLICIT_DENY.to_string(),
        },
        // An explicit `Deny` of another action on a request that addresses one entity: the
        // same wording as that action's withheld refusal.
        Case {
            case: "explicit deny listing one user's tags",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-listusertags".to_string(),
                    statement("Deny", "iam:ListUserTags", "*"),
                ),
            ],
            action: "ListUserTags",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListUserTags",
            named_as: target.clone(),
            reason: EXPLICIT_DENY.to_string(),
        },
        // An explicit `Deny` on the account-wide read.
        Case {
            case: "explicit deny on an account-wide read",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-details".to_string(),
                    statement("Deny", "iam:GetAccountAuthorizationDetails", "*"),
                ),
            ],
            action: "GetAccountAuthorizationDetails",
            params: Vec::new(),
            iam_action: "iam:GetAccountAuthorizationDetails",
            named_as: "*".to_string(),
            reason: EXPLICIT_DENY.to_string(),
        },
        // Two more actions reached on resource forms the rows above do not use: a write
        // naming the policy ARN the request addressed, and a read naming the identity ARN
        // it addressed. What the message carries is neither the resource form's doing nor
        // the operation kind's.
        Case {
            case: "withheld tag removal naming the policy ARN",
            policies: Vec::new(),
            action: "UntagPolicy",
            params: vec![
                (
                    "PolicyArn".to_string(),
                    format!("arn:aws:iam::{account}:policy/payroll"),
                ),
                ("TagKeys.member.1".to_string(), "team".to_string()),
            ],
            iam_action: "iam:UntagPolicy",
            named_as: format!("policy arn:aws:iam::{account}:policy/payroll"),
            reason: no_allow("iam:UntagPolicy"),
        },
        // The same on a READ whose resource is the ARN the request addressed.
        Case {
            case: "withheld policy-grant report naming the identity ARN",
            policies: Vec::new(),
            action: "ListPoliciesGrantingServiceAccess",
            params: vec![
                (
                    "Arn".to_string(),
                    format!("arn:aws:iam::{account}:user/payroll"),
                ),
                ("ServiceNamespaces.member.1".to_string(), "s3".to_string()),
            ],
            iam_action: "iam:ListPoliciesGrantingServiceAccess",
            named_as: format!("arn:aws:iam::{account}:user/payroll"),
            reason: no_allow("iam:ListPoliciesGrantingServiceAccess"),
        },
    ]
}

/// The user the rows that address ONE entity are refused on: a fixture created with the
/// account's own credentials, which no row's refusal is about.
async fn target_user(router: &MultiServiceRouter) {
    let (status, created) = call(router, "CreateUser", &[("UserName", TARGET)]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateUser must succeed, got {status}: {created}"
    );
}

/// A managed policy for the rows that address one policy, answering its ARN: the same kind
/// of fixture, and likewise not what any refusal below is about.
async fn managed_policy(router: &MultiServiceRouter) -> String {
    let (status, policy) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", "wording"),
            ("PolicyDocument", ALLOW_EVERYTHING),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreatePolicy must succeed, got {status}: {policy}"
    );
    element(&policy, "Arn")
}

/// RULE: a refusal's message names the principal, the action and the resource the decision
/// was made on, closed by why it went that way and then by the pointer at that decision's
/// authorization detail — whichever action was asked for and whichever way the decision
/// went.
#[tokio::test]
async fn a_refusal_names_the_decision_the_reason_and_the_authorization_detail() {
    let router = router();
    target_user(&router).await;
    let policy_arn = managed_policy(&router).await;

    for (row, case) in cases(&policy_arn).into_iter().enumerate() {
        let user_name = format!("denied{row}");
        let access_key_id = principal(&router, case.case, &user_name, &case.policies).await;
        let params = borrowed(&case.params);

        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), case.action, &params).await;

        assert_eq!(
            status, 403,
            "case {:?}: expected HTTP 403, got {status}: {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {:?}: expected the error code AccessDenied, got {:?}",
            case.case,
            error_code(&body)
        );

        let message = element(&body, "Message");
        // The sentence the decision is named in, which is the whole message.
        assert_access_denied_message(
            case.case,
            &message,
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            case.iam_action,
            &case.named_as,
            &case.reason,
        );
    }
}

/// RULE: every part of the message besides the per-request authorization id is a fact of
/// the request and of the documents in force, so repeating the same refused call — and
/// making it in another Region — answers the identical sentence and the identical pointer
/// clause, with a FRESH authorization id each time: the id names the record of one
/// decision, so two refusals of the same call are pointed at two records.
#[tokio::test]
async fn the_same_refused_call_is_worded_identically_every_time() {
    let router = router();
    target_user(&router).await;
    let policy_arn = managed_policy(&router).await;
    let user_name = "repeated";
    // No identity-based policy at all, so every call below is the implicit deny.
    let access_key_id = principal(&router, "repeated refusals", user_name, &[]).await;
    let caller = format!("arn:aws:iam::{}:user/{user_name}", account_id());

    // Rows spanning repeated calls and Regions in three of AWS's geographies, including
    // the one IAM's global endpoint lives in: a message carrying anything drawn from the
    // request's Region would differ across them.
    let mut listings: Vec<String> = Vec::new();
    let mut keyings: Vec<String> = Vec::new();
    let mut removals: Vec<String> = Vec::new();
    for region in ["us-east-1", "us-east-1", "eu-west-1", "ap-southeast-2"] {
        for (collected, action, params) in [
            (&mut listings, "ListUsers", Vec::new()),
            (&mut keyings, "ListAccessKeys", vec![("UserName", TARGET)]),
            (
                &mut removals,
                "UntagPolicy",
                vec![
                    ("PolicyArn", policy_arn.as_str()),
                    ("TagKeys.member.1", "env"),
                ],
            ),
        ] {
            let (status, body) =
                call_signed_in_region(&router, Some(&access_key_id), region, action, &params)
                    .await;
            assert_eq!(
                status, 403,
                "case {region:?} {action}: expected the implicit deny to be refused, got \
                 {status}: {body}"
            );
            collected.push(element(&body, "Message"));
        }
    }

    // Every one of the three: the sentence and the pointer clause are the whole message,
    // the same ones every time and in every Region, and the authorization id inside the
    // pointer is a new one on every attempt.
    for (action, iam_action, named_as, messages) in [
        (
            "ListUsers",
            "iam:ListUsers",
            format!("arn:aws:iam::{}:user/", account_id()),
            &listings,
        ),
        (
            "UntagPolicy",
            "iam:UntagPolicy",
            format!("policy {policy_arn}"),
            &removals,
        ),
        (
            "ListAccessKeys",
            "iam:ListAccessKeys",
            format!("user {TARGET}"),
            &keyings,
        ),
    ] {
        let mut minted: Vec<String> = Vec::new();
        for (attempt, message) in messages.iter().enumerate() {
            let case = format!("{action} attempt {attempt}");
            assert_access_denied_message(
                &case,
                message,
                &caller,
                iam_action,
                &named_as,
                &no_allow(iam_action),
            );
            minted.push(authorization_details_id(&case, message));
        }
        let distinct: std::collections::BTreeSet<&String> = minted.iter().collect();
        assert_eq!(
            distinct.len(),
            minted.len(),
            "case {action}: expected every refusal to point at its own authorization \
             detail, got {minted:?}"
        );
    }
}

/// RULE: the message is the sentence naming the decision whichever Region the refused call
/// addressed — IAM is a global service, so nothing of the Region the caller reached it
/// through reaches the refusal.
#[tokio::test]
async fn the_message_is_the_same_sentence_whichever_region_the_call_addressed() {
    let router = router();
    target_user(&router).await;
    let user_name = "elsewhere";
    // A grant for another action: a document is in force, so the refusal of this call is
    // a withholding.
    let access_key_id = principal(
        &router,
        "a withheld call from several Regions",
        user_name,
        &[(
            "allow-listroles".to_string(),
            statement("Allow", "iam:ListRoles", "*"),
        )],
    )
    .await;

    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let (status, body) = call_signed_in_region(
            &router,
            Some(&access_key_id),
            region,
            "GetUser",
            &[("UserName", TARGET)],
        )
        .await;
        assert_eq!(
            status, 403,
            "case {region:?}: expected the withheld call to be refused, got {status}: {body}"
        );
        let message = element(&body, "Message");
        assert_access_denied_message(
            region,
            &message,
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            "iam:GetUser",
            &format!("user {TARGET}"),
            &no_allow("iam:GetUser"),
        );
    }
}

/// The form parameters a call to `action` carries, for the actions the rows below drive:
/// a request that addresses one entity names it, and a collection read names nothing.
fn params_for(action: &str, user_name: &str) -> Vec<(String, String)> {
    match action {
        "ListUsers" => Vec::new(),
        _ => vec![("UserName".to_string(), user_name.to_string())],
    }
}

/// The resource a refusal of `action` NAMES: the entity a request addressing one names,
/// and the collection ARN a list read is decided on.
fn names_resource(action: &str, user_name: &str) -> String {
    match action {
        "ListUsers" => format!("arn:aws:iam::{}:user/", account_id()),
        _ => format!("user {user_name}"),
    }
}

/// RULE: a refusal reached because the only statement for the action is conditioned on an
/// instant the call is not yet past is the same WITHHOLDING as one reached with no
/// document at all — so it is worded as every other refusal is — while the sibling action
/// the same document already grants is served.
#[tokio::test]
async fn a_grant_not_yet_in_effect_is_withheld_and_worded_like_every_other() {
    let router = router();

    // (case, the action granted from a past instant and therefore served, the action
    //  granted only from a future instant and therefore refused)
    let cases: &[(&str, &str, &str)] = &[
        (
            "a tag listing not yet in effect",
            "GetUser",
            "ListUserTags",
        ),
        (
            "a user listing not yet in effect",
            "ListUserTags",
            "ListUsers",
        ),
    ];

    for (row, (case, in_effect, not_yet)) in cases.iter().enumerate() {
        let user_name = format!("conditioned{row}");
        // ONE document holding both statements, so each is decided on its own clause.
        let access_key_id = principal(
            &router,
            case,
            &user_name,
            &[(
                "clocked".to_string(),
                grants_after(&[
                    (&format!("iam:{in_effect}"), PAST_INSTANT),
                    (&format!("iam:{not_yet}"), FUTURE_INSTANT),
                ]),
            )],
        )
        .await;

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            in_effect,
            &borrowed(&params_for(in_effect, &user_name)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the leg already in effect ({in_effect}) to be served, \
             got {status}: {body}"
        );

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            not_yet,
            &borrowed(&params_for(not_yet, &user_name)),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the leg not yet in effect ({not_yet}) to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {:?}",
            error_code(&body)
        );

        let iam_action = format!("iam:{not_yet}");
        let message = element(&body, "Message");
        assert_access_denied_message(
            case,
            &message,
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            &iam_action,
            &names_resource(not_yet, &user_name),
            &no_allow(&iam_action),
        );
    }
}

/// The trust policy every fixture role below is created with: one statement, the shape
/// `CreateRole`'s own modelled example carries. WHO may assume the role decides nothing
/// about a call the session then makes, so no row varies it.
const SESSION_TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// How long a fixture session's credential is live for, from the instant its record is
/// written. Generous on purpose: nothing here waits for a credential to lapse, the rows
/// only need it to still speak for its session while they call
/// (`tests/all/role_protection_and_refusal_messages.rs` is the table for the expiry
/// itself).
const SESSION_LIFETIME_SECONDS: i64 = 3_600;

/// The `RoleSessionName` every fixture session is minted under, which the session's ARN
/// carries and a refusal names the caller by.
const SESSION_NAME: &str = "wording";

/// Which document withheld or denied a session-scoped call — the fact the reason clause of
/// the refusal states, and the one thing that differs between these refusals and the ones
/// the tables above drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Withheld {
    /// The role's own documents allow the call and the session policies in force state no
    /// `Allow` for it: the intersection of the two withholds it.
    NoSessionAllow,
    /// A statement of a session policy denies the request, which no grant overturns.
    SessionDeny,
    /// The session policies allow it and the role's own documents do not. A session policy
    /// caps and never grants, so what the identity's documents do not allow stays withheld
    /// by THEM, and the clause names them.
    NoIdentityAllow,
}

impl Withheld {
    /// The clause closing the message for this refusal of `iam_action`.
    fn reason(self, iam_action: &str) -> String {
        match self {
            Self::NoSessionAllow => {
                format!("because no session policy allows the {iam_action} action")
            }
            Self::SessionDeny => "with an explicit deny in a session policy".to_string(),
            Self::NoIdentityAllow => no_allow(iam_action),
        }
    }

    /// Whether the refusal CLOSES with the pointer at the detail of its decision.
    ///
    /// A session's refusal for want of an identity-based grant, where the decision read a
    /// grant that is not about that very action — which is how the row below reaches this
    /// clause, its scope allowing every IAM action without spelling one — is the refusal
    /// family of this gate whose recordings carry no pointer
    /// (`tests/all/session_refusal_authorization_detail_pointer.rs` and
    /// `tests/all/scope_specificity_refusal_detail_pointer.rs` are the tables for that rule).
    /// The session-policy clauses are the other family and carry it.
    fn points_at_its_detail(self) -> bool {
        self != Self::NoIdentityAllow
    }
}

/// The FORM the mint scoped a row's credential with, both of which `AssumeRole` takes: the
/// document itself, or a managed policy of the account named by ARN whose document is read
/// at the instant of the request — or nothing at all, for a mint that passed no session
/// policy and left the session with the role's own permissions.
///
/// `pub(crate)` with [`session_scoped_caller`], which the sibling suite for the pointer line
/// (`tests/all/session_refusal_authorization_detail_pointer.rs`) drives its own rows through.
pub(crate) enum Scoped {
    /// The document an inline `Policy` parameter carries.
    Inline(String),
    /// The document of the managed policy a `PolicyArns` entry names, created in the
    /// account by the fixture so the scope is held by ARN.
    Managed(String),
    /// No session policy: what every mint that passes none records.
    Nothing,
}

/// One row: what the ROLE's own documents say, how the mint scoped the session's
/// credential, the call that session makes, and which document the answer rests on.
struct Scoping {
    case: &'static str,
    /// The inline document put on the role the session is of — the permissions a session
    /// of it has, which its session policies are the intersection with.
    role_policy: String,
    /// The session policy the mint passed, in one of its two forms.
    scope: Scoped,
    /// The awsQuery `Action` the session calls, and the IAM action it is decided on.
    action: &'static str,
    iam_action: &'static str,
    /// `None` for a call the intersection permits, which is served — the row that shows
    /// the rest are refused by the documents and not by the fixture.
    expect: Option<Withheld>,
}

/// The rows. They differ along what the rule generalises over: which operation the session
/// calls, whether its resource is one role or a collection, the form the session policy
/// took (inline or managed), whether that policy is silent on the action, scoped to another
/// resource or denies it outright, and which of the documents in force decided the call.
fn scopings() -> Vec<Scoping> {
    let elsewhere = format!("arn:aws:iam::{}:role/elsewhere", account_id());
    vec![
        // The pair of operations the recorded session-policy refusals were observed on: a
        // role's tag listing and its attachment listing, each withheld by a session policy
        // that names only the other.
        Scoping {
            case: "a session policy naming another action, refusing a tag listing",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statement("Allow", "iam:ListAttachedRolePolicies", "*")),
            action: "ListRoleTags",
            iam_action: "iam:ListRoleTags",
            expect: Some(Withheld::NoSessionAllow),
        },
        Scoping {
            case: "a session policy naming another action, refusing an attachment listing",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statement("Allow", "iam:ListRoleTags", "*")),
            action: "ListAttachedRolePolicies",
            iam_action: "iam:ListAttachedRolePolicies",
            expect: Some(Withheld::NoSessionAllow),
        },
        // A session policy that names the action but is scoped to a resource the request
        // does not address: the scope states no `Allow` ABOUT this request either.
        Scoping {
            case: "a session policy naming the action on another role",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statement("Allow", "iam:ListRoleTags", &elsewhere)),
            action: "ListRoleTags",
            iam_action: "iam:ListRoleTags",
            expect: Some(Withheld::NoSessionAllow),
        },
        // A collection read, so the resource form the refusal names is not left untested:
        // the ARN of the collection the request addressed rather than one entity.
        Scoping {
            case: "a session policy naming another action, refusing a user listing",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statement("Allow", "iam:ListRoleTags", "*")),
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            expect: Some(Withheld::NoSessionAllow),
        },
        Scoping {
            case: "a session policy naming another action, refusing a role listing",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statement("Allow", "iam:ListRoleTags", "*")),
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            expect: Some(Withheld::NoSessionAllow),
        },
        // An explicit `Deny` in the session policy, over a scope that allows everything
        // else: a different fact about the same decision, worded differently.
        Scoping {
            case: "a session policy denying the action it otherwise allows",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statements(&[
                ("Allow", "iam:*", "*"),
                ("Deny", "iam:ListRoleTags", "*"),
            ])),
            action: "ListRoleTags",
            iam_action: "iam:ListRoleTags",
            expect: Some(Withheld::SessionDeny),
        },
        // The same withholding reached through a MANAGED session policy, so the form the
        // mint named the scope in moves no character of the message.
        Scoping {
            case: "a managed session policy naming another action",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Managed(statement("Allow", "iam:ListRolePolicies", "*")),
            action: "ListAttachedRolePolicies",
            iam_action: "iam:ListAttachedRolePolicies",
            expect: Some(Withheld::NoSessionAllow),
        },
        // A session policy grants nothing of its own, so a call only IT allows is refused
        // as if it had allowed nothing — and the clause names the identity's documents.
        Scoping {
            case: "a session policy allowing what the role's own documents do not",
            role_policy: statement("Allow", "iam:ListRoleTags", "*"),
            scope: Scoped::Inline(statement("Allow", "iam:*", "*")),
            action: "ListAttachedRolePolicies",
            iam_action: "iam:ListAttachedRolePolicies",
            expect: Some(Withheld::NoIdentityAllow),
        },
        // The call both documents allow is served, so no row above is refused by the
        // fixture rather than by the scope under test.
        Scoping {
            case: "a session policy naming the action the role allows",
            role_policy: ALLOW_EVERYTHING.to_string(),
            scope: Scoped::Inline(statement("Allow", "iam:ListRoleTags", "*")),
            action: "ListRoleTags",
            iam_action: "iam:ListRoleTags",
            expect: None,
        },
    ]
}

/// The form parameters a session-scoped call carries: a request addressing the row's own
/// role names it, and a collection read names nothing.
fn session_params(action: &str, role_name: &str) -> Vec<(String, String)> {
    match action {
        "ListRoles" | "ListUsers" => Vec::new(),
        _ => vec![("RoleName".to_string(), role_name.to_string())],
    }
}

/// The resource a refusal of `action` NAMES: the role as the request named it for a call
/// addressing one, and the ARN of the collection a list read is decided on.
fn session_named_as(action: &str, role_name: &str) -> String {
    match action {
        "ListRoles" => format!("arn:aws:iam::{}:role/", account_id()),
        "ListUsers" => format!("arn:aws:iam::{}:user/", account_id()),
        _ => format!("role {role_name}"),
    }
}

/// Give the account a role carrying `role_policy`, record a session of it whose credential
/// the mint scoped the way `scope` says, and answer the access key id that credential is
/// carried under beside the ARN `sts:GetCallerIdentity` reports for it.
///
/// Everything a row needs except the session itself is set up over the wire. The credential
/// of a role session is minted by `sts:AssumeRole`, which is not this service's API, so the
/// record of what that mint scoped is written into this service's own store exactly as the
/// minting operation writes it — the store the gate reads the signing principal out of.
pub(crate) async fn session_scoped_caller(
    router: &MultiServiceRouter,
    held: &engine::InMemoryEngine,
    case: &str,
    role_name: &str,
    role_policy: &str,
    scope: &Scoped,
) -> (String, String) {
    let (status, created) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", SESSION_TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateRole must succeed, got {status}: {created}"
    );
    let role_arn = element(&created, "Arn");

    let (status, put) = call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", "role-grant"),
            ("PolicyDocument", role_policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutRolePolicy must succeed, got {status}: {put}"
    );

    let scoped = match scope {
        Scoped::Nothing => SessionPolicies::default(),
        Scoped::Inline(document) => SessionPolicies {
            inline: Some(document.clone()),
            managed_arns: Vec::new(),
            none_means_no_permissions: false,
            ..SessionPolicies::default()
        },
        Scoped::Managed(document) => {
            let policy_name = format!("{role_name}-session-scope");
            let (status, policy) = call(
                router,
                "CreatePolicy",
                &[
                    ("PolicyName", &policy_name),
                    ("PolicyDocument", document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {policy}"
            );
            SessionPolicies {
                inline: None,
                managed_arns: vec![element(&policy, "Arn")],
                none_means_no_permissions: false,
                ..SessionPolicies::default()
            }
        }
    };

    // The access key id the credential is carried under: an id of the form a minted session
    // carries, one per role, so no two rows sign as the same session.
    let access_key_id = format!("ASIA{}", role_name.to_ascii_uppercase());
    let session_arn = format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{SESSION_NAME}",
        account_id()
    );
    let minted = state::now();
    let store = state::service_state(held).expect("the IAM store");
    store.lock().record_role_session(
        &access_key_id,
        RoleSessionRecord {
            session_arn: session_arn.clone(),
            role_arn,
            expires: ::aws_smithy_types::DateTime::from_secs(
                minted.secs() + SESSION_LIFETIME_SECONDS,
            ),
            scope: scoped,
            tags: Vec::new(),
        },
        minted,
    );
    (access_key_id, session_arn)
}

/// RULE: a refusal of a call signed with a credential a mint SCOPED is worded exactly as
/// every other refusal this gate answers — the principal that signed it (the session, not
/// the role it is of), the action, the resource the decision was made on and the clause
/// naming the document that withheld or denied the call, and nothing after it — whichever
/// of the documents in force decided it and whichever form the session policy took.
#[tokio::test]
async fn a_session_scoped_refusal_is_worded_like_every_other() {
    let (router, engine) = router_with_engine();

    for (row, scoping) in scopings().into_iter().enumerate() {
        let role_name = format!("scoped{row}");
        let (access_key_id, session_arn) = session_scoped_caller(
            &router,
            &engine,
            scoping.case,
            &role_name,
            &scoping.role_policy,
            &scoping.scope,
        )
        .await;

        let params = session_params(scoping.action, &role_name);
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), scoping.action, &borrowed(&params)).await;

        let Some(withheld) = scoping.expect else {
            assert_eq!(
                status, 200,
                "case {:?}: expected the call both documents allow to be served, got \
                 {status}: {body}",
                scoping.case
            );
            continue;
        };
        assert_eq!(
            status, 403,
            "case {:?}: expected HTTP 403, got {status}: {body}",
            scoping.case
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {:?}: expected the error code AccessDenied, got {:?}",
            scoping.case,
            error_code(&body)
        );

        let message = element(&body, "Message");
        // The sentence the decision is named in, which is the whole message — closed by the
        // pointer at the detail of the decision for the refusals that carry one.
        let named_as = session_named_as(scoping.action, &role_name);
        let reason = withheld.reason(scoping.iam_action);
        if withheld.points_at_its_detail() {
            assert_access_denied_message(
                scoping.case,
                &message,
                &session_arn,
                scoping.iam_action,
                &named_as,
                &reason,
            );
        } else {
            assert_access_denied_sentence(
                scoping.case,
                &message,
                &session_arn,
                scoping.iam_action,
                &named_as,
                &reason,
            );
        }
    }
}

/// RULE: every part of a session-scoped refusal besides the per-request authorization id is
/// a fact of the request and of the documents in force — so the same refused call, repeated
/// and made from elsewhere, answers the identical sentence and pointer clause, and a fresh
/// authorization id each time.
#[tokio::test]
async fn a_session_scoped_refusal_is_worded_identically_from_every_region() {
    let (router, engine) = router_with_engine();
    let role_name = "reregional";
    let case = "a withheld session-scoped call from several Regions";
    let (access_key_id, session_arn) = session_scoped_caller(
        &router,
        &engine,
        case,
        role_name,
        ALLOW_EVERYTHING,
        // A scope naming another action, so the call below is withheld by the session
        // policy while the role's own documents allow it.
        &Scoped::Inline(statement("Allow", "iam:ListAttachedRolePolicies", "*")),
    )
    .await;

    let mut answered: Vec<String> = Vec::new();
    // Repeated calls, and Regions in three of AWS's geographies including the one IAM's
    // global endpoint lives in: a message carrying anything drawn from the request's
    // Region, or from the attempt, would differ across these.
    for region in ["us-east-1", "us-east-1", "eu-west-1", "ap-southeast-2"] {
        let (status, body) = call_signed_in_region(
            &router,
            Some(&access_key_id),
            region,
            "ListRoleTags",
            &[("RoleName", role_name)],
        )
        .await;
        assert_eq!(
            status, 403,
            "case {region:?}: expected the withheld call to be refused, got {status}: {body}"
        );
        let message = element(&body, "Message");
        assert_access_denied_message(
            region,
            &message,
            &session_arn,
            "iam:ListRoleTags",
            &format!("role {role_name}"),
            &Withheld::NoSessionAllow.reason("iam:ListRoleTags"),
        );
        answered.push(authorization_details_id(region, &message));
    }
    let distinct: std::collections::BTreeSet<&String> = answered.iter().collect();
    assert_eq!(
        distinct.len(),
        answered.len(),
        "expected each refusal of the same call to point at its own authorization detail, \
         got {answered:?}"
    );
}
