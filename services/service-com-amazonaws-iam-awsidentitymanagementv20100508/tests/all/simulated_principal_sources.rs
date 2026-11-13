// SPDX-License-Identifier: CC0-1.0

//! `SimulatePrincipalPolicy` over the principals a policy source may name and the
//! places the documents in force on one come from, end to end over the awsQuery wire.
//!
//! The rule under test: a simulation reports, per action, what the identity-based
//! documents in force on the entity `PolicySourceArn` names decide — whichever kind
//! of entity that is (the operation documents "the entity can be an IAM user, group,
//! or role") and wherever each document comes from (an inline policy embedded in the
//! entity, or the DEFAULT version of a managed policy attached to it). It is a
//! REPORT: every document it reads was persisted by an earlier call, so what it read
//! is unchanged afterwards and a repeat decides alike.
//!
//! `tests/all/policy_simulation.rs` is the sibling suite that spans the decision rule
//! itself (the effects, the action and resource forms, conditions, a simulated
//! permissions boundary, paging and the refusals) with a user carrying inline
//! policies; the rows here differ along the two axes that suite holds fixed — the
//! ENTITY KIND the ARN names and the SOURCE each document comes from — so neither
//! axis is pinned by one shape of principal.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, and the simulation request spelling is the one
//! `tests/all/policy_simulation.rs` owns.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_simulation::{reported, simulate, ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY};
use crate::user_lifecycle::{call, element, elements, router};

/// A trust policy with a single statement — the form `CreateRole`'s own modelled
/// example carries. Every role here needs one to exist at all; it says who may assume
/// the role rather than what the role may do, so it is never one of the documents a
/// simulation decides from.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lakeformation.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The name every inline policy in this suite is embedded under.
const INLINE_POLICY_NAME: &str = "permissions";

/// The documents put in force on the simulated principal: one `Allow` over a
/// service's whole action namespace and one `Deny` naming a single action within it,
/// so one request reaches all three decisions
/// `PolicyEvaluationDecisionType` declares.
const PERMISSIONS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"lakeformation:*","Resource":"*"},{"Effect":"Deny","Action":"lakeformation:PutDataLakeSettings","Resource":"*"}]}"#;

/// A grant over one service's whole action namespace, the first version of the
/// managed policy the version row attaches.
const ALLOW_SERVICE: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"lakeformation:*","Resource":"*"}]}"#;

/// The same grant narrowed by a `Deny` over one action within it, the version the
/// version row makes the default.
const ALLOW_SERVICE_LESS_ONE_ACTION: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"lakeformation:*","Resource":"*"},{"Effect":"Deny","Action":"lakeformation:PutDataLakeSettings","Resource":"*"}]}"#;

/// The decision each action must be reported with, whatever entity carries
/// [`PERMISSIONS`] and wherever that document comes from: the wildcard `Allow` covers
/// the first two, the `Deny` names the third, and no statement is about the fourth.
const DECIDED: &[(&str, &str)] = &[
    ("lakeformation:GetDataAccess", ALLOWED),
    ("lakeformation:GetDataLakeSettings", ALLOWED),
    ("lakeformation:PutDataLakeSettings", EXPLICIT_DENY),
    ("s3:GetObject", IMPLICIT_DENY),
];

/// The kind of IAM identity a `PolicySourceArn` may name.
#[derive(Clone, Copy, Debug)]
enum Kind {
    User,
    Group,
    Role,
}

impl Kind {
    /// The word IAM builds this kind's operation and member names from:
    /// `CreateUser`/`UserName`, `PutGroupPolicy`/`GroupName`, `AttachRolePolicy`/
    /// `RoleName`.
    fn word(self) -> &'static str {
        match self {
            Kind::User => "User",
            Kind::Group => "Group",
            Kind::Role => "Role",
        }
    }
}

/// Where one document in force on the principal comes from.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// An inline policy embedded in the entity (`Put<kind>Policy`).
    Inline,
    /// The default version of a managed policy attached to the entity
    /// (`CreatePolicy` + `Attach<kind>Policy`).
    Managed,
}

/// One principal: its kind, where its document comes from, and the name the account
/// holds it under.
struct Case {
    case: &'static str,
    kind: Kind,
    source: Source,
    entity_name: &'static str,
}

/// Rows spanning the three entity kinds a policy source may name and both places a
/// document in force on one comes from. The group's row is the INLINE one, which
/// covers the kind; a group's managed attachments are asserted end to end by
/// `tests/all/group_attached_policies.rs` and
/// `tests/all/group_attached_policy_listing.rs`, which drive the same collection
/// through the simulation and the context-key reader. The names span the
/// `^[\w+=,.@-]+$` alphabet and both name cases, so the rule is not pinned to one
/// spelling.
const CASES: &[Case] = &[
    Case {
        case: "a user's inline policy",
        kind: Kind::User,
        source: Source::Inline,
        entity_name: "lake_reader",
    },
    Case {
        case: "a managed policy attached to a user",
        kind: Kind::User,
        source: Source::Managed,
        entity_name: "Lake.Admin",
    },
    Case {
        case: "a role's inline policy",
        kind: Kind::Role,
        source: Source::Inline,
        entity_name: "data-admin",
    },
    Case {
        case: "a managed policy attached to a role",
        kind: Kind::Role,
        source: Source::Managed,
        entity_name: "DataAdmin=2",
    },
    Case {
        case: "a group's inline policy",
        kind: Kind::Group,
        source: Source::Inline,
        entity_name: "data+admins",
    },
];

/// Create the entity `case` names, answering the ARN the account holds it under —
/// which is what a simulation addresses it by.
async fn create_entity(router: &MultiServiceRouter, case: &Case) -> String {
    let word = case.kind.word();
    let action = format!("Create{word}");
    let name_member = format!("{word}Name");
    let mut params: Vec<(&str, &str)> = vec![(name_member.as_str(), case.entity_name)];
    if matches!(case.kind, Kind::Role) {
        params.push(("AssumeRolePolicyDocument", TRUST_POLICY));
    }
    let (status, body) = call(router, &action, &params).await;
    assert_eq!(
        status, 200,
        "case {:?}: {action} must succeed, got {status}: {body}",
        case.case
    );
    element(&body, "Arn")
}

/// Put `document` in force on the entity `case` names, through the operation that
/// persists it for that kind and source.
///
/// Answers the handle a later read proves the simulation left alone: the inline
/// policy's name, or the attached managed policy's ARN.
async fn put_in_force(router: &MultiServiceRouter, case: &Case, document: &str) -> String {
    let word = case.kind.word();
    let name_member = format!("{word}Name");
    match case.source {
        Source::Inline => {
            let action = format!("Put{word}Policy");
            let (status, body) = call(
                router,
                &action,
                &[
                    (name_member.as_str(), case.entity_name),
                    ("PolicyName", INLINE_POLICY_NAME),
                    ("PolicyDocument", document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: {action} must succeed, got {status}: {body}",
                case.case
            );
            INLINE_POLICY_NAME.to_string()
        }
        Source::Managed => {
            let policy_name = format!("{}-permissions", case.entity_name);
            let (status, body) = call(
                router,
                "CreatePolicy",
                &[
                    ("PolicyName", policy_name.as_str()),
                    ("PolicyDocument", document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: CreatePolicy must succeed, got {status}: {body}",
                case.case
            );
            let policy_arn = element(&body, "Arn");
            let action = format!("Attach{word}Policy");
            let (status, body) = call(
                router,
                &action,
                &[
                    (name_member.as_str(), case.entity_name),
                    ("PolicyArn", policy_arn.as_str()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: {action} must succeed, got {status}: {body}",
                case.case
            );
            policy_arn
        }
    }
}

/// Assert the document the simulation read is still exactly what was persisted: the
/// embedded document reads back unchanged, and the attachment is still the one the
/// entity carries.
async fn still_in_force(
    router: &MultiServiceRouter,
    case: &Case,
    handle: &str,
    document: &str,
) {
    let word = case.kind.word();
    let name_member = format!("{word}Name");
    match case.source {
        Source::Inline => {
            let action = format!("Get{word}Policy");
            let (status, body) = call(
                router,
                &action,
                &[
                    (name_member.as_str(), case.entity_name),
                    ("PolicyName", handle),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: {action} must succeed, got {status}: {body}",
                case.case
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&body, "PolicyDocument"),
                document,
                "case {:?}: a simulation must leave the embedded document as it was, got {body}",
                case.case
            );
        }
        Source::Managed => {
            let action = format!("ListAttached{word}Policies");
            let (status, body) =
                call(router, &action, &[(name_member.as_str(), case.entity_name)]).await;
            assert_eq!(
                status, 200,
                "case {:?}: {action} must succeed, got {status}: {body}",
                case.case
            );
            assert_eq!(
                elements(&body, "PolicyArn"),
                vec![handle.to_string()],
                "case {:?}: a simulation must leave the attachment as it was, got {body}",
                case.case
            );
        }
    }
}

/// RULE: the simulation decides from the documents in force on the entity the policy
/// source names, whichever kind of entity it is and wherever each document comes
/// from — and it changes neither what is in force nor what a repeat decides.
#[tokio::test]
async fn every_principal_kind_and_document_source_decides_the_simulation() {
    let router = router();
    let actions: Vec<&str> = DECIDED.iter().map(|(action, _)| *action).collect();
    let expected: Vec<(String, String)> = DECIDED
        .iter()
        .map(|(action, decision)| ((*action).to_string(), (*decision).to_string()))
        .collect();

    for case in CASES {
        let arn = create_entity(&router, case).await;
        let handle = put_in_force(&router, case, PERMISSIONS).await;

        let (status, body) = simulate(&router, &arn, &actions, &[], &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        assert_eq!(
            reported(&body),
            expected,
            "case {:?}: expected {expected:?}, got {body}",
            case.case
        );

        // A report: what it read is untouched, so the same request decides alike.
        still_in_force(&router, case, &handle, PERMISSIONS).await;
        let (status, again) = simulate(&router, &arn, &actions, &[], &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 on the repeat, got {status}: {again}",
            case.case
        );
        assert_eq!(
            reported(&again),
            expected,
            "case {:?}: a simulation must decide alike while nothing changed, got {again}",
            case.case
        );
    }
}

/// RULE: the version of an attached managed policy that decides is its DEFAULT
/// version — "the default version is the operative version … the one that is in
/// effect for the IAM users, groups, and roles to which the policy is attached"
/// (`SetDefaultPolicyVersion`) — so making another version the default changes what
/// the simulation decides, while the attachment itself stays as it was.
#[tokio::test]
async fn the_attached_policys_default_version_is_what_decides() {
    let router = router();
    let role_name = "versioned-admin";
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateRole must succeed, got {status}: {body}");
    let role_arn = element(&body, "Arn");

    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "VersionedPermissions"),
            ("PolicyDocument", ALLOW_SERVICE),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreatePolicy must succeed, got {status}: {body}");
    let policy_arn = element(&body, "Arn");

    let (status, body) = call(
        &router,
        "AttachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", policy_arn.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "AttachRolePolicy must succeed, got {status}: {body}"
    );

    let actions = ["lakeformation:PutDataLakeSettings", "lakeformation:GetDataAccess"];
    // Rows: the version the create itself made the default, then a later version
    // made the default, which narrows the grant by one action.
    let rows: [(&str, Option<&str>, [&str; 2]); 2] = [
        ("the version the create made default", None, [ALLOWED, ALLOWED]),
        (
            "a later version made default",
            Some(ALLOW_SERVICE_LESS_ONE_ACTION),
            [EXPLICIT_DENY, ALLOWED],
        ),
    ];
    for (case, new_default, decisions) in rows {
        if let Some(document) = new_default {
            let (status, body) = call(
                &router,
                "CreatePolicyVersion",
                &[
                    ("PolicyArn", policy_arn.as_str()),
                    ("PolicyDocument", document),
                    ("SetAsDefault", "true"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreatePolicyVersion must succeed, got {status}: {body}"
            );
        }
        let (status, body) = simulate(&router, &role_arn, &actions, &[], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let expected: Vec<(String, String)> = actions
            .iter()
            .zip(decisions)
            .map(|(action, decision)| ((*action).to_string(), decision.to_string()))
            .collect();
        assert_eq!(
            reported(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );

        let (status, listed) = call(
            &router,
            "ListAttachedRolePolicies",
            &[("RoleName", role_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAttachedRolePolicies must succeed, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "PolicyArn"),
            vec![policy_arn.clone()],
            "case {case:?}: the attachment must be the one it was, got {listed}"
        );
    }
}
