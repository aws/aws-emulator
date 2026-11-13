// SPDX-License-Identifier: CC0-1.0

//! Inline group policies, end to end over the awsQuery wire.
//!
//! `PutGroupPolicy` embeds a policy document in a group that is PERSISTED, and
//! `GetGroupPolicy` — its reporting half — reads it back, so every assertion here is
//! about what a client on the socket sees after a round trip through the store, not
//! about a handler called directly. The awsQuery harness (router, call, element,
//! error_code) is the one `tests/all/user_lifecycle.rs` owns and every suite in this
//! crate shares.
//!
//! Each test is a TABLE whose rows differ along what the rule being asserted
//! generalises over: the group's path, the case and alphabet of the group and policy
//! names, the form and size of the document, and the number of policies embedded. No
//! single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, error_code, router};

/// The aggregate size, in characters, of all inline policies one group may hold.
/// AWS's "IAM and STS quotas" page: "Group policy size cannot exceed 5,120
/// characters".
const MAX_GROUP_POLICY_CHARACTERS: usize = 5_120;

/// The shortest document [`document_of`] can build, which is also the smallest
/// document the top-level IAM policy grammar admits in that form.
const SHORTEST_DOCUMENT_CHARACTERS: usize = 26;

/// Create a group, failing the test when the create this suite depends on does not
/// succeed.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateGroup must succeed, got {status}: {body}"
    );
}

/// Embed `document` in `group_name` under `policy_name`.
async fn put_policy(
    router: &MultiServiceRouter,
    group_name: &str,
    policy_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "PutGroupPolicy",
        &[
            ("GroupName", group_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
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

/// A policy document of exactly `characters` characters: the shortest document the
/// grammar admits, padded inside a `Sid` string, so a size rule can be exercised at
/// any point of its range.
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

/// One embed: the group it goes into and the name and document it carries.
struct Case {
    case: &'static str,
    group_name: &'static str,
    path: &'static str,
    policy_name: &'static str,
    document: &'static str,
}

/// Rows spanning the root path, a one-segment and a nested path; group and policy
/// names drawn from the whole `^[\w+=,.@-]+$` alphabet
/// (`smithy_query(query="operation", operation="PutGroupPolicy")`) in mixed case;
/// and documents spanning a statement list, a bare statement object, a document
/// with an `Id`, one with no `Version`, one indented with the tab, line feed and
/// carriage return `policyDocumentType`'s pattern admits, one carrying a Latin-1
/// Supplement character (the top of that pattern's range), and the smallest
/// document the grammar admits.
const CASES: &[Case] = &[
    Case {
        case: "a statement list at the account root",
        group_name: "Admins",
        path: "/",
        policy_name: "AllowRead",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    },
    Case {
        case: "a bare statement object under a one-segment path",
        group_name: "billing_2",
        path: "/eng/",
        policy_name: "deny_all",
        document: r#"{"Statement":{"Effect":"Deny","Action":"*","Resource":"*"}}"#,
    },
    Case {
        case: "a document carrying an Id, under a nested path",
        group_name: "core.3",
        path: "/eng/tools/",
        policy_name: "Policy.With.Id",
        document: r#"{"Version":"2012-10-17","Id":"one","Statement":[{"Effect":"Allow","Action":"ec2:*"}]}"#,
    },
    Case {
        case: "a document with no Version",
        group_name: "data+4",
        path: "/sales/",
        policy_name: "no+version",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"fis:StartExperiment"}]}"#,
    },
    Case {
        case: "a document indented with tab, line feed and carriage return",
        group_name: "eng=5",
        path: "/",
        policy_name: "indented=5",
        document: "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t{\"Effect\": \"Allow\", \"Action\": \"iam:PassRole\"}\r\n\t]\r\n}",
    },
    Case {
        case: "a document carrying a Latin-1 Supplement character",
        group_name: "finance@6",
        path: "/eng/",
        policy_name: "latin1@6",
        document: r#"{"Statement":[{"Sid":"caf\u00e9 \u00ff","Effect":"Allow","Action":"s3:*"}]}"#,
    },
    Case {
        case: "the smallest document the grammar admits",
        group_name: "growth-7",
        path: "/sales/west/",
        policy_name: "minimal-7",
        document: r#"{"Statement":[]}"#,
    },
];

/// An embedded policy is reported back by `GetGroupPolicy` with the document stored
/// VERBATIM, under the group and policy names the account holds.
#[tokio::test]
async fn an_embedded_group_policy_reads_back_verbatim() {
    let router = router();
    for Case {
        case,
        group_name,
        path,
        policy_name,
        document,
    } in CASES
    {
        create_group(&router, group_name, path).await;

        let (status, body) = put_policy(&router, group_name, policy_name, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutGroupPolicy must succeed, got {status}: {body}"
        );

        let (status, body) = get_policy(&router, group_name, policy_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetGroupPolicy must report the embedded policy, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "GroupName"),
            *group_name,
            "case {case:?}: expected the stored group name, got {body}"
        );
        assert_eq!(
            element(&body, "PolicyName"),
            *policy_name,
            "case {case:?}: expected the stored policy name, got {body}"
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "PolicyDocument"),
            *document,
            "case {case:?}: the document must read back exactly as it was embedded, got {body}"
        );
    }
}

/// IAM entity and policy names are unique within the account and "are not
/// distinguished by case", so a put and a read reach the same policy however either
/// request spells the names — and the response reports the group as it was created
/// and the policy as the last put spelled it.
#[tokio::test]
async fn names_are_matched_without_regard_to_case() {
    let spellings = [
        ("as created", "CaseGroup", "Embedded"),
        ("all lower case", "casegroup", "embedded"),
        ("all upper case", "CASEGROUP", "EMBEDDED"),
        ("inverted case", "cASEGROUP", "eMBEDDED"),
    ];

    let router = router();
    create_group(&router, "CaseGroup", "/eng/").await;
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"sts:AssumeRole"}]}"#;
    let (status, body) = put_policy(&router, "CaseGroup", "Embedded", document).await;
    assert_eq!(
        status, 200,
        "PutGroupPolicy must succeed, got {status}: {body}"
    );

    for (case, group_spelling, policy_spelling) in spellings {
        let (status, body) = get_policy(&router, group_spelling, policy_spelling).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read must reach the policy whatever the case, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "GroupName"),
            "CaseGroup",
            "case {case:?}: expected the group name the account holds, got {body}"
        );
        assert_eq!(
            element(&body, "PolicyName"),
            "Embedded",
            "case {case:?}: expected the policy name the account holds, got {body}"
        );
    }

    // A put of the same policy name in another case REPLACES that policy rather
    // than adding a second one, and the name it is reported under is the one the
    // latest put chose.
    let replacement = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"*"}]}"#;
    let (status, body) = put_policy(&router, "CASEGROUP", "EMBEDDED", replacement).await;
    assert_eq!(status, 200, "a repeat put must succeed, got {status}: {body}");
    let (status, body) = get_policy(&router, "casegroup", "embedded").await;
    assert_eq!(
        status, 200,
        "the replaced policy must read back, got {status}: {body}"
    );
    assert_eq!(
        crate::user_lifecycle::document_element(&body, "PolicyDocument"),
        replacement,
        "a repeat put must replace the stored document, got {body}"
    );
    assert_eq!(
        element(&body, "PolicyName"),
        "EMBEDDED",
        "the policy must be reported under the name the latest put chose, got {body}"
    );
}

/// Each (group, policy name) pair names its own document: policies embedded in one
/// group are invisible on another, and two names in one group do not collide. The
/// same name embedded in a USER is a different policy too, because an inline policy
/// is reached only through the entity that embeds it.
#[tokio::test]
async fn embedded_policies_are_per_group_and_per_name() {
    // Rows: how many policies the group holds, and which other group must NOT see
    // them.
    let groups = [("first", "/", 1_usize), ("second", "/eng/", 3)];

    let router = router();
    for (group_name, path, _) in groups {
        create_group(&router, group_name, path).await;
    }
    for (group_name, _, count) in groups {
        for index in 0..count {
            let policy_name = format!("{group_name}-policy-{index}");
            let document = format!(
                r#"{{"Statement":[{{"Sid":"{group_name}{index}","Effect":"Allow","Action":"s3:*"}}]}}"#
            );
            let (status, body) = put_policy(&router, group_name, &policy_name, &document).await;
            assert_eq!(
                status, 200,
                "case {policy_name:?}: PutGroupPolicy must succeed, got {status}: {body}"
            );
        }
    }

    for (group_name, _, count) in groups {
        for index in 0..count {
            let policy_name = format!("{group_name}-policy-{index}");
            let expected = format!(
                r#"{{"Statement":[{{"Sid":"{group_name}{index}","Effect":"Allow","Action":"s3:*"}}]}}"#
            );
            let (status, body) = get_policy(&router, group_name, &policy_name).await;
            assert_eq!(
                status, 200,
                "case {policy_name:?}: the group's own policy must read back, got {status}: {body}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&body, "PolicyDocument"),
                expected,
                "case {policy_name:?}: expected this policy's own document, got {body}"
            );

            // The other group in the table does not hold it.
            for (other, _, _) in groups {
                if other == group_name {
                    continue;
                }
                let (status, body) = get_policy(&router, other, &policy_name).await;
                assert_eq!(
                    status, 404,
                    "case {policy_name:?}: group {other:?} must not hold it, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "NoSuchEntity",
                    "case {policy_name:?}: expected the modelled awsQuery code, got {body}"
                );
            }
        }
    }

    // A user of the same name embedding the same policy name is a different
    // policy: the group's document is untouched by it.
    let (status, body) = call(&router, "CreateUser", &[("UserName", "first")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let user_document = r#"{"Statement":[{"Effect":"Deny","Action":"iam:*"}]}"#;
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "first"),
            ("PolicyName", "first-policy-0"),
            ("PolicyDocument", user_document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "PutUserPolicy must succeed, got {status}: {body}"
    );
    let (status, body) = get_policy(&router, "first", "first-policy-0").await;
    assert_eq!(
        status, 200,
        "the group's policy must still read back, got {status}: {body}"
    );
    assert_eq!(
        crate::user_lifecycle::document_element(&body, "PolicyDocument"),
        r#"{"Statement":[{"Sid":"first0","Effect":"Allow","Action":"s3:*"}]}"#,
        "an embed into a user of the same name must not touch the group's policy, got {body}"
    );
}

/// An embed into a group the account does not hold, and a read of a policy that was
/// never embedded, are both refused with the modelled `NoSuchEntityException` — and
/// the refused embed persists nothing.
#[tokio::test]
async fn an_absent_group_or_policy_is_not_found() {
    // Rows: a group that was never created, one whose name differs from an existing
    // group only past the first characters, and a name spanning the alphabet.
    let missing_groups = [
        ("never created", "ghost"),
        ("a near miss on an existing name", "presentx"),
        ("a name spanning the alphabet", "n0+=,.@-"),
    ];

    let router = router();
    create_group(&router, "present", "/").await;
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"s3:*"}]}"#;

    for (case, group_name) in missing_groups {
        let (status, body) = put_policy(&router, group_name, "Embedded", document).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an embed into an absent group must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        let (status, body) = get_policy(&router, group_name, "Embedded").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused embed must persist nothing, got {status}: {body}"
        );
    }

    for (case, policy_name) in [
        ("a policy nobody embedded", "Absent"),
        ("a policy name spanning the alphabet", "a_b+c=d,e.f@g-h"),
    ] {
        let (status, body) = get_policy(&router, "present", policy_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unembedded policy must not be found, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }
}

/// A document that is not an IAM policy document — not JSON, not an object, or an
/// object carrying no `Statement` — is refused with the modelled
/// `MalformedPolicyDocumentException` and embeds nothing.
#[tokio::test]
async fn a_malformed_policy_document_is_refused() {
    // The same table the user and role inline-policy suites assert, because the
    // rule is the one shared definition (`state::is_policy_document`) every
    // operation taking a policy document uses.
    let refused = [
        ("not JSON at all", "not a policy"),
        ("a bare string", r#""Statement""#),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#),
        ("a JSON number", "17"),
        ("a JSON null", "null"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        (
            "an object carrying every element but Statement",
            r#"{"Version":"2012-10-17","Id":"one"}"#,
        ),
        ("an empty object", "{}"),
        ("a truncated object", r#"{"Statement":[{"Effect":"Allow"}"#),
        ("a lower-case statement key", r#"{"statement":[]}"#),
    ];

    let router = router();
    create_group(&router, "MalformedTarget", "/eng/").await;
    for (index, (case, document)) in refused.iter().enumerate() {
        let policy_name = format!("refused-{index}");
        let (status, body) = put_policy(&router, "MalformedTarget", &policy_name, document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a malformed document, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "MalformedPolicyDocument",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        let (status, body) = get_policy(&router, "MalformedTarget", &policy_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused embed must persist nothing, got {status}: {body}"
        );
    }
}

/// The aggregate size of the inline policies embedded in one group is bounded, the
/// bound counts every policy the group holds, and a put that replaces an existing
/// name is measured without the document it replaces.
#[tokio::test]
async fn the_aggregate_group_policy_size_is_bounded() {
    let router = router();

    // The published bound itself, one character under it, and one past it, on a
    // fresh group each.
    for (case, size, expected_status) in [
        (
            "one character under the bound",
            MAX_GROUP_POLICY_CHARACTERS - 1,
            200,
        ),
        ("exactly the bound", MAX_GROUP_POLICY_CHARACTERS, 200),
        (
            "one character past the bound",
            MAX_GROUP_POLICY_CHARACTERS + 1,
            409,
        ),
    ] {
        let group_name = format!("bound{size}");
        create_group(&router, &group_name, "/").await;
        let document = document_of(size);
        let (status, body) = put_policy(&router, &group_name, "Sized", &document).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a {size}-character document must answer {expected_status}, \
             got {status}: {body}"
        );
        let (read_status, read_body) = get_policy(&router, &group_name, "Sized").await;
        if expected_status == 200 {
            assert_eq!(
                crate::user_lifecycle::document_element(&read_body, "PolicyDocument"),
                document,
                "case {case:?}: the accepted document must read back, got {read_body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                read_status, 404,
                "case {case:?}: a refused embed must persist nothing, got {read_status}: \
                 {read_body}"
            );
        }
    }

    // The bound is on the AGGREGATE: several policies that together reach it are
    // accepted, and the next one of any size is refused.
    create_group(&router, "Aggregate", "/eng/").await;
    let halves = [
        ("the first half", MAX_GROUP_POLICY_CHARACTERS / 2),
        (
            "the second half",
            MAX_GROUP_POLICY_CHARACTERS - MAX_GROUP_POLICY_CHARACTERS / 2,
        ),
    ];
    for (index, (case, size)) in halves.iter().enumerate() {
        let policy_name = format!("half-{index}");
        let (status, body) =
            put_policy(&router, "Aggregate", &policy_name, &document_of(*size)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: policies summing to the bound must be accepted, got {status}: {body}"
        );
    }
    let (status, body) = put_policy(
        &router,
        "Aggregate",
        "one-too-many",
        &document_of(SHORTEST_DOCUMENT_CHARACTERS),
    )
    .await;
    assert_eq!(
        status, 409,
        "a policy past the aggregate bound must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "expected the modelled awsQuery code, got {body}"
    );

    // Replacing one of those policies with a document of its own size is still
    // inside the bound, because the document it replaces does not count twice.
    let (status, body) = put_policy(
        &router,
        "Aggregate",
        "half-0",
        &document_of(MAX_GROUP_POLICY_CHARACTERS / 2),
    )
    .await;
    assert_eq!(
        status, 200,
        "a replacement must be measured without the document it replaces, got {status}: {body}"
    );

    // A group holding nothing but one policy may grow that policy to the whole
    // bound.
    create_group(&router, "Grow", "/").await;
    for (case, size) in [
        ("the first embed", SHORTEST_DOCUMENT_CHARACTERS),
        ("grown to the bound", MAX_GROUP_POLICY_CHARACTERS),
    ] {
        let document = document_of(size);
        let (status, body) = put_policy(&router, "Grow", "Only", &document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the put to be accepted, got {status}: {body}"
        );
        let (status, body) = get_policy(&router, "Grow", "Only").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the policy must read back, got {status}: {body}"
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "PolicyDocument"),
            document,
            "case {case:?}: the latest document must be the stored one, got {body}"
        );
    }
}
