// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `AccessDenied` message IAM answers a refused call with.
//!
//! The rule under test: the message names what the decision was made about — `User: ` +
//! the ARN of the principal that signed the request + ` is not authorized to perform: ` +
//! the IAM action the call was decided on + ` on resource: ` + the resource it was decided
//! against + ` ` + why the decision went that way — and ENDS there, which is the refusal
//! AWS documents. Only for the actions whose refusal AWS continues with the pointer at
//! the request's authorization details does that sentence go on: the console page, at IAM's
//! own endpoint Region whichever Region the caller addressed, and the authorization id of
//! the refused request, named in both of the pointer's slots, whether or not the caller
//! held any document for the evaluation to read. Which of the two an action is answered
//! with is a fact about that action, not about the call: nothing else selects the wording —
//! not the Region the request addressed, not which documents the caller held, not the
//! resource form, not whether the call reads or writes, not how often it is made, and not
//! WHY the decision went the way it did — so a refusal explained by an explicit `Deny`
//! about the request carries the wording that action's withheld refusal carries.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation is refused and therefore which of the two wordings it is answered with,
//! whether the resource is an ARN, a path-scoped collection, the account-wide `*` or a
//! named entity, which of the two refusals it is (an explicit `Deny` about the request, or
//! no `Allow` for it), and whether the caller held any document at all.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::borrowed;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, authorization_details_id, call, call_signed_by,
    call_signed_in_region, element, error_code, router,
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
    /// Whether the refusal points at the authorization details of an evaluation: this
    /// action's refusal is one AWS words with that pointer.
    details: bool,
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
        // iam:ListUsers is worded with the sentence alone.
        Case {
            case: "implicit deny on a collection read",
            policies: Vec::new(),
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
            details: false,
        },
        // The same read with a page size: a parameter that does not touch the decision
        // must not touch the wording either.
        Case {
            case: "implicit deny on a collection read asking for one item",
            policies: Vec::new(),
            action: "ListUsers",
            params: vec![("MaxItems".to_string(), "1".to_string())],
            iam_action: "iam:ListUsers",
            named_as: format!("arn:aws:iam::{account}:user/"),
            reason: no_allow("iam:ListUsers"),
            details: false,
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
            details: false,
        },
        // Another collection and another action, to show the resource form is not what
        // selects the wording.
        Case {
            case: "implicit deny listing roles",
            policies: Vec::new(),
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            named_as: format!("arn:aws:iam::{account}:role/"),
            reason: no_allow("iam:ListRoles"),
            details: false,
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
            details: false,
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
            details: false,
        },
        // A WRITE refused on one entity, worded like the reads above: what the call does
        // is not what selects the wording either.
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
            details: false,
        },
        // The few actions whose refusal AWS continues with the pointer, on the resource
        // forms their requests take: a read of one user, a read of that user's keys, a
        // write of its tags, a wildcard collection and the account singleton.
        Case {
            case: "implicit deny reading one user by name",
            policies: Vec::new(),
            action: "GetUser",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:GetUser",
            named_as: target.clone(),
            reason: no_allow("iam:GetUser"),
            details: true,
        },
        Case {
            case: "implicit deny listing one user's access keys",
            policies: Vec::new(),
            action: "ListAccessKeys",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListAccessKeys",
            named_as: target.clone(),
            reason: no_allow("iam:ListAccessKeys"),
            details: true,
        },
        Case {
            case: "implicit deny tagging one user",
            policies: Vec::new(),
            action: "TagUser",
            params: [vec![("UserName".to_string(), TARGET.to_string())], tagged].concat(),
            iam_action: "iam:TagUser",
            named_as: target.clone(),
            reason: no_allow("iam:TagUser"),
            details: true,
        },
        Case {
            case: "implicit deny listing the account's OIDC providers",
            policies: Vec::new(),
            action: "ListOpenIDConnectProviders",
            params: Vec::new(),
            iam_action: "iam:ListOpenIDConnectProviders",
            named_as: format!("arn:aws:iam::{account}:oidc-provider/*"),
            reason: no_allow("iam:ListOpenIDConnectProviders"),
            details: true,
        },
        Case {
            case: "implicit deny listing one user's tags",
            policies: Vec::new(),
            action: "ListUserTags",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListUserTags",
            named_as: target.clone(),
            reason: no_allow("iam:ListUserTags"),
            details: true,
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
            details: true,
        },
        Case {
            case: "implicit deny listing one user's embedded policies",
            policies: Vec::new(),
            action: "ListUserPolicies",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListUserPolicies",
            named_as: target.clone(),
            reason: no_allow("iam:ListUserPolicies"),
            details: true,
        },
        // The same two reads of a ROLE: a request that addresses one entity of another
        // type, so neither the entity type nor the resource form selects the wording —
        // each of these actions is one AWS words with the pointer, as the user's tag
        // listing above is.
        Case {
            case: "implicit deny listing one role's tags",
            policies: Vec::new(),
            action: "ListRoleTags",
            params: vec![("RoleName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListRoleTags",
            named_as: format!("role {TARGET}"),
            reason: no_allow("iam:ListRoleTags"),
            details: true,
        },
        Case {
            case: "implicit deny listing one role's embedded policies",
            policies: Vec::new(),
            action: "ListRolePolicies",
            params: vec![("RoleName".to_string(), TARGET.to_string())],
            iam_action: "iam:ListRolePolicies",
            named_as: format!("role {TARGET}"),
            reason: no_allow("iam:ListRolePolicies"),
            details: true,
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
            details: true,
        },
        // The two halves of the account's credential report: both are decided on the
        // account-wide resource, like the account property above, and both are refused with
        // the pointer — the read as well as the write, so nothing about the direction of the
        // call selects the wording.
        Case {
            case: "implicit deny generating the account's credential report",
            policies: Vec::new(),
            action: "GenerateCredentialReport",
            params: Vec::new(),
            iam_action: "iam:GenerateCredentialReport",
            named_as: "*".to_string(),
            reason: no_allow("iam:GenerateCredentialReport"),
            details: true,
        },
        Case {
            case: "implicit deny reading the account's credential report",
            policies: Vec::new(),
            action: "GetCredentialReport",
            params: Vec::new(),
            iam_action: "iam:GetCredentialReport",
            named_as: "*".to_string(),
            reason: no_allow("iam:GetCredentialReport"),
            details: true,
        },
        // The account's password policy is decided on the same account-wide resource, and
        // its refusal ENDS at the reason: nothing has been observed answering the pointer
        // for this action, so it takes the documented sentence.
        Case {
            case: "implicit deny reading the account's password policy",
            policies: Vec::new(),
            action: "GetAccountPasswordPolicy",
            params: Vec::new(),
            iam_action: "iam:GetAccountPasswordPolicy",
            named_as: "*".to_string(),
            reason: no_allow("iam:GetAccountPasswordPolicy"),
            details: false,
        },
        // A caller holding a document that grants something ELSE: the same withholding,
        // reached with a document in force rather than none. Which wording the refusal
        // takes is the action's, so these two rows differ from their no-document twins
        // above in nothing at all.
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
            details: false,
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
            details: true,
        },
        // A grant for the refused action itself whose CONDITION does not hold at the
        // instant of the call: the statement was read and does not apply, which is again
        // the withholding — so these two rows are worded exactly as their no-document
        // twins above are, each as its own action is.
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
            details: true,
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
            details: false,
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
            details: false,
        },
        // An explicit `Deny` over a grant for everything: the other reason clause, on a
        // collection read. The deny itself explains the refusal, so nothing follows it.
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
            details: false,
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
            details: false,
        },
        // An explicit `Deny` of an action whose refusal carries the pointer: WHICH wording
        // a refusal takes is a fact about the action, so the pointer closes this one too —
        // the evaluation it names ran whatever the decision was.
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
            details: true,
        },
        // An explicit `Deny` of another action whose refusal carries the pointer, on a
        // request that addresses one entity: the same wording as that action's withheld
        // refusal.
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
            details: true,
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
            details: false,
        },
        // Two more actions on the default wording, reached on resource forms the rows
        // above do not use: a write naming the policy ARN the request addressed, and a
        // read naming the identity ARN it addressed. What the message ends at is neither
        // the resource form's doing nor the operation kind's.
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
            details: false,
        },
        // The same wording on a READ whose resource is the ARN the request addressed.
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
            details: false,
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
/// was made on, closed by why it went that way, and ENDS there — continuing with the
/// pointer at the request's authorization details only for the actions AWS answers that
/// pointer for, whichever way their decision went.
#[tokio::test]
async fn a_refusal_names_the_decision_and_is_worded_as_its_action_is() {
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
        // The sentence the decision is named in, and — for the refusals AWS points at an
        // evaluation from — the pointer at its authorization details.
        assert_access_denied_message(
            case.case,
            &message,
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            case.iam_action,
            &case.named_as,
            &case.reason,
        );
        let sentence = format!(
            "User: arn:aws:iam::{}:user/{user_name} is not authorized to perform: {} on \
             resource: {} {}",
            account_id(),
            case.iam_action,
            case.named_as,
            case.reason,
        );
        let clause = message
            .strip_prefix(&sentence)
            .expect("the sentence the shared assertion just matched");
        if case.details {
            assert!(
                authorization_details_id(clause).is_some(),
                "case {:?}: expected the refusal to point at the authorization details of \
                 the request, got {clause:?} after the reason",
                case.case
            );
        } else {
            assert!(
                clause.is_empty(),
                "case {:?}: expected the message to end at the reason {:?}, got {clause:?} \
                 after it",
                case.case,
                case.reason
            );
        }
    }
}

/// RULE: every part of the message is a fact of the request and of the documents in
/// force, so repeating the same refused call — and making it in another Region — answers
/// the identical message: the sentence alone for an action AWS words with it, and the
/// sentence closed by that request's own authorization details for an action it points at
/// an evaluation from.
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

    // The two actions worded with the sentence alone: every attempt reads exactly it.
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
    ] {
        let expected = format!(
            "User: {caller} is not authorized to perform: {iam_action} on resource: \
             {named_as} {}",
            no_allow(iam_action),
        );
        for (attempt, message) in messages.iter().enumerate() {
            assert_eq!(
                message, &expected,
                "case {action} attempt {attempt}: expected every refusal of the same call \
                 to read {expected:?}, got {message:?}"
            );
        }
    }

    // The action AWS points at an evaluation from: the sentence is the same every time,
    // and what follows it is the pointer at the authorization details of THAT refusal,
    // whose id names the one request — so the id is the same across the attempts too.
    let expected = format!(
        "User: {caller} is not authorized to perform: iam:ListAccessKeys on resource: user \
         {TARGET} {}",
        no_allow("iam:ListAccessKeys"),
    );
    let mut ids: Vec<String> = Vec::new();
    for (attempt, message) in keyings.iter().enumerate() {
        let clause = message.strip_prefix(&expected).unwrap_or_else(|| {
            panic!(
                "case ListAccessKeys attempt {attempt}: expected every refusal of the same \
                 call to be worded alike, got {message:?}"
            )
        });
        let pointed_at = authorization_details_id(clause).unwrap_or_else(|| {
            panic!(
                "case ListAccessKeys attempt {attempt}: expected the refusal to point at \
                 the authorization details of the request, got {clause:?} after the reason"
            )
        });
        ids.push(pointed_at.to_string());
    }
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "case ListAccessKeys: expected one authorization id for the same refused call, got \
         {ids:?}"
    );
}

/// RULE: the page the pointer names is IAM's own, at its one endpoint Region, whichever
/// Region the refused call addressed — IAM is a global service, so there is one place its
/// authorization details are read from.
#[tokio::test]
async fn the_pointer_names_one_page_whichever_region_the_call_addressed() {
    let router = router();
    target_user(&router).await;
    let user_name = "elsewhere";
    // A grant for another action: a document is in force, so the refusal of this call is
    // a withholding and carries the pointer.
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
        let sentence = format!(
            "User: arn:aws:iam::{account}:user/{user_name} is not authorized to perform: \
             iam:GetUser on resource: user {TARGET} {}",
            no_allow("iam:GetUser"),
            account = account_id(),
        );
        let clause = message.strip_prefix(&sentence).unwrap_or_else(|| {
            panic!("case {region:?}: expected the refusal sentence, got {message:?}")
        });
        assert!(
            authorization_details_id(clause).is_some(),
            "case {region:?}: expected the pointer at IAM's one authorization-details page, \
             got {clause:?}"
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
/// document at all — so it is worded as that action is: the sentence closed by the
/// pointer at the request's authorization details for an action AWS answers that pointer
/// for, and the sentence alone for every other — while the sibling action the same
/// document already grants is served.
#[tokio::test]
async fn a_grant_not_yet_in_effect_is_withheld_and_worded_as_its_action_is() {
    let router = router();

    // (case, the action granted from a past instant and therefore served, the action
    //  granted only from a future instant and therefore refused, whether that action's
    //  withheld refusal carries the pointer)
    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "a tag listing not yet in effect",
            "GetUser",
            "ListUserTags",
            true,
        ),
        (
            "a user listing not yet in effect",
            "ListUserTags",
            "ListUsers",
            false,
        ),
    ];

    for (row, (case, in_effect, not_yet, details)) in cases.iter().enumerate() {
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
        let pointed_at = assert_access_denied_message(
            case,
            &message,
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            &iam_action,
            &names_resource(not_yet, &user_name),
            &no_allow(&iam_action),
        );
        if *details {
            assert!(
                !pointed_at.is_empty(),
                "case {case:?}: expected the refusal of {not_yet} to point at the \
                 authorization details of the request, got {message:?}"
            );
        } else {
            assert!(
                pointed_at.is_empty(),
                "case {case:?}: expected the refusal of {not_yet} to end at the reason, got \
                 the authorization id {pointed_at:?} in {message:?}"
            );
        }
    }
}
