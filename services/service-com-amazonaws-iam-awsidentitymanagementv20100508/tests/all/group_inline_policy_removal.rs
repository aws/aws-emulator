// SPDX-License-Identifier: CC0-1.0

//! `DeleteGroupPolicy`, end to end over the awsQuery wire.
//!
//! RULE: deleting an inline policy embedded in an IAM group REMOVES the document the
//! group holds under that name, so the group stops depending on it — `GetGroupPolicy`
//! stops resolving it, `ListGroupPolicies` stops reporting it, the aggregate inline
//! policy size it occupied is free again, and once nothing else is embedded or attached
//! `DeleteGroup` releases the group instead of answering `DeleteConflict` — while a
//! group the account does not hold, or a policy name the group does not embed, is
//! refused with `NoSuchEntity` (HTTP 404) and changes nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how
//! many policies the group embeds, the alphabet and case of the group and policy names,
//! the group's path, the form and size of the documents, and which of several embedded
//! policies the delete names. The awsQuery harness (router, call, element, elements,
//! error_code) is the one `tests/all/user_lifecycle.rs` owns and every suite in this
//! crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The aggregate size, in characters, of all inline policies one group may hold.
/// AWS's "IAM and STS quotas" page: "Group policy size cannot exceed 5,120
/// characters". The same bound `tests/all/group_inline_policies.rs` asserts the embed
/// side of; here it is what a delete has to GIVE BACK.
const MAX_GROUP_POLICY_CHARACTERS: usize = 5_120;

/// The shortest document [`document_of`] can build, which is also the smallest document
/// the top-level IAM policy grammar admits in that form.
const SHORTEST_DOCUMENT_CHARACTERS: usize = 26;

/// Create a group, failing the test when the fixture create does not succeed.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: the fixture CreateGroup must succeed, got {status}: {body}"
    );
}

/// Embed `document` in `group_name` under `policy_name`, failing the test when the
/// fixture put does not succeed.
async fn put_policy(
    router: &MultiServiceRouter,
    group_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        "PutGroupPolicy",
        &[
            ("GroupName", group_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: the fixture PutGroupPolicy must succeed, got {status}: {body}"
    );
}

/// Delete one embedded policy, answering the status and body of the delete itself.
async fn delete_policy(
    router: &MultiServiceRouter,
    group_name: &str,
    policy_name: &str,
) -> (u16, String) {
    call(
        router,
        "DeleteGroupPolicy",
        &[("GroupName", group_name), ("PolicyName", policy_name)],
    )
    .await
}

/// Read one embedded policy back.
async fn get_policy(
    router: &MultiServiceRouter,
    group_name: &str,
    policy_name: &str,
) -> (u16, String) {
    call(
        router,
        "GetGroupPolicy",
        &[("GroupName", group_name), ("PolicyName", policy_name)],
    )
    .await
}

/// The names `ListGroupPolicies` reports for a group.
async fn listed_policies(router: &MultiServiceRouter, group_name: &str) -> Vec<String> {
    let (status, body) = call(router, "ListGroupPolicies", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: ListGroupPolicies must answer for a held group, got {status}: \
         {body}"
    );
    elements(&body, "member")
}

/// A policy document of exactly `characters` characters: the shortest document the
/// grammar admits, padded inside a `Sid` string, so a size rule can be exercised at any
/// point of its range.
fn document_of(characters: usize) -> String {
    assert!(
        characters >= SHORTEST_DOCUMENT_CHARACTERS,
        "a padded document cannot be shorter than {SHORTEST_DOCUMENT_CHARACTERS} characters"
    );
    let padding = "a".repeat(characters - SHORTEST_DOCUMENT_CHARACTERS);
    let document = format!(r#"{{"Statement":[{{"Sid":"{padding}"}}]}}"#);
    assert_eq!(
        document.chars().count(),
        characters,
        "the padded document must be exactly {characters} characters: {document}"
    );
    document
}

/// One group this suite embeds policies in and then empties again.
struct Case {
    case: &'static str,
    group_name: &'static str,
    path: &'static str,
    /// The policies embedded in it, as (name, document) pairs.
    policies: &'static [(&'static str, &'static str)],
}

/// Rows spanning the account root, a one-segment and a nested path; group and policy
/// names drawn from the whole `^[\w+=,.@-]+$` alphabet
/// (`smithy_query(query="operation", operation="DeleteGroupPolicy")`) in mixed case; one
/// to four embedded policies per group; and documents spanning a statement list, a bare
/// statement object, one with no `Version`, one indented with the tab, line feed and
/// carriage return `policyDocumentType`'s pattern admits, and the smallest document the
/// grammar admits.
const CASES: &[Case] = &[
    Case {
        case: "a single policy at the account root",
        group_name: "Admins",
        path: "/",
        policies: &[(
            "AllowRead",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        )],
    },
    Case {
        case: "two policies under a one-segment path",
        group_name: "billing_2",
        path: "/eng/",
        policies: &[
            (
                "deny_all",
                r#"{"Statement":{"Effect":"Deny","Action":"*","Resource":"*"}}"#,
            ),
            (
                "no+version",
                r#"{"Statement":[{"Effect":"Allow","Action":"fis:StartExperiment"}]}"#,
            ),
        ],
    },
    Case {
        case: "three policies under a nested path",
        group_name: "core.3",
        path: "/eng/tools/",
        policies: &[
            (
                "Policy.With.Id",
                r#"{"Version":"2012-10-17","Id":"one","Statement":[{"Effect":"Allow","Action":"ec2:*"}]}"#,
            ),
            (
                "indented=3",
                "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t{\"Effect\": \"Allow\", \"Action\": \"iam:PassRole\"}\r\n\t]\r\n}",
            ),
            ("minimal-3", r#"{"Statement":[]}"#),
        ],
    },
    Case {
        case: "four policies whose names span the alphabet",
        group_name: "data+4",
        path: "/sales/west/",
        policies: &[
            ("a_b", r#"{"Statement":[{"Effect":"Allow","Action":"sts:*"}]}"#),
            ("c=d", r#"{"Statement":[{"Effect":"Deny","Action":"iam:*"}]}"#),
            ("e.f@g", r#"{"Statement":[{"Effect":"Allow","Action":"s3:*"}]}"#),
            ("h-i,j", r#"{"Statement":[{"Effect":"Allow","Action":"kms:*"}]}"#),
        ],
    },
];

/// RULE: a delete removes exactly the policy it names from exactly the group it names —
/// the deleted name stops resolving through `GetGroupPolicy` and stops being reported by
/// `ListGroupPolicies`, and every other policy the group embeds is untouched.
///
/// Every other delete spells both names in UPPER case, because IAM entity and policy
/// names "are not distinguished by case".
#[tokio::test]
async fn deleting_an_embedded_policy_removes_only_that_policy() {
    for Case {
        case,
        group_name,
        path,
        policies,
    } in CASES
    {
        let router = router();
        create_group(&router, group_name, path).await;
        for (policy_name, document) in *policies {
            put_policy(&router, group_name, policy_name, document).await;
        }

        let mut remaining: Vec<(&str, &str)> = policies.to_vec();
        for (index, (policy_name, _)) in policies.iter().enumerate() {
            let spelling = if index % 2 == 0 {
                (*policy_name).to_string()
            } else {
                policy_name.to_ascii_uppercase()
            };
            let group_spelling = if index % 2 == 0 {
                (*group_name).to_string()
            } else {
                group_name.to_ascii_uppercase()
            };

            let (status, body) = delete_policy(&router, &group_spelling, &spelling).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected deleting {spelling:?} from {group_spelling:?} to \
                 succeed, got {status}: {body}"
            );

            remaining.retain(|(name, _)| name != policy_name);

            let (status, body) = get_policy(&router, group_name, policy_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {policy_name:?} must stop resolving after its delete, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code for {policy_name:?}, got \
                 {body}"
            );

            let listed = listed_policies(&router, group_name).await;
            assert!(
                !listed.contains(&(*policy_name).to_string()),
                "case {case:?}: {policy_name:?} is still listed after its delete: {listed:?}"
            );
            for (held, document) in &remaining {
                assert!(
                    listed.contains(&(*held).to_string()),
                    "case {case:?}: deleting {policy_name:?} also dropped {held:?} from the \
                     listing {listed:?}"
                );
                let (status, body) = get_policy(&router, group_name, held).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {held:?} must survive the delete of {policy_name:?}, got \
                     {status}: {body}"
                );
                assert_eq!(
                    crate::user_lifecycle::document_element(&body, "PolicyDocument"),
                    *document,
                    "case {case:?}: {held:?} must still report its own document, got {body}"
                );
            }
        }

        // The group embeds nothing now, so it is released rather than refused.
        let listed = listed_policies(&router, group_name).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected no embedded policy to be listed, got {listed:?}"
        );
        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteGroup 200 once every policy was deleted, got \
             {status}: {body}"
        );
    }
}

/// RULE: an embedded inline policy holds the group back from `DeleteGroup`
/// (`DeleteConflict`, HTTP 409) for exactly as long as it is embedded, so the delete of
/// the last one is what frees the group.
#[tokio::test]
async fn an_embedded_policy_blocks_the_group_delete_until_it_is_deleted() {
    for Case {
        case,
        group_name,
        path,
        policies,
    } in CASES
    {
        let router = router();
        create_group(&router, group_name, path).await;
        for (policy_name, document) in *policies {
            put_policy(&router, group_name, policy_name, document).await;
        }

        for (policy_name, _) in *policies {
            let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group_name)]).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected DeleteGroup 409 while a policy is embedded, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "DeleteConflict",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );

            let (status, body) = delete_policy(&router, group_name, policy_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected deleting {policy_name:?} to succeed, got {status}: \
                 {body}"
            );
        }

        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the last delete to free the group, got {status}: {body}"
        );
    }
}

/// RULE: a delete naming a group the account does not hold, or a policy name the group
/// does not embed, is refused with `NoSuchEntity` (HTTP 404), the refusal names the
/// entity it could not find and the name the request carried, and nothing is removed —
/// so a repeated delete is refused rather than reported as a second success.
#[tokio::test]
async fn deleting_a_policy_the_group_does_not_embed_is_not_found() {
    let router = router();
    create_group(&router, "Carrier", "/eng/").await;
    let embedded = [
        ("held-one", r#"{"Statement":[{"Effect":"Allow","Action":"s3:*"}]}"#),
        ("held_two", r#"{"Statement":[{"Effect":"Deny","Action":"iam:*"}]}"#),
    ];
    for (policy_name, document) in embedded {
        put_policy(&router, "Carrier", policy_name, document).await;
    }

    // Rows: a group nobody created, one whose name differs from the held group only
    // past its first characters, and one spanning the whole name alphabet. Each is
    // asked for a policy name that IS embedded in the held group, so only the group is
    // what makes it absent.
    for (case, absent_group) in [
        ("never created", "ghost"),
        ("a near miss on a held name", "Carriers"),
        ("a name spanning the alphabet", "n0_+=,.@-"),
    ] {
        for (policy_name, _) in embedded {
            let (status, body) = delete_policy(&router, absent_group, policy_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}/{policy_name:?}: expected 404 for an unheld group, got {status}: \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}/{policy_name:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!("The group with name {absent_group} cannot be found."),
                "case {case:?}/{policy_name:?}: expected the refusal to name the group and the \
                 name the request carried, got {:?}",
                element(&body, "Message")
            );
        }
    }

    // Rows: a name nobody embedded, one differing from a held name only in a separator
    // character, one differing only past its first characters, and one spanning the
    // alphabet.
    for (case, absent_policy) in [
        ("a policy nobody embedded", "Absent"),
        ("a held name with another separator", "held_one"),
        ("a near miss on a held name", "held-one-2"),
        ("a name spanning the alphabet", "a_b+c=d,e.f@g-h"),
    ] {
        let (status, body) = delete_policy(&router, "Carrier", absent_policy).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a policy the group does not embed, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The group policy with name {absent_policy} cannot be found."),
            "case {case:?}: expected the refusal to name the group policy and the name the \
             request carried, got {:?}",
            element(&body, "Message")
        );
    }

    // The refusals removed nothing: both embedded policies still resolve and are still
    // listed.
    let listed = listed_policies(&router, "Carrier").await;
    for (policy_name, document) in embedded {
        assert!(
            listed.contains(&policy_name.to_string()),
            "case {policy_name:?}: a refused delete dropped it from the listing {listed:?}"
        );
        let (status, body) = get_policy(&router, "Carrier", policy_name).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: a refused delete removed a policy the group embeds, got \
             {status}: {body}"
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "PolicyDocument"),
            document,
            "case {policy_name:?}: a refused delete changed the stored document, got {body}"
        );
    }

    // A delete that DID remove something cannot remove it twice: the second one is the
    // same refusal as for a name that was never embedded.
    for (policy_name, _) in embedded {
        let (status, body) = delete_policy(&router, "Carrier", policy_name).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: the first delete must succeed, got {status}: {body}"
        );
        let (status, body) = delete_policy(&router, "Carrier", policy_name).await;
        assert_eq!(
            status, 404,
            "case {policy_name:?}: expected 404 for a repeated delete, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {policy_name:?}: expected the modelled awsQuery code on the repeat, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The group policy with name {policy_name} cannot be found."),
            "case {policy_name:?}: expected the repeat to name the group policy, got {:?}",
            element(&body, "Message")
        );
    }
}

/// RULE: an inline policy is reached only through the entity that embeds it, so a
/// delete addressed to one group never removes a policy of the same name embedded in
/// another group, nor one embedded in a user or a role of that name.
#[tokio::test]
async fn a_delete_reaches_only_the_named_groups_own_policy() {
    let router = router();
    let shared_name = "shared-policy";
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"sqs:*"}]}"#;

    for (group_name, path) in [("first", "/"), ("second", "/eng/"), ("third", "/eng/tools/")] {
        create_group(&router, group_name, path).await;
        put_policy(&router, group_name, shared_name, document).await;
    }

    // A user and a role of the same name as the first group embed the same policy name:
    // three different policies, reached through three different entity types.
    let (status, body) = call(&router, "CreateUser", &[("UserName", "first")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "first"),
            ("PolicyName", shared_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "PutUserPolicy must succeed, got {status}: {body}"
    );

    // Deleting the first group's copy leaves the other groups' and the user's alone.
    let (status, body) = delete_policy(&router, "first", shared_name).await;
    assert_eq!(
        status, 200,
        "the first group's own policy must be deletable, got {status}: {body}"
    );
    for group_name in ["second", "third"] {
        let (status, body) = get_policy(&router, group_name, shared_name).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: another group's policy of the same name must survive, got \
             {status}: {body}"
        );
    }
    let (status, body) = call(
        &router,
        "GetUserPolicy",
        &[("UserName", "first"), ("PolicyName", shared_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "a user's policy of the same name must survive, got {status}: {body}"
    );
}

/// RULE: a deleted policy's share of the group's bounded aggregate inline policy size
/// is free again, and the deleted name is free again too — so a put may re-use both.
///
/// The rows are the sizes a freed document is re-embedded at: the whole bound, half of
/// it, and the smallest document the grammar admits.
#[tokio::test]
async fn a_delete_frees_the_groups_policy_size_and_name() {
    for (case, size) in [
        ("the whole bound", MAX_GROUP_POLICY_CHARACTERS),
        ("half the bound", MAX_GROUP_POLICY_CHARACTERS / 2),
        ("the smallest document", SHORTEST_DOCUMENT_CHARACTERS),
    ] {
        let router = router();
        create_group(&router, "Sized", "/").await;

        // The group is filled to its bound, so nothing more fits.
        let filling = document_of(MAX_GROUP_POLICY_CHARACTERS);
        put_policy(&router, "Sized", "Filler", &filling).await;
        let (status, body) = call(
            &router,
            "PutGroupPolicy",
            &[
                ("GroupName", "Sized"),
                ("PolicyName", "Another"),
                ("PolicyDocument", &document_of(SHORTEST_DOCUMENT_CHARACTERS)),
            ],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a full group must refuse one more policy, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // Deleting the filler gives the whole quota back, so a document of any size
        // inside the bound is accepted again — under the SAME name, which is free too.
        let (status, body) = delete_policy(&router, "Sized", "Filler").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the filler must be deletable, got {status}: {body}"
        );
        let document = document_of(size);
        put_policy(&router, "Sized", "Filler", &document).await;
        let (status, body) = get_policy(&router, "Sized", "Filler").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-embedded policy must read back, got {status}: {body}"
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "PolicyDocument"),
            document,
            "case {case:?}: the re-embedded document must be the stored one, got {body}"
        );
    }
}
