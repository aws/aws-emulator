// SPDX-License-Identifier: CC0-1.0

//! `DeleteGroup` and `GetGroup`, end to end over the awsQuery wire.
//!
//! RULE: a delete RELEASES the stored entity the request names, so the name resolves
//! nowhere afterwards — a read and a second delete both answer the modelled
//! `NoSuchEntityException`, the listing no longer reports it, and the name is free for a
//! new create — while every other entity the account holds is untouched; an entity that
//! still holds a subordinate entity is refused with `DeleteConflictException` instead; a
//! name the account never held is refused with that same not-found error rather than
//! reported as deleted; and either call is decided against the caller's identity-based
//! policies before anything is reported or removed.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! group's name alphabet and length (the shortest and longest `groupNameType` admits),
//! its path depth, the case the delete spells the name in, which position in the
//! account's group set is released, and — for the authorization suite — the effect, the
//! action element and the resource a statement is scoped to. No single request is the
//! specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, so every suite in
//! this crate builds and reads a request the same way; `tests/all/group_listing.rs` owns
//! the `CreateGroup`/`ListGroups` behaviour this suite creates its fixtures with.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// The fixture groups: `(GroupName, Path)` rows spanning the whole `^[\w+=,.@-]+$` name
/// alphabet, the shortest and longest names `groupNameType` admits (1 and 128
/// characters, `smithy_query(query="operation", operation="DeleteGroup")`), and the
/// root, one-segment and nested paths `pathType` admits.
fn groups() -> Vec<(String, &'static str)> {
    vec![
        ("admins".to_string(), "/"),
        ("g".to_string(), "/"),
        ("Group_7+x=y,z.w@example-1".to_string(), "/eng/"),
        ("nested".to_string(), "/eng/tools/platform/"),
        ("l".repeat(128), "/sales/west/"),
    ]
}

/// Create every fixture group on a fresh router, and answer it.
async fn with_fixture_groups() -> MultiServiceRouter {
    let router = router();
    for (group_name, path) in groups() {
        let (status, body) = call(
            &router,
            "CreateGroup",
            &[("GroupName", group_name.as_str()), ("Path", path)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: the fixture create must succeed, got {status}: {body}"
        );
    }
    router
}

/// The groups the account holds as `(GroupName, GroupId)`, read back through
/// `ListGroups` — the implemented reader for a persisted group.
async fn held_groups(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let (status, body) = call(router, "ListGroups", &[]).await;
    assert_eq!(status, 200, "expected ListGroups 200, got {status}: {body}");
    let names = elements(&body, "GroupName");
    let ids = elements(&body, "GroupId");
    assert_eq!(
        names.len(),
        ids.len(),
        "expected every listed group to carry one GroupId, got {} for {} groups: {body}",
        ids.len(),
        names.len()
    );
    names.into_iter().zip(ids).collect()
}

/// RULE: deleting a held group releases exactly that group — the name resolves nowhere
/// afterwards and the listing drops it, while every group still held reads back with the
/// id its create minted.
///
/// The rows are the fixture groups, released one after another, so each delete is taken
/// against a different remaining set (all of them, then all but one, …, then the last).
/// The delete spells the name in UPPER case, because IAM entity names "are not
/// distinguished by case".
#[tokio::test]
async fn deleting_a_group_releases_that_group_and_leaves_the_others() {
    let router = with_fixture_groups().await;

    // The ids the creates minted, so "unchanged" is asserted against a stored value
    // rather than against the fact that a read answered at all.
    let mut held = held_groups(&router).await;
    assert_eq!(
        held.len(),
        groups().len(),
        "the fixture creates must all be held: {held:?}"
    );

    while let Some((group_name, _)) = held.first().cloned() {
        let spelling = group_name.to_ascii_uppercase();
        let (status, body) =
            call(&router, "DeleteGroup", &[("GroupName", spelling.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: expected DeleteGroup 200 for a held group spelled \
             {spelling:?}, got {status}: {body}"
        );

        // The released name resolves nowhere: neither a read nor a second delete.
        for action in ["GetGroup", "DeleteGroup"] {
            let (status, body) = call(&router, action, &[("GroupName", group_name.as_str())]).await;
            assert_eq!(
                status, 404,
                "case {group_name:?}: expected {action} 404 after the delete, got {status}: \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {group_name:?}: expected {action} to answer the modelled awsQuery code, \
                 got {body}"
            );
        }

        held.remove(0);
        assert_eq!(
            held_groups(&router).await,
            held,
            "case {group_name:?}: expected the delete to release that group and leave every \
             other one with the id its create minted"
        );
        // Every group still held reads back through `GetGroup` with the id its create
        // minted, so the delete released one group and not a neighbour.
        for (still_held, group_id) in &held {
            let (status, body) =
                call(&router, "GetGroup", &[("GroupName", still_held.as_str())]).await;
            assert_eq!(
                status, 200,
                "case {group_name:?}: releasing it released {still_held:?} too, got {status}: \
                 {body}"
            );
            assert_eq!(
                element(&body, "GroupId"),
                *group_id,
                "case {group_name:?}: releasing it changed the stored {still_held:?}, got {body}"
            );
        }
    }
}

/// RULE: a name the account does not hold is refused with `NoSuchEntity` (HTTP 404)
/// rather than reported as deleted; the refusal names the entity type and the name the
/// request carried, and changes nothing.
///
/// The rows are names the account never held: one unrelated, two near-misses of a held
/// name, the held name with one character appended, one at the longest length
/// `groupNameType` admits, and one spanning the rest of the name alphabet.
#[tokio::test]
async fn deleting_a_group_the_account_does_not_hold_is_not_found() {
    let router = with_fixture_groups().await;
    let before = held_groups(&router).await;

    let longest = "n".repeat(128);
    for absent in [
        "nobody",
        "admin",
        "admins1",
        "dmins",
        longest.as_str(),
        "absent+=,.@-_1",
    ] {
        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", absent)]).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected DeleteGroup 404 for an unheld name, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The group with name {absent} cannot be found."),
            "case {absent:?}: expected the refusal to name the entity and the name the \
             request carried, got {:?}",
            element(&body, "Message")
        );
    }

    assert_eq!(
        held_groups(&router).await,
        before,
        "expected the refused deletes to release nothing"
    );
}

/// RULE: a released name is free again — a create may take it, and the new entity is a
/// NEW one (its own minted id), not the released one coming back.
///
/// The rows are the fixture groups, each released and then re-created at another path.
#[tokio::test]
async fn a_released_group_name_can_be_created_again_as_a_new_group() {
    let router = with_fixture_groups().await;
    let minted = held_groups(&router).await;

    for (group_name, path) in groups() {
        let first_id = minted
            .iter()
            .find(|(held, _)| *held == group_name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("case {group_name:?}: the fixture group must be held"));

        let (status, body) =
            call(&router, "DeleteGroup", &[("GroupName", group_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: expected DeleteGroup 200, got {status}: {body}"
        );

        // The name is free: a create takes it, with a different path to show the new
        // group is built from the new request rather than restored.
        let (status, recreated) = call(
            &router,
            "CreateGroup",
            &[
                ("GroupName", group_name.as_str()),
                ("Path", "/recreated/"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: expected a released name to be free, got {status}: {recreated}"
        );
        assert_ne!(
            element(&recreated, "GroupId"),
            first_id,
            "case {group_name:?}: expected a NEW group id after the release, got the old \
             {first_id:?}"
        );
        assert_eq!(
            element(&recreated, "Path"),
            "/recreated/",
            "case {group_name:?}: expected the re-created group to carry the new path, not the \
             released {path:?}, got {recreated}"
        );
    }
}

/// RULE: a reporting read answers the STORED record the name identifies — every
/// required member of the entity's shape, as its create minted them — together with the
/// collection that entity owns, reporting an empty collection as empty rather than as an
/// error; and the read changes nothing a later caller sees.
///
/// The rows are the fixture groups, each read back under three spellings of its name
/// (as created, upper case, lower case), because IAM entity names "are not distinguished
/// by case" — and the record reported is the stored one, which keeps the case the create
/// chose.
#[tokio::test]
async fn get_group_reports_the_stored_group_and_its_members() {
    let router = with_fixture_groups().await;
    let minted = held_groups(&router).await;

    for (group_name, path) in groups() {
        let group_id = minted
            .iter()
            .find(|(held, _)| *held == group_name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("case {group_name:?}: the fixture group must be held"));

        for spelling in [
            group_name.clone(),
            group_name.to_ascii_uppercase(),
            group_name.to_ascii_lowercase(),
        ] {
            let case = format!("{group_name} read as {spelling}");
            let (status, body) =
                call(&router, "GetGroup", &[("GroupName", spelling.as_str())]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetGroup 200 for a held group, got {status}: {body}"
            );
            for (member, expected) in [
                ("GroupName", group_name.clone()),
                ("Path", path.to_string()),
                ("GroupId", group_id.clone()),
                (
                    "Arn",
                    format!("arn:aws:iam::{}:group{path}{group_name}", account_id()),
                ),
            ] {
                assert_eq!(
                    element(&body, member),
                    expected,
                    "case {case:?}: expected the stored {member}, got {:?}",
                    element(&body, member)
                );
            }
            // The group owns no member while nothing persists a membership, so the one
            // page holds the whole list: it is empty, complete and carries no token.
            assert!(
                elements(&body, "UserName").is_empty(),
                "case {case:?}: expected no member for a group nothing joined, got {body}"
            );
            assert_eq!(
                element(&body, "IsTruncated"),
                "false",
                "case {case:?}: expected IsTruncated false for a complete page, got {body}"
            );
            assert!(
                elements(&body, "Marker").is_empty(),
                "case {case:?}: a complete page must carry no Marker, got {body}"
            );
        }
    }

    // A read cannot change what a later caller sees.
    assert_eq!(
        held_groups(&router).await,
        minted,
        "expected the reads to leave every stored group as its create minted it"
    );

    // Rows spanning names the account never held, including near-misses of a held one.
    for absent in ["nobody", "admin", "admins1", "dmins"] {
        let (status, body) = call(&router, "GetGroup", &[("GroupName", absent)]).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected GetGroup 404 for an unheld name, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The group with name {absent} cannot be found."),
            "case {absent:?}: expected the refusal to name the entity and the name the \
             request carried, got {:?}",
            element(&body, "Message")
        );
    }

    // A token this service never issued is refused rather than read as the start of the
    // collection: the rows are a word, a short token and one at the longest length
    // `markerType` admits (320 characters).
    let longest = "z".repeat(320);
    for token in ["not-a-marker", "zz", longest.as_str()] {
        let (status, body) = call(
            &router,
            "GetGroup",
            &[("GroupName", "admins"), ("Marker", token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected a foreign Marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {token:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: an entity that still holds a subordinate entity is NOT deleted — the delete is
/// refused with `DeleteConflict` (HTTP 409) and the entity, with everything it holds, is
/// still there afterwards — while one holding none is released.
///
/// The rows differ along how many inline policies the group embeds (one, two, three) and
/// the size of the documents embedded, because the rule is about whether a subordinate
/// entity exists and not about what it says.
#[tokio::test]
async fn a_group_that_still_embeds_a_policy_is_not_deleted() {
    // A policy document is a JSON object carrying a `Statement`, which is what
    // `PutGroupPolicy` requires of one (`tests/all/group_inline_policies.rs`).
    let documents = [
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#.to_string(),
        r#"{"Statement":[{"Sid":"short"}]}"#.to_string(),
        format!(r#"{{"Statement":[{{"Sid":"{}"}}]}}"#, "p".repeat(512)),
    ];

    for embed in 1..=documents.len() {
        let router = router();
        let group_name = format!("holder{embed}");
        let (status, created) = call(
            &router,
            "CreateGroup",
            &[("GroupName", group_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {embed} policies: the fixture create must succeed, got {status}: {created}"
        );
        let group_id = element(&created, "GroupId");

        for (index, document) in documents.iter().take(embed).enumerate() {
            let policy_name = format!("inline-{index}");
            let (status, put) = call(
                &router,
                "PutGroupPolicy",
                &[
                    ("GroupName", group_name.as_str()),
                    ("PolicyName", policy_name.as_str()),
                    ("PolicyDocument", document.as_str()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {embed} policies: embedding {policy_name:?} must succeed, got {status}: \
                 {put}"
            );
        }

        let (status, body) = call(
            &router,
            "DeleteGroup",
            &[("GroupName", group_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {embed} policies: expected DeleteGroup 409 while the group still embeds a \
             policy, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {embed} policies: expected the modelled awsQuery code, got {body}"
        );

        // The refused delete released nothing: the group reads back unchanged, and so
        // does every policy it embeds.
        let (status, read) = call(
            &router,
            "GetGroup",
            &[("GroupName", group_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {embed} policies: the group must survive the refused delete, got {status}: \
             {read}"
        );
        assert_eq!(
            element(&read, "GroupId"),
            group_id,
            "case {embed} policies: the refused delete changed the stored group, got {read}"
        );
        for (index, document) in documents.iter().take(embed).enumerate() {
            let policy_name = format!("inline-{index}");
            let (status, got) = call(
                &router,
                "GetGroupPolicy",
                &[
                    ("GroupName", group_name.as_str()),
                    ("PolicyName", policy_name.as_str()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {embed} policies: {policy_name:?} must survive the refused delete, got \
                 {status}: {got}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&got, "PolicyDocument"),
                *document,
                "case {embed} policies: the refused delete changed {policy_name:?}, got {got}"
            );
        }

        // A group embedding nothing, in the same account, is released as usual: it is
        // the embedded policy that blocks the delete, not the account's state at large.
        let (status, body) = call(&router, "CreateGroup", &[("GroupName", "unembedded")]).await;
        assert_eq!(
            status, 200,
            "case {embed} policies: the second fixture create must succeed, got {status}: {body}"
        );
        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", "unembedded")]).await;
        assert_eq!(
            status, 200,
            "case {embed} policies: expected a group embedding no policy to be released, got \
             {status}: {body}"
        );
    }
}

/// A one-statement identity policy document with `effect` on `action` over `resource`.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Create a user holding `policies`, and answer the access key id a request signs with
/// to speak for it — the fixture `tests/all/identity_policy_enforcement.rs` builds a
/// principal with, for the suites that drive a signed call.
async fn principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    policies: &[(&str, String)],
) -> String {
    let (status, created) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
    );
    for (policy_name, document) in policies {
        let (status, put) = call(
            router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", document.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture PutUserPolicy {policy_name:?} must succeed, \
             got {status}: {put}"
        );
    }
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    element(&key, "AccessKeyId")
}

/// What a row of the authorization table expects the signed call to answer.
#[derive(Debug, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the call.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One row: the inline policies the calling user holds and the answer the rule
/// requires of the call it signs.
struct Case {
    case: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs, each put on the caller with
    /// `PutUserPolicy` in this order.
    policies: Vec<(&'static str, String)>,
    expect: Expect,
}

/// The rows for one IAM action over the group `target_arn` names, with `other_arn` the
/// ARN of a group the request does not address.
fn cases(iam_action: &str, target_arn: &str, other_arn: &str) -> Vec<Case> {
    // `iam:DeleteGroup` -> `iam:Delete*`: the wildcard run that covers the action
    // without naming it.
    let wildcard = format!(
        "{}*",
        iam_action
            .strip_suffix("Group")
            .expect("an IAM action on a group")
    );
    vec![
        Case {
            case: "an Allow naming the action on every resource",
            policies: vec![("allow-all", statement("Allow", iam_action, "*"))],
            expect: Expect::Served,
        },
        Case {
            case: "an Allow scoped to the addressed group",
            policies: vec![("allow-target", statement("Allow", iam_action, target_arn))],
            expect: Expect::Served,
        },
        Case {
            case: "an Allow whose action element is a wildcard run",
            policies: vec![("allow-star", statement("Allow", &wildcard, "*"))],
            expect: Expect::Served,
        },
        Case {
            case: "no identity-based policy at all",
            policies: Vec::new(),
            expect: Expect::NoAllow,
        },
        Case {
            case: "an Allow scoped to another group",
            policies: vec![("allow-other", statement("Allow", iam_action, other_arn))],
            expect: Expect::NoAllow,
        },
        Case {
            case: "an Allow for another action only",
            policies: vec![("allow-create", statement("Allow", "iam:CreateGroup", "*"))],
            expect: Expect::NoAllow,
        },
        Case {
            case: "a Deny on top of an Allow",
            policies: vec![
                ("allow-all", statement("Allow", iam_action, "*")),
                ("deny-action", statement("Deny", iam_action, "*")),
            ],
            expect: Expect::ExplicitDeny,
        },
    ]
}

/// RULE: a call signed by an IAM user is decided against that user's identity-based
/// policies BEFORE anything is reported or removed — a `Deny` about the request refuses
/// it whatever any `Allow` says, an `Allow` with no such `Deny` serves it, and a
/// principal with neither is refused with HTTP 403 `AccessDenied` naming the principal,
/// the action and the entity the request addressed — and a refused call changes nothing.
///
/// The rows differ along the operation called (the mutation and the read), the effect,
/// whether the action element is a literal or a wildcard run, and the resource the
/// statement is scoped to (every resource, the addressed group's own ARN, another
/// group's ARN).
#[tokio::test]
async fn a_group_call_is_decided_against_the_callers_identity_policies() {
    let target = "payroll";
    let target_arn = format!("arn:aws:iam::{}:group/{target}", account_id());
    let other_arn = format!("arn:aws:iam::{}:group/decoys", account_id());

    for action in ["DeleteGroup", "GetGroup"] {
        let iam_action = format!("iam:{action}");
        for (row, Case { case, policies, expect }) in
            cases(&iam_action, &target_arn, &other_arn)
                .into_iter()
                .enumerate()
        {
            // A fresh account a row at a time, so a row's policies are the only
            // documents in force when it calls.
            let router = router();
            let (status, created) = call(&router, "CreateGroup", &[("GroupName", target)]).await;
            assert_eq!(
                status, 200,
                "case {action}/{case:?}: the fixture CreateGroup must succeed, got {status}: \
                 {created}"
            );
            let group_id = element(&created, "GroupId");

            let user_name = format!("caller{row}");
            let access_key_id = principal(&router, case, &user_name, &policies).await;
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                action,
                &[("GroupName", target)],
            )
            .await;

            if expect == Expect::Served {
                assert_eq!(
                    status, 200,
                    "case {action}/{case:?}: expected the call to be served, got {status}: {body}"
                );
                // The mutation released the group; the read left it exactly as it was.
                let still_held = held_groups(&router)
                    .await
                    .contains(&(target.to_string(), group_id.clone()));
                assert_eq!(
                    still_held,
                    action == "GetGroup",
                    "case {action}/{case:?}: expected the group to be held afterwards only for \
                     the read, got still_held {still_held}"
                );
                continue;
            }

            assert_eq!(
                status, 403,
                "case {action}/{case:?}: expected HTTP 403, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {action}/{case:?}: expected the error code AccessDenied, got {:?}",
                error_code(&body)
            );
            let reason = if expect == Expect::ExplicitDeny {
                "with an explicit deny in an identity-based policy".to_string()
            } else {
                format!("because no identity-based policy allows the {iam_action} action")
            };
            assert_access_denied_message(
                &format!("{action}/{case}"),
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                &iam_action,
                &format!("group {target}"),
                &reason,
            );
            assert!(
                held_groups(&router)
                    .await
                    .contains(&(target.to_string(), group_id.clone())),
                "case {action}/{case:?}: expected the refused call to change nothing"
            );
        }
    }
}
