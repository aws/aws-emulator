// SPDX-License-Identifier: CC0-1.0

//! Inline role policies, end to end over the awsQuery wire.
//!
//! `PutRolePolicy` embeds a policy document in a role that is PERSISTED, and
//! `GetRolePolicy` — its reporting half — reads it back, so every assertion here is
//! about what a client on the socket sees after a round trip through the store, not
//! about a handler called directly. The awsQuery harness (router, call, element,
//! error_code) is the one `tests/all/user_lifecycle.rs` owns and every suite in this
//! crate shares.
//!
//! Each test is a TABLE whose rows differ along what the rule being asserted
//! generalises over: the role's path, the case and alphabet of the role and policy
//! names, the form and size of the document, the number of policies embedded, and
//! the service a role is linked to. No single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, error_code, router};

/// A trust policy with a single statement — every role this suite creates needs
/// one, and the trust policy is not what is under test here.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The aggregate size, in characters, of all inline policies one role may hold.
/// AWS's "IAM and STS quotas" page: "Role policy size cannot exceed 10,240
/// characters".
const MAX_ROLE_POLICY_CHARACTERS: usize = 10_240;

/// The shortest document [`document_of`] can build, which is also the smallest
/// document the top-level IAM policy grammar admits in that form.
const SHORTEST_DOCUMENT_CHARACTERS: usize = 26;

/// Create a role, failing the test when the create this suite depends on does not
/// succeed.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// Embed `document` in `role_name` under `policy_name`.
async fn put_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await
}

/// Read one embedded policy back.
async fn get_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
) -> (u16, String) {
    call(
        router,
        "GetRolePolicy",
        &[("RoleName", role_name), ("PolicyName", policy_name)],
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

/// One embed: the role it goes into and the name and document it carries.
struct Case {
    case: &'static str,
    role_name: &'static str,
    path: &'static str,
    policy_name: &'static str,
    document: &'static str,
}

/// Rows spanning the root path, a one-segment and a nested path; role and policy
/// names drawn from the whole `^[\w+=,.@-]+$` alphabet
/// (`smithy_query(query="operation", operation="PutRolePolicy")`) in mixed case;
/// and documents spanning a statement list, a bare statement object, a document
/// with an `Id`, one with no `Version`, one indented with the tab, line feed and
/// carriage return `policyDocumentType`'s pattern admits, one carrying a Latin-1
/// Supplement character (the top of that pattern's range), and the smallest
/// document the grammar admits.
const CASES: &[Case] = &[
    Case {
        case: "a statement list at the account root",
        role_name: "Alpha",
        path: "/",
        policy_name: "AllowRead",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    },
    Case {
        case: "a bare statement object under a one-segment path",
        role_name: "beta_2",
        path: "/eng/",
        policy_name: "deny_all",
        document: r#"{"Statement":{"Effect":"Deny","Action":"*","Resource":"*"}}"#,
    },
    Case {
        case: "a document carrying an Id, under a nested path",
        role_name: "gamma.3",
        path: "/eng/tools/",
        policy_name: "Policy.With.Id",
        document: r#"{"Version":"2012-10-17","Id":"one","Statement":[{"Effect":"Allow","Action":"ec2:*"}]}"#,
    },
    Case {
        case: "a document with no Version",
        role_name: "delta+4",
        path: "/sales/",
        policy_name: "no+version",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"fis:StartExperiment"}]}"#,
    },
    Case {
        case: "a document indented with tab, line feed and carriage return",
        role_name: "epsilon=5",
        path: "/",
        policy_name: "indented=5",
        document: "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t{\"Effect\": \"Allow\", \"Action\": \"iam:PassRole\"}\r\n\t]\r\n}",
    },
    Case {
        case: "a document carrying a Latin-1 Supplement character",
        role_name: "zeta@6",
        path: "/eng/",
        policy_name: "latin1@6",
        document: r#"{"Statement":[{"Sid":"caf\u00e9 \u00ff","Effect":"Allow","Action":"s3:*"}]}"#,
    },
    Case {
        case: "the smallest document the grammar admits",
        role_name: "eta-7",
        path: "/sales/west/",
        policy_name: "minimal-7",
        document: r#"{"Statement":[]}"#,
    },
];

/// An embedded policy is reported back by `GetRolePolicy` with the document stored
/// VERBATIM, under the role and policy names the account holds.
#[tokio::test]
async fn an_embedded_role_policy_reads_back_verbatim() {
    let router = router();
    for Case {
        case,
        role_name,
        path,
        policy_name,
        document,
    } in CASES
    {
        create_role(&router, role_name, path).await;

        let (status, body) = put_policy(&router, role_name, policy_name, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutRolePolicy must succeed, got {status}: {body}"
        );

        let (status, body) = get_policy(&router, role_name, policy_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRolePolicy must report the embedded policy, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "RoleName"),
            *role_name,
            "case {case:?}: expected the stored role name, got {body}"
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
/// request spells the names — and the response reports the role as it was created
/// and the policy as the last put spelled it.
#[tokio::test]
async fn names_are_matched_without_regard_to_case() {
    let spellings = [
        ("as created", "CaseRole", "Embedded"),
        ("all lower case", "caserole", "embedded"),
        ("all upper case", "CASEROLE", "EMBEDDED"),
        ("inverted case", "cASEROLE", "eMBEDDED"),
    ];

    let router = router();
    create_role(&router, "CaseRole", "/eng/").await;
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"sts:AssumeRole"}]}"#;
    let (status, body) = put_policy(&router, "CaseRole", "Embedded", document).await;
    assert_eq!(
        status, 200,
        "PutRolePolicy must succeed, got {status}: {body}"
    );

    for (case, role_spelling, policy_spelling) in spellings {
        let (status, body) = get_policy(&router, role_spelling, policy_spelling).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read must reach the policy whatever the case, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "RoleName"),
            "CaseRole",
            "case {case:?}: expected the role name the account holds, got {body}"
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
    let (status, body) = put_policy(&router, "CASEROLE", "EMBEDDED", replacement).await;
    assert_eq!(
        status, 200,
        "a repeat put must succeed, got {status}: {body}"
    );
    let (status, body) = get_policy(&router, "caserole", "embedded").await;
    assert_eq!(status, 200, "the replaced policy must read back, got {status}: {body}");
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

/// Each (role, policy name) pair names its own document: policies embedded in one
/// role are invisible on another, and two names in one role do not collide.
#[tokio::test]
async fn embedded_policies_are_per_role_and_per_name() {
    // Rows: how many policies the role holds, and which other role must NOT see
    // them.
    let roles = [("first", "/", 1_usize), ("second", "/eng/", 3)];

    let router = router();
    for (role_name, path, _) in roles {
        create_role(&router, role_name, path).await;
    }
    for (role_name, _, count) in roles {
        for index in 0..count {
            let policy_name = format!("{role_name}-policy-{index}");
            let document = format!(
                r#"{{"Statement":[{{"Sid":"{role_name}{index}","Effect":"Allow","Action":"s3:*"}}]}}"#
            );
            let (status, body) = put_policy(&router, role_name, &policy_name, &document).await;
            assert_eq!(
                status, 200,
                "case {policy_name:?}: PutRolePolicy must succeed, got {status}: {body}"
            );
        }
    }

    for (role_name, _, count) in roles {
        for index in 0..count {
            let policy_name = format!("{role_name}-policy-{index}");
            let expected = format!(
                r#"{{"Statement":[{{"Sid":"{role_name}{index}","Effect":"Allow","Action":"s3:*"}}]}}"#
            );
            let (status, body) = get_policy(&router, role_name, &policy_name).await;
            assert_eq!(
                status, 200,
                "case {policy_name:?}: the role's own policy must read back, got {status}: {body}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&body, "PolicyDocument"),
                expected,
                "case {policy_name:?}: expected this policy's own document, got {body}"
            );

            // The other role in the table does not hold it.
            for (other, _, _) in roles {
                if other == role_name {
                    continue;
                }
                let (status, body) = get_policy(&router, other, &policy_name).await;
                assert_eq!(
                    status, 404,
                    "case {policy_name:?}: role {other:?} must not hold it, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "NoSuchEntity",
                    "case {policy_name:?}: expected the modelled awsQuery code, got {body}"
                );
            }
        }
    }
}

/// An embed into a role the account does not hold, and a read of a policy that was
/// never embedded, are both refused with the modelled `NoSuchEntityException` — and
/// the refused embed persists nothing.
#[tokio::test]
async fn an_absent_role_or_policy_is_not_found() {
    // Rows: a role that was never created, one whose name differs from an existing
    // role only past the first characters, and an existing role with a policy name
    // nobody embedded.
    let missing_roles = [
        ("never created", "ghost"),
        ("a near miss on an existing name", "presentx"),
        ("a name spanning the alphabet", "n0+=,.@-"),
    ];

    let router = router();
    create_role(&router, "present", "/").await;
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"s3:*"}]}"#;

    for (case, role_name) in missing_roles {
        let (status, body) = put_policy(&router, role_name, "Embedded", document).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an embed into an absent role must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        let (status, body) = get_policy(&router, role_name, "Embedded").await;
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
    // The same table `CreateRole`'s trust-policy suite asserts, because the rule is
    // the one shared definition (`state::is_policy_document`) both operations use.
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
    create_role(&router, "MalformedTarget", "/eng/").await;
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

/// The aggregate size of the inline policies embedded in one role is bounded, the
/// bound counts every policy the role holds, and a put that replaces an existing
/// name is measured without the document it replaces.
#[tokio::test]
async fn the_aggregate_role_policy_size_is_bounded() {
    let router = router();

    // The published bound itself, and one character past it, on a fresh role each.
    for (case, size, expected_status) in [
        ("one character under the bound", MAX_ROLE_POLICY_CHARACTERS - 1, 200),
        ("exactly the bound", MAX_ROLE_POLICY_CHARACTERS, 200),
        ("one character past the bound", MAX_ROLE_POLICY_CHARACTERS + 1, 409),
    ] {
        let role_name = format!("bound{size}");
        create_role(&router, &role_name, "/").await;
        let document = document_of(size);
        let (status, body) = put_policy(&router, &role_name, "Sized", &document).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a {size}-character document must answer {expected_status}, \
             got {status}: {body}"
        );
        let (read_status, read_body) = get_policy(&router, &role_name, "Sized").await;
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
    create_role(&router, "Aggregate", "/eng/").await;
    let halves = [
        ("the first half", MAX_ROLE_POLICY_CHARACTERS / 2),
        (
            "the second half",
            MAX_ROLE_POLICY_CHARACTERS - MAX_ROLE_POLICY_CHARACTERS / 2,
        ),
    ];
    for (index, (case, size)) in halves.iter().enumerate() {
        let policy_name = format!("half-{index}");
        let (status, body) = put_policy(&router, "Aggregate", &policy_name, &document_of(*size)).await;
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
        &document_of(MAX_ROLE_POLICY_CHARACTERS / 2),
    )
    .await;
    assert_eq!(
        status, 200,
        "a replacement must be measured without the document it replaces, got {status}: {body}"
    );

    // A role holding nothing but one policy may grow that policy to the whole
    // bound.
    create_role(&router, "Grow", "/").await;
    for (case, size) in [
        ("the first embed", SHORTEST_DOCUMENT_CHARACTERS),
        ("grown to the bound", MAX_ROLE_POLICY_CHARACTERS),
    ] {
        let document = document_of(size);
        let (status, body) = put_policy(&router, "Grow", "Only", &document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the put to be accepted, got {status}: {body}"
        );
        let (status, body) = get_policy(&router, "Grow", "Only").await;
        assert_eq!(status, 200, "case {case:?}: the policy must read back, got {status}: {body}");
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "PolicyDocument"),
            document,
            "case {case:?}: the latest document must be the stored one, got {body}"
        );
    }
}

/// A service-linked role is a protected resource: only the service that depends on
/// it may change it, so embedding an inline policy in one is refused with the
/// modelled `UnmodifiableEntityException`, stated in the one sentence this service
/// words that protection with, and nothing is embedded.
#[tokio::test]
async fn a_service_linked_role_may_not_be_given_an_inline_policy() {
    // Rows spanning two- and three-label service principals and a role created with
    // a custom suffix, so the rule is read off the role's PATH rather than its name.
    let principals = [
        ("a two-label principal", "autoscaling.amazonaws.com", None),
        (
            "a three-label principal",
            "ops.apigateway.amazonaws.com",
            None,
        ),
        (
            "a principal with a custom suffix",
            "elasticbeanstalk.amazonaws.com",
            Some("debug"),
        ),
    ];

    let router = router();
    for (case, principal, custom_suffix) in principals {
        let mut params = vec![("AWSServiceName", principal)];
        if let Some(custom_suffix) = custom_suffix {
            params.push(("CustomSuffix", custom_suffix));
        }
        let (status, body) = call(&router, "CreateServiceLinkedRole", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateServiceLinkedRole must succeed, got {status}: {body}"
        );
        let role_name = element(&body, "RoleName");

        let document = r#"{"Statement":[{"Effect":"Allow","Action":"s3:*"}]}"#;
        let (status, body) = put_policy(&router, &role_name, "Embedded", document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an embed into a service-linked role must be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "UnmodifiableEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!(
                "Cannot perform the operation on the protected role '{role_name}' - this \
                 role is only modifiable by AWS"
            ),
            "case {case:?}: expected the service's one protected-role sentence, got \
             {message:?}"
        );

        let (status, body) = get_policy(&router, &role_name, "Embedded").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused embed must persist nothing, got {status}: {body}"
        );
    }
}
