// SPDX-License-Identifier: CC0-1.0

//! `UpdateGroup` — renaming and re-pathing an IAM group — end to end over the awsQuery
//! wire.
//!
//! RULE: an update to a stored entity's name and/or path leaves the SAME entity — the id
//! and create date its create minted are not reminted — under the name and path the
//! request leaves it under (an absent member leaves that half as it stands), with its ARN
//! spelled from those two, and every record this service keys by the entity's name moves
//! with it; a new name ANOTHER entity of that type is already held under is refused with
//! `EntityAlreadyExists` (HTTP 409), a name the account holds no such entity under with
//! `NoSuchEntity` (HTTP 404), and the call is decided against the caller's identity-based
//! policies on BOTH the old and the new name before anything changes.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the two optional members the request carries (the name, the path, both, neither), the
//! name's alphabet and length (the shortest and longest `groupNameType` admits), the path's
//! depth and length (the root, one segment, nested, the longest `pathType` admits), the
//! case the request spells the addressed name in, how many subordinate records the group
//! holds, and — for the authorization suite — the effect and the name a statement is scoped
//! to. No single request is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, and the signed-call
//! fixture the one `tests/all/identity_policy_enforcement.rs` owns, so every suite in this
//! crate builds and reads a request the same way.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// A managed policy document with a single statement, for the inline policies a fixture
/// group embeds (the form `tests/all/group_inline_policies.rs` drives `PutGroupPolicy`
/// with).
const EMBEDDED_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// ARNs of AWS-managed policies, which the account holds no record of and can therefore
/// attach without creating anything first. The rows span the paths the managed-policy ARN
/// grammar admits — entries of the AWS Managed Policy Reference, as
/// `tests/all/group_attached_policies.rs` lists them.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
];

/// One row of the update table: the group as created, and the optional members the update
/// carries.
struct UpdateCase {
    case: &'static str,
    group_name: String,
    path: String,
    new_group_name: Option<String>,
    new_path: Option<String>,
}

/// The rows: every combination of the two optional members (name only, path only, both,
/// neither), over names spanning the `^[\w+=,.@-]+$` alphabet and the shortest and longest
/// lengths `groupNameType` admits (1 and 128 characters), and paths spanning the account
/// root, one segment, a nested path and the longest `pathType` admits (512 characters) —
/// `smithy_query(query="operation", operation="UpdateGroup")`.
fn update_cases() -> Vec<UpdateCase> {
    // The longest path the model admits: 512 characters, both slashes included.
    let longest_path = format!("/{}/", "p".repeat(510));
    vec![
        UpdateCase {
            case: "a rename alone",
            group_name: "admins".to_string(),
            path: "/".to_string(),
            new_group_name: Some("Administrators".to_string()),
            new_path: None,
        },
        UpdateCase {
            case: "a re-path alone",
            group_name: "payroll".to_string(),
            path: "/finance/".to_string(),
            new_group_name: None,
            new_path: Some("/finance/payroll/".to_string()),
        },
        UpdateCase {
            case: "a rename and a re-path at once",
            group_name: "Group_7+x=y,z.w@example-1".to_string(),
            path: "/eng/".to_string(),
            new_group_name: Some("renamed+9=a,b.c@example-2".to_string()),
            new_path: Some("/eng/tools/platform/".to_string()),
        },
        UpdateCase {
            case: "neither member, so nothing changes",
            group_name: "unchanged".to_string(),
            path: "/sales/west/".to_string(),
            new_group_name: None,
            new_path: None,
        },
        UpdateCase {
            case: "a re-spelling of the group's own name",
            group_name: "mixedcase".to_string(),
            path: "/".to_string(),
            new_group_name: Some("MIXEDCASE".to_string()),
            new_path: None,
        },
        UpdateCase {
            case: "the shortest name the model admits",
            group_name: "g".to_string(),
            path: "/eng/".to_string(),
            new_group_name: Some("h".to_string()),
            new_path: Some("/".to_string()),
        },
        UpdateCase {
            case: "to the longest name the model admits",
            group_name: "short".to_string(),
            path: "/".to_string(),
            new_group_name: Some("l".repeat(128)),
            new_path: None,
        },
        UpdateCase {
            case: "from the longest name the model admits",
            group_name: "y".repeat(128),
            path: "/a/".to_string(),
            new_group_name: Some("z".to_string()),
            new_path: None,
        },
        UpdateCase {
            case: "from a nested path to the account root",
            group_name: "nested".to_string(),
            path: "/eng/tools/platform/".to_string(),
            new_group_name: None,
            new_path: Some("/".to_string()),
        },
        UpdateCase {
            case: "to the longest path the model admits",
            group_name: "deep".to_string(),
            path: "/".to_string(),
            new_group_name: Some("deeper".to_string()),
            new_path: Some(longest_path),
        },
    ]
}

/// The ARN an IAM group at `path` carries under `group_name`.
fn group_arn(path: &str, group_name: &str) -> String {
    format!("arn:aws:iam::{}:group{path}{group_name}", account_id())
}

/// Create one group, answering the `(GroupId, CreateDate)` its create MINTED, so a later
/// read is checked against stored values rather than against the fact that it answered.
async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    group_name: &str,
    path: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateGroup must succeed, got {status}: {body}"
    );
    (element(&body, "GroupId"), element(&body, "CreateDate"))
}

/// One `UpdateGroup` call carrying only the members the row names.
async fn update(
    router: &MultiServiceRouter,
    group_name: &str,
    new_group_name: Option<&str>,
    new_path: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("GroupName", group_name)];
    if let Some(new_group_name) = new_group_name {
        params.push(("NewGroupName", new_group_name));
    }
    if let Some(new_path) = new_path {
        params.push(("NewPath", new_path));
    }
    call(router, "UpdateGroup", &params).await
}

/// RULE: the update leaves the SAME group under the name and path it names — the minted id
/// and create date unchanged, the ARN spelled from the name and path it now carries — and
/// the name it no longer carries resolves nowhere.
///
/// The rows are [`update_cases`]. Each addressed group is named in UPPER case, because IAM
/// entity names "are not distinguished by case", so the case a request spells the addressed
/// name in is not part of what it identifies.
#[tokio::test]
async fn an_update_moves_the_same_group_to_the_name_and_path_it_names() {
    for UpdateCase {
        case,
        group_name,
        path,
        new_group_name,
        new_path,
    } in update_cases()
    {
        // A fresh account a row at a time, so the row's group is the only one held and
        // "the account holds exactly this group afterwards" is assertable.
        let router = router();
        let (group_id, create_date) = create_group(&router, case, &group_name, &path).await;

        let addressed = group_name.to_ascii_uppercase();
        let (status, body) = update(
            &router,
            &addressed,
            new_group_name.as_deref(),
            new_path.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateGroup 200 for a held group spelled {addressed:?}, \
             got {status}: {body}"
        );

        // An absent member leaves that half of the group's identity as it stands.
        let expected_name = new_group_name.clone().unwrap_or_else(|| group_name.clone());
        let expected_path = new_path.clone().unwrap_or_else(|| path.clone());

        let (status, read) = call(&router, "GetGroup", &[("GroupName", &expected_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated group to read back under {expected_name:?}, \
             got {status}: {read}"
        );
        for (member, expected) in [
            ("GroupName", expected_name.clone()),
            ("Path", expected_path.clone()),
            ("Arn", group_arn(&expected_path, &expected_name)),
            // The id and the create date identify the group over its lifetime, so an
            // update must not remint either.
            ("GroupId", group_id.clone()),
            ("CreateDate", create_date.clone()),
        ] {
            let actual = element(&read, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?} after the update, got {actual:?}"
            );
        }

        // One group, not two: the update MOVED the group rather than copying it.
        let (status, listed) = call(&router, "ListGroups", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListGroups 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "GroupName"),
            vec![expected_name.clone()],
            "case {case:?}: expected the account to hold exactly the updated group, got {listed}"
        );
        assert_eq!(
            elements(&listed, "GroupId"),
            vec![group_id.clone()],
            "case {case:?}: expected the listed group to keep its minted id, got {listed}"
        );

        // The name the group no longer carries resolves nowhere — unless the update only
        // re-spelled the name it already had, which is the same name: IAM entity names
        // "are not distinguished by case".
        let (status, body) = call(&router, "GetGroup", &[("GroupName", &group_name)]).await;
        if group_name.eq_ignore_ascii_case(&expected_name) {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the re-spelled name to still name the group, got \
                 {status}: {body}"
            );
            assert_eq!(
                element(&body, "GroupName"),
                expected_name,
                "case {case:?}: expected the stored spelling to be the updated one, got {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: expected the released name {group_name:?} to resolve nowhere, \
                 got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
        }
    }
}

/// One row of the subordinate-record table: how many of each subordinate record the group
/// holds, and whether the update renames it (which re-keys everything held under its name)
/// or only re-paths it.
struct FollowCase {
    case: &'static str,
    held: usize,
    new_group_name: Option<&'static str>,
    new_path: Option<&'static str>,
}

/// The rows: one, two and three subordinate records of each kind, over an update that
/// renames the group, one that only re-paths it, and one that does both.
const FOLLOW_CASES: &[FollowCase] = &[
    FollowCase {
        case: "one of each, renamed",
        held: 1,
        new_group_name: Some("platform"),
        new_path: None,
    },
    FollowCase {
        case: "two of each, renamed and re-pathed",
        held: 2,
        new_group_name: Some("Platform.Team"),
        new_path: Some("/eng/platform/"),
    },
    FollowCase {
        case: "three of each, re-pathed only",
        held: 3,
        new_group_name: None,
        new_path: Some("/"),
    },
];

/// RULE: every record this service keys by the entity's name belongs to the ENTITY, so it
/// moves with an update that changes that name — the group's members stay its members, its
/// embedded policies stay embedded in it and its policy attachments stay attached to it —
/// and nothing is left behind under the name it released, so a later entity created with
/// that name inherits none of it.
#[tokio::test]
async fn a_renamed_group_keeps_its_members_policies_and_attachments() {
    for FollowCase {
        case,
        held,
        new_group_name,
        new_path,
    } in FOLLOW_CASES
    {
        let router = router();
        let group_name = "engineers";
        let path = "/eng/";
        create_group(&router, case, group_name, path).await;

        // The members, embedded policies and attachments the group holds before the
        // update. Each kind is read back through its own implemented reader afterwards.
        let members: Vec<String> = (0..*held).map(|index| format!("member{index}")).collect();
        for member in &members {
            let (status, body) = call(&router, "CreateUser", &[("UserName", member)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
            );
            let (status, body) = call(
                &router,
                "AddUserToGroup",
                &[("GroupName", group_name), ("UserName", member)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture AddUserToGroup must succeed, got {status}: {body}"
            );
        }
        let embedded: Vec<String> = (0..*held).map(|index| format!("inline{index}")).collect();
        for policy_name in &embedded {
            let (status, body) = call(
                &router,
                "PutGroupPolicy",
                &[
                    ("GroupName", group_name),
                    ("PolicyName", policy_name),
                    ("PolicyDocument", EMBEDDED_DOCUMENT),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture PutGroupPolicy must succeed, got {status}: {body}"
            );
        }
        let attached: Vec<&str> = AWS_MANAGED_POLICIES.iter().copied().take(*held).collect();
        for policy_arn in &attached {
            let (status, body) = call(
                &router,
                "AttachGroupPolicy",
                &[("GroupName", group_name), ("PolicyArn", policy_arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture AttachGroupPolicy must succeed, got {status}: {body}"
            );
        }

        let (status, body) = update(&router, group_name, *new_group_name, *new_path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateGroup 200, got {status}: {body}"
        );
        let updated = new_group_name.unwrap_or(group_name);

        // The members stayed members of the group, under the name it now carries. The
        // store enumerates them ascending by the case-folded name it keys them on.
        let (status, read) = call(&router, "GetGroup", &[("GroupName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated group to read back, got {status}: {read}"
        );
        assert_eq!(
            elements(&read, "UserName"),
            members,
            "case {case:?}: expected the group to keep its {held} member(s), got {read}"
        );

        // The embedded policies stayed embedded, documents and all.
        let (status, listed) = call(&router, "ListGroupPolicies", &[("GroupName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListGroupPolicies 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "member"),
            embedded,
            "case {case:?}: expected the group to keep its {held} embedded policy(s), got {listed}"
        );
        for policy_name in &embedded {
            let (status, got) = call(
                &router,
                "GetGroupPolicy",
                &[("GroupName", updated), ("PolicyName", policy_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {policy_name:?} to survive the update, got {status}: \
                 {got}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&got, "PolicyDocument"),
                EMBEDDED_DOCUMENT,
                "case {case:?}: expected {policy_name:?} to keep its document, got {got}"
            );
        }

        // The attachments stayed attached — reported both by the group's own list and by
        // the policy's record of the entities it relates, which resolves the group from
        // the key the attachment is held under.
        let (status, listed) = call(
            &router,
            "ListAttachedGroupPolicies",
            &[("GroupName", updated)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAttachedGroupPolicies 200, got {status}: {listed}"
        );
        let mut expected_arns: Vec<String> = attached.iter().map(ToString::to_string).collect();
        expected_arns.sort_unstable();
        let mut reported_arns = elements(&listed, "PolicyArn");
        reported_arns.sort_unstable();
        assert_eq!(
            reported_arns, expected_arns,
            "case {case:?}: expected the group to keep its {held} attachment(s), got {listed}"
        );
        for policy_arn in &attached {
            let (status, related) = call(
                &router,
                "ListEntitiesForPolicy",
                &[("PolicyArn", policy_arn), ("EntityFilter", "Group")],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListEntitiesForPolicy 200, got {status}: {related}"
            );
            assert_eq!(
                elements(&related, "GroupName"),
                vec![updated.to_string()],
                "case {case:?}: expected {policy_arn:?} to relate the group under the name it \
                 now carries, got {related}"
            );
        }

        if updated.eq_ignore_ascii_case(group_name) {
            // The update kept the name, so there is no released name to check.
            continue;
        }

        // Nothing is left behind under the released name: every reader refuses it, and a
        // group later created with it holds none of what its predecessor held — which a
        // delete, refused while a group still holds a subordinate entity, confirms.
        for action in ["GetGroup", "ListGroupPolicies", "ListAttachedGroupPolicies"] {
            let (status, body) = call(&router, action, &[("GroupName", group_name)]).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected {action} 404 for the released name, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code from {action}, got {body}"
            );
        }
        create_group(&router, case, group_name, path).await;
        let (status, read) = call(&router, "GetGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created group must read back, got {status}: {read}"
        );
        assert!(
            elements(&read, "UserName").is_empty(),
            "case {case:?}: expected the re-created group to inherit no member, got {read}"
        );
        let (status, listed) =
            call(&router, "ListGroupPolicies", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListGroupPolicies 200 for the re-created group, got \
             {status}: {listed}"
        );
        assert!(
            elements(&listed, "member").is_empty(),
            "case {case:?}: expected the re-created group to embed no policy, got {listed}"
        );
        let (status, listed) = call(
            &router,
            "ListAttachedGroupPolicies",
            &[("GroupName", group_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAttachedGroupPolicies 200 for the re-created group, \
             got {status}: {listed}"
        );
        assert!(
            elements(&listed, "PolicyArn").is_empty(),
            "case {case:?}: expected the re-created group to carry no attachment, got {listed}"
        );
        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-created group to hold nothing blocking its \
             delete, got {status}: {body}"
        );
    }
}

/// RULE: a new name ANOTHER entity of the same type is already held under is refused with
/// `EntityAlreadyExists` (HTTP 409) rather than merging the two, and the refused update
/// changes nothing; the case the new name is spelled in does not matter, because IAM
/// entity names "are not distinguished by case".
///
/// The rows differ along the case the taken name is spelled in, the path the other group
/// sits at (so the conflict is about the NAME and not the ARN), and whether the refused
/// update also asked for a new path.
#[tokio::test]
async fn a_new_name_another_group_already_carries_is_refused() {
    // `(the spelling the update asks for, the other group's path, the new path asked for)`.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("auditors", "/", None),
        ("AUDITORS", "/", None),
        ("AuDiToRs", "/compliance/", None),
        ("auditors", "/compliance/deep/", Some("/moved/")),
    ];

    for (spelling, other_path, new_path) in rows {
        let case = format!("{spelling} at {other_path}");
        let router = router();
        let (group_id, _) = create_group(&router, &case, "admins", "/eng/").await;
        let (other_id, _) = create_group(&router, &case, "auditors", other_path).await;

        let (status, body) = update(&router, "admins", Some(spelling), *new_path).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected UpdateGroup 409 for a name another group carries, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("Group with name {spelling} already exists."),
            "case {case:?}: expected the refusal to name the entity and the name the request \
             carried, got {:?}",
            element(&body, "Message")
        );

        // The refused update changed nothing: both groups read back as their creates
        // minted them.
        for (group_name, path, group_id) in [
            ("admins", "/eng/", &group_id),
            ("auditors", other_path, &other_id),
        ] {
            let (status, read) = call(&router, "GetGroup", &[("GroupName", group_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {group_name:?} must survive the refused update, got {status}: \
                 {read}"
            );
            for (member, expected) in [
                ("GroupName", group_name.to_string()),
                ("Path", path.to_string()),
                ("GroupId", group_id.to_string()),
            ] {
                let actual = element(&read, member);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the refused update changed {group_name:?}'s {member}: \
                     expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// RULE: a name the account holds no entity of that type under names no entity, so the
/// update is refused with `NoSuchEntity` (HTTP 404) and creates nothing.
///
/// The rows span names the account never held — including near-misses of a held one — and
/// each combination of the optional members, because a name that names no group is refused
/// whatever the update asked to change.
#[tokio::test]
async fn an_update_of_a_name_the_account_holds_no_group_under_is_refused() {
    let router = router();
    create_group(&router, "the fixture", "admins", "/eng/").await;

    for absent in ["nobody", "admin", "admins1", "dmins"] {
        for (new_group_name, new_path) in [
            (Some("renamed"), None),
            (None, Some("/moved/")),
            (Some("renamed"), Some("/moved/")),
            (None, None),
        ] {
            let case = format!("{absent} -> {new_group_name:?} at {new_path:?}");
            let (status, body) = update(&router, absent, new_group_name, new_path).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected UpdateGroup 404 for an unheld name, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!("The group with name {absent} cannot be found."),
                "case {case:?}: expected the refusal to name the entity and the name the \
                 request carried, got {:?}",
                element(&body, "Message")
            );

            // The refused update created nothing, under either name.
            for name in [absent, new_group_name.unwrap_or(absent)] {
                let (status, read) = call(&router, "GetGroup", &[("GroupName", name)]).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the refused update to leave {name:?} naming no \
                     group, got {status}: {read}"
                );
            }
        }
    }
}

/// What a row of the authorization table expects the signed update to answer.
#[derive(Debug, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the call on both names.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One row of the authorization table: the inline policies the calling user holds, whether
/// the update asks for a new name at all, the answer the rule requires, and — for a refusal
/// — the group the refusal must NAME, which is the one of the two names the decision went
/// against.
struct AuthorizationCase {
    case: &'static str,
    policies: Vec<(String, String)>,
    renames: bool,
    expect: Expect,
    refused_on: &'static str,
}

/// The group the fixture account holds, the name an update moves it to, and a third name no
/// statement is scoped to.
const HELD_GROUP: &str = "payroll";
const NEW_GROUP: &str = "finance";

/// The rows: an `Allow` over every resource, one scoped to each of the two names alone, one
/// scoped to both, none at all, one for another action, and a `Deny` scoped to each of the
/// two names on top of a blanket `Allow` — the dimensions the "both names" rule generalises
/// over.
fn authorization_cases() -> Vec<AuthorizationCase> {
    let action = "iam:UpdateGroup";
    let held_arn = group_arn("/", HELD_GROUP);
    let new_arn = group_arn("/", NEW_GROUP);
    vec![
        AuthorizationCase {
            case: "an Allow naming the action on every resource",
            policies: vec![("allow-all".to_string(), statement("Allow", action, "*"))],
            renames: true,
            expect: Expect::Served,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "an Allow scoped to both the old and the new name",
            policies: vec![
                ("allow-old".to_string(), statement("Allow", action, &held_arn)),
                ("allow-new".to_string(), statement("Allow", action, &new_arn)),
            ],
            renames: true,
            expect: Expect::Served,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "an Allow scoped to the old name only",
            policies: vec![("allow-old".to_string(), statement("Allow", action, &held_arn))],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: NEW_GROUP,
        },
        AuthorizationCase {
            case: "an Allow scoped to the new name only",
            policies: vec![("allow-new".to_string(), statement("Allow", action, &new_arn))],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "an Allow scoped to the old name, for an update that renames nothing",
            policies: vec![("allow-old".to_string(), statement("Allow", action, &held_arn))],
            renames: false,
            expect: Expect::Served,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "no identity-based policy at all",
            policies: Vec::new(),
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "an Allow for another action only",
            policies: vec![(
                "allow-create".to_string(),
                statement("Allow", "iam:CreateGroup", "*"),
            )],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "a Deny scoped to the old name on top of a blanket Allow",
            policies: vec![
                ("allow-all".to_string(), statement("Allow", action, "*")),
                ("deny-old".to_string(), statement("Deny", action, &held_arn)),
            ],
            renames: true,
            expect: Expect::ExplicitDeny,
            refused_on: HELD_GROUP,
        },
        AuthorizationCase {
            case: "a Deny scoped to the new name on top of a blanket Allow",
            policies: vec![
                ("allow-all".to_string(), statement("Allow", action, "*")),
                ("deny-new".to_string(), statement("Deny", action, &new_arn)),
            ],
            renames: true,
            expect: Expect::ExplicitDeny,
            refused_on: NEW_GROUP,
        },
    ]
}

/// RULE: the call is decided against the caller's identity-based policies on BOTH the name
/// the entity carries and the name the update would leave it under — the principal must be
/// permitted on each of them — and the decision is reached before anything changes, so a
/// refused update leaves the entity exactly as it was.
#[tokio::test]
async fn an_update_is_decided_against_the_old_and_the_new_name() {
    let iam_action = "iam:UpdateGroup";

    for (
        row,
        AuthorizationCase {
            case,
            policies,
            renames,
            expect,
            refused_on,
        },
    ) in authorization_cases().into_iter().enumerate()
    {
        // A fresh account a row at a time, so a row's policies are the only documents in
        // force when it calls.
        let router = router();
        let (group_id, create_date) = create_group(&router, case, HELD_GROUP, "/").await;

        let user_name = format!("caller{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let mut params = vec![("GroupName", HELD_GROUP)];
        if renames {
            params.push(("NewGroupName", NEW_GROUP));
        }
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), "UpdateGroup", &params).await;

        if expect == Expect::Served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            );
            let expected_name = if renames { NEW_GROUP } else { HELD_GROUP };
            let (status, read) = call(&router, "GetGroup", &[("GroupName", expected_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the served update to leave the group under \
                 {expected_name:?}, got {status}: {read}"
            );
            assert_eq!(
                element(&read, "GroupId"),
                group_id,
                "case {case:?}: expected the served update to keep the minted id, got {read}"
            );
            continue;
        }

        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {:?}",
            error_code(&body)
        );
        let reason = if expect == Expect::ExplicitDeny {
            "with an explicit deny in an identity-based policy".to_string()
        } else {
            format!("because no identity-based policy allows the {iam_action} action")
        };
        assert_access_denied_message(
            case,
            &element(&body, "Message"),
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            iam_action,
            &format!("group {refused_on}"),
            &reason,
        );

        // The refused update changed nothing: the group is still held under the name and
        // path its create gave it, and the name it was asked to move to names no group.
        let (status, read) = call(&router, "GetGroup", &[("GroupName", HELD_GROUP)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must survive the refused update, got {status}: {read}"
        );
        for (member, expected) in [
            ("GroupName", HELD_GROUP.to_string()),
            ("Path", "/".to_string()),
            ("GroupId", group_id.clone()),
            ("CreateDate", create_date.clone()),
        ] {
            let actual = element(&read, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: the refused update changed {member}: expected {expected:?}, \
                 got {actual:?}"
            );
        }
        let (status, read) = call(&router, "GetGroup", &[("GroupName", NEW_GROUP)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused update to leave {NEW_GROUP:?} naming no \
             group, got {status}: {read}"
        );
    }
}
