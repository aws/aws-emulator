// SPDX-License-Identifier: CC0-1.0

//! `UpdateUser` — renaming and re-pathing an IAM user — end to end over the awsQuery wire.
//!
//! RULE: an update to a stored user's name and/or path leaves the SAME user — "the user's
//! unique identifier remains the same", so neither the id nor the create date its create
//! minted is reminted — under the name and path the request leaves it under (a member the
//! request omits leaves that half as it stands), with its ARN spelled from those two, and
//! every record this service keys by the user's name moves with it; a new name ANOTHER
//! user is already held under is refused with `EntityAlreadyExists` (HTTP 409), a name the
//! account holds no user under with `NoSuchEntity` (HTTP 404), and the call is decided
//! against the caller's identity-based policies on BOTH the old and the new name before
//! anything changes ("to change a user name, the requester must have appropriate
//! permissions on both the source object and the target object").
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the two optional members the request carries (the name, the path, both, neither), the
//! name's alphabet and the length bounds `userNameType` admits, the path's depth and the
//! length bounds `pathType` admits, the case the request spells the addressed name in, how
//! many subordinate records the user holds, and — for the authorization suite — the effect
//! and the name a statement is scoped to. No single request is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the signed-call
//! fixture the one `tests/all/identity_policy_enforcement.rs` owns, so every suite in this
//! crate builds and reads a request the same way.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// The IAM user this emulator's own long-term credential belongs to
/// (`service_common::caller_principal`), which the account holds before any request is
/// served. Every read that enumerates users reports it beside the fixture's own, so the
/// suites that count users name it here.
const ACCOUNT_CALLER: &str = ::service_common::caller_principal::AMBIENT_USER_NAME;

/// An identity policy document with a single statement, for the inline policies a fixture
/// user embeds (the form `tests/all/user_policy_listing.rs` drives `PutUserPolicy` with).
const EMBEDDED_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The password a fixture user's console profile is created with.
const CONSOLE_PASSWORD: &str = "a-fixture-password-1!";

/// ARNs of AWS-managed policies, which the account holds no record of and can therefore
/// attach without creating anything first — entries of the AWS Managed Policy Reference, as
/// `tests/all/user_attached_policies.rs` lists them.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
];

/// Services that issue service-specific credentials, from the catalog
/// `CreateServiceSpecificCredential`'s documentation publishes (the rows
/// `tests/all/service_specific_credentials.rs` drives).
const CREDENTIAL_SERVICES: &[&str] = &["cassandra.amazonaws.com", "codecommit.amazonaws.com"];

/// One row of the update table: the user as created, and the optional members the update
/// carries.
struct UpdateCase {
    case: &'static str,
    user_name: String,
    path: String,
    new_user_name: Option<String>,
    new_path: Option<String>,
}

/// The rows: every combination of the two optional members (name only, path only, both,
/// neither), over names spanning the `^[\w+=,.@-]+$` alphabet and the shortest and longest
/// lengths `userNameType` admits (1 and 64 characters), and paths spanning the account
/// root, one segment, a nested path and the longest `pathType` admits (512 characters) —
/// `smithy_query(query="operation", operation="UpdateUser")`.
fn update_cases() -> Vec<UpdateCase> {
    // The longest path the model admits: 512 characters, both slashes included.
    let longest_path = format!("/{}/", "p".repeat(510));
    vec![
        UpdateCase {
            case: "a rename alone",
            user_name: "alice".to_string(),
            path: "/".to_string(),
            new_user_name: Some("Alice.Renamed".to_string()),
            new_path: None,
        },
        UpdateCase {
            case: "a re-path alone",
            user_name: "payroll".to_string(),
            path: "/finance/".to_string(),
            new_user_name: None,
            new_path: Some("/finance/payroll/".to_string()),
        },
        UpdateCase {
            case: "a rename and a re-path at once",
            user_name: "User_7+x=y,z.w@example-1".to_string(),
            path: "/eng/".to_string(),
            new_user_name: Some("renamed+9=a,b.c@example-2".to_string()),
            new_path: Some("/eng/tools/platform/".to_string()),
        },
        UpdateCase {
            case: "neither member, so nothing changes",
            user_name: "unchanged".to_string(),
            path: "/sales/west/".to_string(),
            new_user_name: None,
            new_path: None,
        },
        UpdateCase {
            case: "a re-spelling of the user's own name",
            user_name: "mixedcase".to_string(),
            path: "/".to_string(),
            new_user_name: Some("MIXEDCASE".to_string()),
            new_path: None,
        },
        UpdateCase {
            case: "the shortest name the model admits",
            user_name: "u".to_string(),
            path: "/eng/".to_string(),
            new_user_name: Some("v".to_string()),
            new_path: Some("/".to_string()),
        },
        UpdateCase {
            case: "to the longest name the model admits",
            user_name: "short".to_string(),
            path: "/".to_string(),
            new_user_name: Some("l".repeat(64)),
            new_path: None,
        },
        UpdateCase {
            case: "from the longest name the model admits",
            user_name: "y".repeat(64),
            path: "/a/".to_string(),
            new_user_name: Some("z".to_string()),
            new_path: None,
        },
        UpdateCase {
            case: "from a nested path to the account root",
            user_name: "nested".to_string(),
            path: "/eng/tools/platform/".to_string(),
            new_user_name: None,
            new_path: Some("/".to_string()),
        },
        UpdateCase {
            case: "to the longest path the model admits",
            user_name: "deep".to_string(),
            path: "/".to_string(),
            new_user_name: Some("deeper".to_string()),
            new_path: Some(longest_path),
        },
    ]
}

/// The ARN an IAM user at `path` carries under `user_name`.
fn user_arn(path: &str, user_name: &str) -> String {
    format!("arn:aws:iam::{}:user{path}{user_name}", account_id())
}

/// Create one user, answering the `(UserId, CreateDate)` its create MINTED, so a later
/// read is checked against stored values rather than against the fact that it answered.
async fn create_user(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    path: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
    (element(&body, "UserId"), element(&body, "CreateDate"))
}

/// One `UpdateUser` call carrying only the members the row names.
async fn update(
    router: &MultiServiceRouter,
    user_name: &str,
    new_user_name: Option<&str>,
    new_path: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("UserName", user_name)];
    if let Some(new_user_name) = new_user_name {
        params.push(("NewUserName", new_user_name));
    }
    if let Some(new_path) = new_path {
        params.push(("NewPath", new_path));
    }
    call(router, "UpdateUser", &params).await
}

/// The names the account holds besides [`ACCOUNT_CALLER`], as `ListUsers` reports them.
async fn fixture_users(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, listed) = call(router, "ListUsers", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListUsers 200, got {status}: {listed}"
    );
    elements(&listed, "UserName")
        .into_iter()
        .filter(|name| name != ACCOUNT_CALLER)
        .collect()
}

/// Assert that `action` refuses `user_name` with the modelled not-found.
async fn assert_not_found(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    user_name: &str,
) {
    let (status, body) = call(router, action, &[("UserName", user_name)]).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected {action} 404 for {user_name:?}, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "case {case:?}: expected the modelled awsQuery code from {action}, got {body}"
    );
}

/// RULE: the update leaves the SAME user under the name and path it names — the minted id
/// and create date unchanged, the ARN spelled from the name and path it now carries — and
/// the name it no longer carries resolves nowhere.
///
/// The rows are [`update_cases`]. Each addressed user is named in UPPER case, because IAM
/// entity names "are not distinguished by case", so the case a request spells the addressed
/// name in is not part of what it identifies.
#[tokio::test]
async fn an_update_moves_the_same_user_to_the_name_and_path_it_names() {
    for UpdateCase {
        case,
        user_name,
        path,
        new_user_name,
        new_path,
    } in update_cases()
    {
        // A fresh account a row at a time, so the row's user is the only one the fixture
        // holds and "the account holds exactly this user afterwards" is assertable.
        let router = router();
        let (user_id, create_date) = create_user(&router, case, &user_name, &path).await;

        let addressed = user_name.to_ascii_uppercase();
        let (status, body) = update(
            &router,
            &addressed,
            new_user_name.as_deref(),
            new_path.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateUser 200 for a held user spelled {addressed:?}, \
             got {status}: {body}"
        );

        // A member the request omits leaves that half of the user's identity as it stands.
        let expected_name = new_user_name.clone().unwrap_or_else(|| user_name.clone());
        let expected_path = new_path.clone().unwrap_or_else(|| path.clone());

        let (status, read) = call(&router, "GetUser", &[("UserName", &expected_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated user to read back under {expected_name:?}, \
             got {status}: {read}"
        );
        for (member, expected) in [
            ("UserName", expected_name.clone()),
            ("Path", expected_path.clone()),
            ("Arn", user_arn(&expected_path, &expected_name)),
            // "The user's unique identifier remains the same": the id and the create date
            // identify the user over its lifetime, so an update must not remint either.
            ("UserId", user_id.clone()),
            ("CreateDate", create_date.clone()),
        ] {
            let actual = element(&read, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?} after the update, got {actual:?}"
            );
        }

        // One user, not two: the update MOVED the user rather than copying it.
        assert_eq!(
            fixture_users(&router, case).await,
            vec![expected_name.clone()],
            "case {case:?}: expected the account to hold exactly the updated user"
        );

        // The name the user no longer carries resolves nowhere — unless the update only
        // re-spelled the name it already had, which is the same name: IAM entity names
        // "are not distinguished by case".
        if user_name.eq_ignore_ascii_case(&expected_name) {
            let (status, body) = call(&router, "GetUser", &[("UserName", &user_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the re-spelled name to still name the user, got \
                 {status}: {body}"
            );
            assert_eq!(
                element(&body, "UserName"),
                expected_name,
                "case {case:?}: expected the stored spelling to be the updated one, got {body}"
            );
        } else {
            assert_not_found(&router, case, "GetUser", &user_name).await;
            let (_, body) = call(&router, "GetUser", &[("UserName", &user_name)]).await;
            assert_eq!(
                element(&body, "Message"),
                format!("The user with name {user_name} cannot be found."),
                "case {case:?}: expected the refusal to name the entity and the name the \
                 request carried, got {:?}",
                element(&body, "Message")
            );
        }
    }
}

/// One row of the subordinate-record table: how many of each subordinate record the user
/// holds, and whether the update renames it (which re-keys everything held under its name)
/// or only re-paths it.
struct FollowCase {
    case: &'static str,
    held: usize,
    new_user_name: Option<&'static str>,
    new_path: Option<&'static str>,
}

/// The rows: one and two subordinate records of each kind — two being the most access keys
/// and the most service-specific credentials per service an IAM user may hold — over an
/// update that renames the user, one that renames and re-paths it, and one that only
/// re-paths it.
const FOLLOW_CASES: &[FollowCase] = &[
    FollowCase {
        case: "one of each, renamed",
        held: 1,
        new_user_name: Some("renamed.once"),
        new_path: None,
    },
    FollowCase {
        case: "two of each, renamed and re-pathed",
        held: 2,
        new_user_name: Some("Renamed_Twice"),
        new_path: Some("/eng/platform/"),
    },
    FollowCase {
        case: "two of each, re-pathed only",
        held: 2,
        new_user_name: None,
        new_path: Some("/"),
    },
];

/// RULE: every record this service keys by the user's name belongs to the USER, so it moves
/// with an update that changes that name — its embedded and attached policies, its access
/// keys, its service-specific credentials, its console password, its group memberships and
/// its tags — and nothing is left behind under the name it released, so a later user created
/// with that name inherits none of it.
#[tokio::test]
async fn a_renamed_user_keeps_the_records_the_account_holds_about_it() {
    for FollowCase {
        case,
        held,
        new_user_name,
        new_path,
    } in FOLLOW_CASES
    {
        let router = router();
        let user_name = "engineer";
        let path = "/eng/";
        let tag_list = tags(&[("team", "platform"), ("cost-center", "4711")]);
        let params = with_tag_params(
            vec![param("UserName", user_name), param("Path", path)],
            &tag_list[..*held],
        );
        let (status, created) = call(&router, "CreateUser", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
        );
        let user_id = element(&created, "UserId");

        // The records the user holds before the update. Each kind is read back through its
        // own implemented reader afterwards.
        let embedded: Vec<String> = (0..*held).map(|index| format!("inline{index}")).collect();
        for policy_name in &embedded {
            let (status, body) = call(
                &router,
                "PutUserPolicy",
                &[
                    ("UserName", user_name),
                    ("PolicyName", policy_name),
                    ("PolicyDocument", EMBEDDED_DOCUMENT),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {body}"
            );
        }
        let attached: Vec<&str> = AWS_MANAGED_POLICIES.iter().copied().take(*held).collect();
        for policy_arn in &attached {
            let (status, body) = call(
                &router,
                "AttachUserPolicy",
                &[("UserName", user_name), ("PolicyArn", policy_arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture AttachUserPolicy must succeed, got {status}: {body}"
            );
        }
        let mut access_keys = Vec::new();
        for _ in 0..*held {
            let (status, body) = call(&router, "CreateAccessKey", &[("UserName", user_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {body}"
            );
            access_keys.push(element(&body, "AccessKeyId"));
        }
        access_keys.sort_unstable();
        let services: Vec<&str> = CREDENTIAL_SERVICES.iter().copied().take(*held).collect();
        for service in &services {
            let (status, body) = call(
                &router,
                "CreateServiceSpecificCredential",
                &[("UserName", user_name), ("ServiceName", service)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture CreateServiceSpecificCredential must succeed, got \
                 {status}: {body}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateLoginProfile",
            &[("UserName", user_name), ("Password", CONSOLE_PASSWORD)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateLoginProfile must succeed, got {status}: {body}"
        );
        let groups: Vec<String> = (0..*held).map(|index| format!("group{index}")).collect();
        for group_name in &groups {
            let (status, body) = call(&router, "CreateGroup", &[("GroupName", group_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture CreateGroup must succeed, got {status}: {body}"
            );
            let (status, body) = call(
                &router,
                "AddUserToGroup",
                &[("GroupName", group_name), ("UserName", user_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture AddUserToGroup must succeed, got {status}: {body}"
            );
        }

        let (status, body) = update(&router, user_name, *new_user_name, *new_path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateUser 200, got {status}: {body}"
        );
        let updated = new_user_name.unwrap_or(user_name);

        // The same user, with the tags it carried.
        let (status, read) = call(&router, "GetUser", &[("UserName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated user to read back, got {status}: {read}"
        );
        assert_eq!(
            element(&read, "UserId"),
            user_id,
            "case {case:?}: expected the updated user to keep its minted id, got {read}"
        );
        assert_eq!(
            reported_tags(&read),
            expected_map(&tag_list[..*held]),
            "case {case:?}: expected the tags to travel with the user, got {read}"
        );

        // The embedded policies stayed embedded, documents and all.
        let (status, listed) = call(&router, "ListUserPolicies", &[("UserName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListUserPolicies 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "member"),
            embedded,
            "case {case:?}: expected the user to keep its {held} embedded policy(s), got {listed}"
        );
        for policy_name in &embedded {
            let (status, got) = call(
                &router,
                "GetUserPolicy",
                &[("UserName", updated), ("PolicyName", policy_name)],
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

        // The attachments stayed attached.
        let (status, listed) = call(
            &router,
            "ListAttachedUserPolicies",
            &[("UserName", updated)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAttachedUserPolicies 200, got {status}: {listed}"
        );
        let mut expected_arns: Vec<String> = attached.iter().map(ToString::to_string).collect();
        expected_arns.sort_unstable();
        let mut reported_arns = elements(&listed, "PolicyArn");
        reported_arns.sort_unstable();
        assert_eq!(
            reported_arns, expected_arns,
            "case {case:?}: expected the user to keep its {held} attachment(s), got {listed}"
        );

        // The access keys are still the user's own, reported under the name it now carries.
        let (status, listed) = call(&router, "ListAccessKeys", &[("UserName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAccessKeys 200, got {status}: {listed}"
        );
        let mut reported_keys = elements(&listed, "AccessKeyId");
        reported_keys.sort_unstable();
        assert_eq!(
            reported_keys, access_keys,
            "case {case:?}: expected the user to keep its {held} access key(s), got {listed}"
        );
        assert_eq!(
            elements(&listed, "UserName"),
            vec![updated.to_string(); *held],
            "case {case:?}: expected every key to report the name the user now carries, got \
             {listed}"
        );

        // So are the service-specific credentials.
        let (status, listed) = call(
            &router,
            "ListServiceSpecificCredentials",
            &[("UserName", updated)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListServiceSpecificCredentials 200, got {status}: {listed}"
        );
        let mut reported_services = elements(&listed, "ServiceName");
        reported_services.sort_unstable();
        let mut expected_services: Vec<String> = services.iter().map(ToString::to_string).collect();
        expected_services.sort_unstable();
        assert_eq!(
            reported_services, expected_services,
            "case {case:?}: expected the user to keep its {held} service credential(s), got \
             {listed}"
        );
        assert_eq!(
            elements(&listed, "UserName"),
            vec![updated.to_string(); *held],
            "case {case:?}: expected every service credential to report the name the user now \
             carries, got {listed}"
        );

        // The console password travelled with the user, and so did its memberships —
        // reported both by the user's own list and by each group's roster.
        let (status, profile) = call(&router, "GetLoginProfile", &[("UserName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetLoginProfile 200, got {status}: {profile}"
        );
        assert_eq!(
            element(&profile, "UserName"),
            updated,
            "case {case:?}: expected the profile to report the name the user now carries, got \
             {profile}"
        );
        let (status, listed) = call(&router, "ListGroupsForUser", &[("UserName", updated)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListGroupsForUser 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "GroupName"),
            groups,
            "case {case:?}: expected the user to keep its {held} membership(s), got {listed}"
        );
        for group_name in &groups {
            let (status, roster) = call(&router, "GetGroup", &[("GroupName", group_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetGroup 200, got {status}: {roster}"
            );
            assert_eq!(
                elements(&roster, "UserName"),
                vec![updated.to_string()],
                "case {case:?}: expected {group_name:?} to hold the user under the name it now \
                 carries, got {roster}"
            );
        }

        if updated.eq_ignore_ascii_case(user_name) {
            // The update kept the name, so there is no released name to check.
            continue;
        }

        // Nothing is left behind under the released name: every reader refuses it, and a
        // user later created with it holds none of what its predecessor held.
        for action in [
            "GetUser",
            "ListUserPolicies",
            "ListAttachedUserPolicies",
            "ListAccessKeys",
            "ListGroupsForUser",
            "GetLoginProfile",
        ] {
            assert_not_found(&router, case, action, user_name).await;
        }
        create_user(&router, case, user_name, path).await;
        let (status, read) = call(&router, "GetUser", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created user must read back, got {status}: {read}"
        );
        assert_ne!(
            element(&read, "UserId"),
            user_id,
            "case {case:?}: expected the re-created user to be a different user, got {read}"
        );
        assert!(
            reported_tags(&read).is_empty(),
            "case {case:?}: expected the re-created user to inherit no tag, got {read}"
        );
        let (status, listed) = call(&router, "ListUserPolicies", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListUserPolicies 200 for the re-created user, got {status}: \
             {listed}"
        );
        assert!(
            elements(&listed, "member").is_empty(),
            "case {case:?}: expected the re-created user to embed no policy, got {listed}"
        );
        let (status, listed) = call(&router, "ListAccessKeys", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAccessKeys 200 for the re-created user, got {status}: \
             {listed}"
        );
        assert!(
            elements(&listed, "AccessKeyId").is_empty(),
            "case {case:?}: expected the re-created user to hold no access key, got {listed}"
        );
        let (status, listed) = call(
            &router,
            "ListGroupsForUser",
            &[("UserName", user_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListGroupsForUser 200 for the re-created user, got {status}: \
             {listed}"
        );
        assert!(
            elements(&listed, "GroupName").is_empty(),
            "case {case:?}: expected the re-created user to hold no membership, got {listed}"
        );
    }
}

/// RULE: a new name ANOTHER user is already held under is refused with
/// `EntityAlreadyExists` (HTTP 409) rather than merging the two, and the refused update
/// changes nothing; the case the new name is spelled in does not matter, because IAM entity
/// names "are not distinguished by case".
///
/// The rows differ along the case the taken name is spelled in, the path the other user
/// sits at (so the conflict is about the NAME and not the ARN), and whether the refused
/// update also asked for a new path.
#[tokio::test]
async fn a_new_name_another_user_already_carries_is_refused() {
    // `(the spelling the update asks for, the other user's path, the new path asked for)`.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("auditor", "/", None),
        ("AUDITOR", "/", None),
        ("AuDiToR", "/compliance/", None),
        ("auditor", "/compliance/deep/", Some("/moved/")),
    ];

    for (spelling, other_path, new_path) in rows {
        let case = format!("{spelling} at {other_path}");
        let router = router();
        let (user_id, _) = create_user(&router, &case, "admin", "/eng/").await;
        let (other_id, _) = create_user(&router, &case, "auditor", other_path).await;

        let (status, body) = update(&router, "admin", Some(spelling), *new_path).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected UpdateUser 409 for a name another user carries, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("User with name {spelling} already exists."),
            "case {case:?}: expected the refusal to name the entity and the name the request \
             carried, got {:?}",
            element(&body, "Message")
        );

        // The refused update changed nothing: both users read back as their creates minted
        // them.
        for (user_name, path, user_id) in [
            ("admin", "/eng/", &user_id),
            ("auditor", other_path, &other_id),
        ] {
            let (status, read) = call(&router, "GetUser", &[("UserName", user_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {user_name:?} must survive the refused update, got {status}: \
                 {read}"
            );
            for (member, expected) in [
                ("UserName", user_name.to_string()),
                ("Path", path.to_string()),
                ("UserId", user_id.to_string()),
            ] {
                let actual = element(&read, member);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the refused update changed {user_name:?}'s {member}: \
                     expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// RULE: a name the account holds no user under names no user, so the update is refused
/// with `NoSuchEntity` (HTTP 404) and creates nothing.
///
/// The rows span names the account never held — including near-misses of a held one — and
/// each combination of the optional members, because a name that names no user is refused
/// whatever the update asked to change.
#[tokio::test]
async fn an_update_of_a_name_the_account_holds_no_user_under_is_refused() {
    let router = router();
    create_user(&router, "the fixture", "admin", "/eng/").await;

    for absent in ["nobody", "admi", "admin1", "dmin"] {
        for (new_user_name, new_path) in [
            (Some("renamed"), None),
            (None, Some("/moved/")),
            (Some("renamed"), Some("/moved/")),
            (None, None),
        ] {
            let case = format!("{absent} -> {new_user_name:?} at {new_path:?}");
            let (status, body) = update(&router, absent, new_user_name, new_path).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected UpdateUser 404 for an unheld name, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!("The user with name {absent} cannot be found."),
                "case {case:?}: expected the refusal to name the entity and the name the \
                 request carried, got {:?}",
                element(&body, "Message")
            );

            // The refused update created nothing, under either name.
            for name in [absent, new_user_name.unwrap_or(absent)] {
                let (status, read) = call(&router, "GetUser", &[("UserName", name)]).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the refused update to leave {name:?} naming no \
                     user, got {status}: {read}"
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
/// — the user the refusal must NAME, which is the one of the two names the decision went
/// against.
struct AuthorizationCase {
    case: &'static str,
    policies: Vec<(String, String)>,
    renames: bool,
    expect: Expect,
    refused_on: &'static str,
}

/// The user the fixture account holds, and the name an update moves it to.
const HELD_USER: &str = "payroll.clerk";
const NEW_USER: &str = "finance.clerk";

/// The rows: an `Allow` over every resource, one scoped to each of the two names alone, one
/// scoped to both, none at all, one for another action, and a `Deny` scoped to each of the
/// two names on top of a blanket `Allow` — the dimensions the "both names" rule generalises
/// over.
fn authorization_cases() -> Vec<AuthorizationCase> {
    let action = "iam:UpdateUser";
    let held_arn = user_arn("/", HELD_USER);
    let new_arn = user_arn("/", NEW_USER);
    vec![
        AuthorizationCase {
            case: "an Allow naming the action on every resource",
            policies: vec![("allow-all".to_string(), statement("Allow", action, "*"))],
            renames: true,
            expect: Expect::Served,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "an Allow scoped to both the old and the new name",
            policies: vec![
                ("allow-old".to_string(), statement("Allow", action, &held_arn)),
                ("allow-new".to_string(), statement("Allow", action, &new_arn)),
            ],
            renames: true,
            expect: Expect::Served,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "an Allow scoped to the old name only",
            policies: vec![("allow-old".to_string(), statement("Allow", action, &held_arn))],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: NEW_USER,
        },
        AuthorizationCase {
            case: "an Allow scoped to the new name only",
            policies: vec![("allow-new".to_string(), statement("Allow", action, &new_arn))],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "an Allow scoped to the old name, for an update that renames nothing",
            policies: vec![("allow-old".to_string(), statement("Allow", action, &held_arn))],
            renames: false,
            expect: Expect::Served,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "no identity-based policy at all",
            policies: Vec::new(),
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "an Allow for another action only",
            policies: vec![(
                "allow-create".to_string(),
                statement("Allow", "iam:CreateUser", "*"),
            )],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "a Deny scoped to the old name on top of a blanket Allow",
            policies: vec![
                ("allow-all".to_string(), statement("Allow", action, "*")),
                ("deny-old".to_string(), statement("Deny", action, &held_arn)),
            ],
            renames: true,
            expect: Expect::ExplicitDeny,
            refused_on: HELD_USER,
        },
        AuthorizationCase {
            case: "a Deny scoped to the new name on top of a blanket Allow",
            policies: vec![
                ("allow-all".to_string(), statement("Allow", action, "*")),
                ("deny-new".to_string(), statement("Deny", action, &new_arn)),
            ],
            renames: true,
            expect: Expect::ExplicitDeny,
            refused_on: NEW_USER,
        },
    ]
}

/// RULE: the call is decided against the caller's identity-based policies on BOTH the name
/// the user carries and the name the update would leave it under — "the requester must have
/// appropriate permissions on both the source object and the target object" — and the
/// decision is reached before anything changes, so a refused update leaves the user exactly
/// as it was.
#[tokio::test]
async fn an_update_is_decided_against_the_old_and_the_new_name() {
    let iam_action = "iam:UpdateUser";

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
        let (user_id, create_date) = create_user(&router, case, HELD_USER, "/").await;

        let caller = format!("caller{row}");
        let access_key_id = principal(&router, case, &caller, &policies).await;
        let mut params = vec![("UserName", HELD_USER)];
        if renames {
            params.push(("NewUserName", NEW_USER));
        }
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), "UpdateUser", &params).await;

        if expect == Expect::Served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            );
            let expected_name = if renames { NEW_USER } else { HELD_USER };
            let (status, read) = call(&router, "GetUser", &[("UserName", expected_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the served update to leave the user under \
                 {expected_name:?}, got {status}: {read}"
            );
            assert_eq!(
                element(&read, "UserId"),
                user_id,
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
            &user_arn("/", &caller),
            iam_action,
            &format!("user {refused_on}"),
            &reason,
        );

        // The refused update changed nothing: the user is still held under the name and
        // path its create gave it, and the name it was asked to move to names no user.
        let (status, read) = call(&router, "GetUser", &[("UserName", HELD_USER)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the user must survive the refused update, got {status}: {read}"
        );
        for (member, expected) in [
            ("UserName", HELD_USER.to_string()),
            ("Path", "/".to_string()),
            ("UserId", user_id.clone()),
            ("CreateDate", create_date.clone()),
        ] {
            let actual = element(&read, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: the refused update changed {member}: expected {expected:?}, \
                 got {actual:?}"
            );
        }
        let (status, read) = call(&router, "GetUser", &[("UserName", NEW_USER)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused update to leave {NEW_USER:?} naming no user, \
             got {status}: {read}"
        );
    }
}
