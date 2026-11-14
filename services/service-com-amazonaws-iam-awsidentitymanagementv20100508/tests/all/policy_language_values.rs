// SPDX-License-Identifier: CC0-1.0

//! The VALUES an IAM policy document declares, held to the ones the IAM policy language
//! defines — end to end over the awsQuery wire.
//!
//! The rule under test: **every operation that stores an identity-based policy document
//! reads the values the document declares against the IAM JSON policy language before it
//! stores anything** — the `Version` it declares has to be one the language publishes, a
//! statement's `Effect` has to be one of the two the language defines (compared literally,
//! so `allow` is not `Allow`), and a statement's `Resource` and `NotResource` name
//! resources, so every value they carry is an ARN or the wildcard `*`. A document that
//! declares a value outside one of those is refused with the modelled
//! `MalformedPolicyDocumentException` (awsQuery code `MalformedPolicyDocument`, HTTP 400)
//! and AWS's sentence for the clause it broke, and NOTHING is stored.
//!
//! The sentence is the clause's, not the operation's — with ONE exception recorded on both
//! sides: an `Effect` the language does not define is worded `The policy failed legacy
//! parsing` by the INLINE-policy validator and `Syntax errors in policy.` by the
//! MANAGED-policy one, so each row below carries the two sentences its clause is refused
//! with.
//!
//! SOURCE for the published values: the "IAM JSON policy elements" reference — `Version`
//! is `2012-10-17` or the legacy `2008-10-17`, `Effect` is `Allow` or `Deny`, and a
//! resource is named by its ARN, with `*` admitted for "all resources". The rows below are
//! written against that reference rather than against any one request: they differ along
//! which clause the document breaks, which value it declares instead, the form the element
//! takes (a string or a list), which spelling of the resource element names the value, and
//! which statement of a two-statement document carries it.
//!
//! Every call goes over the wire IAM speaks, through the `MultiServiceRouter` harness
//! `tests/all/user_lifecycle.rs` owns, so what is asserted is the HTTP behaviour of a
//! PERSISTED (or refused) write rather than a handler called directly.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The awsQuery error code the model fixes for `MalformedPolicyDocumentException`
/// (`awsQueryError` on the error shape), which is what a client branches on.
const MALFORMED: &str = "MalformedPolicyDocument";

/// The awsQuery error code a read of an entity that does not exist answers with — what a
/// refused write must leave behind.
const NO_SUCH_ENTITY: &str = "NoSuchEntity";

/// A trust policy with a single statement: every role this suite creates needs one, and the
/// trust policy is not what is under test here.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The role, user and group the identity policies below are written on.
const ROLE_NAME: &str = "grant-target-role";
const USER_NAME: &str = "grant-target-user";
const GROUP_NAME: &str = "grant-target-group";

/// One document the policy language does not define a value of, and AWS's sentence for the
/// clause it breaks — as the inline-policy validator words it, and as the managed-policy
/// validator does.
struct RefusedCase {
    case: &'static str,
    document: &'static str,
    message: &'static str,
    /// The sentence a MANAGED-policy write is refused the same document with, where it
    /// differs from [`Self::message`].
    managed_message: &'static str,
}

/// The sentence the MANAGED-policy validator words an `Effect` the language does not define
/// with — recorded for a `CreatePolicy` whose statement declares one.
const MANAGED_UNREADABLE_EFFECT: &str = "Syntax errors in policy.";

/// The sentence the INLINE-policy validator words that same clause with — recorded for a
/// `PutRolePolicy` whose statement declares an effect spelled in another case.
const INLINE_UNREADABLE_EFFECT: &str = "The policy failed legacy parsing";

/// Every clause of the language a document can declare a value outside, each with the
/// sentence that clause is refused with.
const REFUSED: &[RefusedCase] = &[
    RefusedCase {
        case: "a policy version that does not exist",
        document: r#"{"Version":"2012-10-18","Statement":[{"Sid":"InvokeApi","Effect":"Allow","Action":"execute-api:Invoke","Resource":"arn:aws:execute-api:us-west-2:000000000000:a1b2c3d4e5/beta/GET/*"}]}"#,
        message: "Syntax errors in policy.",
        managed_message: "Syntax errors in policy.",
    },
    RefusedCase {
        case: "a policy version that is not a string",
        document: r#"{"Version":20121017,"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        message: "Syntax errors in policy.",
        managed_message: "Syntax errors in policy.",
    },
    RefusedCase {
        case: "an effect written in another case",
        document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"InvokeApi","Effect":"allow","Action":"execute-api:Invoke","Resource":"arn:aws:execute-api:us-west-2:000000000000:a1b2c3d4e5/beta/GET/*"}]}"#,
        message: INLINE_UNREADABLE_EFFECT,
        managed_message: MANAGED_UNREADABLE_EFFECT,
    },
    RefusedCase {
        case: "an effect the language does not define",
        document: r#"{"Statement":[{"Effect":"Permit","Action":"s3:GetObject","Resource":"*"}]}"#,
        message: INLINE_UNREADABLE_EFFECT,
        managed_message: MANAGED_UNREADABLE_EFFECT,
    },
    RefusedCase {
        case: "an effect that is not a string",
        document: r#"{"Statement":[{"Effect":true,"Action":"s3:GetObject","Resource":"*"}]}"#,
        message: INLINE_UNREADABLE_EFFECT,
        managed_message: MANAGED_UNREADABLE_EFFECT,
    },
    RefusedCase {
        case: "the second statement's effect",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"},{"Effect":"DENY","Action":"s3:PutObject","Resource":"*"}]}"#,
        message: INLINE_UNREADABLE_EFFECT,
        managed_message: MANAGED_UNREADABLE_EFFECT,
    },
    RefusedCase {
        case: "a resource that is no ARN",
        document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"InvokeApi","Effect":"Allow","Action":"execute-api:Invoke","Resource":"data-agreements/*"}]}"#,
        message: "Resource data-agreements/* must be in ARN format or \"*\".",
        managed_message: "Resource data-agreements/* must be in ARN format or \"*\".",
    },
    RefusedCase {
        case: "the second value of a resource list",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":["arn:aws:s3:::reports/*","reports/*"]}]}"#,
        message: "Resource reports/* must be in ARN format or \"*\".",
        managed_message: "Resource reports/* must be in ARN format or \"*\".",
    },
    RefusedCase {
        case: "a NotResource that is no ARN",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","NotResource":"public"}]}"#,
        message: "Resource public must be in ARN format or \"*\".",
        managed_message: "Resource public must be in ARN format or \"*\".",
    },
    RefusedCase {
        case: "a single statement object naming no resource ARN",
        document: r#"{"Statement":{"Effect":"Allow","Action":"s3:GetObject","Resource":"my-bucket"}}"#,
        message: "Resource my-bucket must be in ARN format or \"*\".",
        managed_message: "Resource my-bucket must be in ARN format or \"*\".",
    },
    RefusedCase {
        case: "the second statement's resource",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"},{"Effect":"Deny","Action":"s3:PutObject","Resource":"writes"}]}"#,
        message: "Resource writes must be in ARN format or \"*\".",
        managed_message: "Resource writes must be in ARN format or \"*\".",
    },
];

/// Documents whose every declared value IS one the language defines, in each form it admits
/// — what keeps the refusals above from being a blanket refusal.
const ACCEPTED: &[(&str, &str)] = &[
    (
        "an ARN resource under the current version",
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"InvokeApi","Effect":"Allow","Action":"execute-api:Invoke","Resource":"arn:aws:execute-api:us-west-2:000000000000:a1b2c3d4e5/beta/GET/*"}]}"#,
    ),
    (
        "the wildcard in place of an ARN",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"s3:DeleteObject","Resource":"*"}]}"#,
    ),
    (
        "the legacy policy version",
        r#"{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#,
    ),
    (
        "no version at all",
        r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":["arn:aws:s3:::a/*","*"]}]}"#,
    ),
    (
        "a NotResource naming an ARN",
        r#"{"Statement":[{"Effect":"Deny","Action":"s3:*","NotResource":"arn:aws:s3:::public/*"}]}"#,
    ),
    (
        "two statements the language defines every value of",
        r#"{"Version":"2012-10-17","Id":"two","Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"},{"Effect":"Deny","Action":"logs:DeleteLogGroup","Resource":"arn:aws:logs:us-west-2:000000000000:log-group:audit"}]}"#,
    ),
];

/// A managed policy ARN is `arn:aws:iam::<account>:policy<path><name>` — IAM is global, so
/// the region segment is EMPTY.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy/{policy_name}", account_id())
}

/// Create the role, user and group the inline writes below are addressed to.
async fn with_entities() -> MultiServiceRouter {
    let router = router();
    for (action, params) in [
        (
            "CreateRole",
            vec![
                ("RoleName", ROLE_NAME),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        ),
        ("CreateUser", vec![("UserName", USER_NAME)]),
        ("CreateGroup", vec![("GroupName", GROUP_NAME)]),
    ] {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "the fixture {action} must succeed, got {status}: {body}"
        );
    }
    router
}

/// The three inline-policy write paths, each with the read that reports what it embedded:
/// `(the write, the read, the entity member, the entity's name)`.
const INLINE_PATHS: &[(&str, &str, &str, &str)] = &[
    ("PutRolePolicy", "GetRolePolicy", "RoleName", ROLE_NAME),
    ("PutUserPolicy", "GetUserPolicy", "UserName", USER_NAME),
    ("PutGroupPolicy", "GetGroupPolicy", "GroupName", GROUP_NAME),
];

/// Every inline-policy write refuses a document declaring a value the language does not
/// define, words the refusal with that clause's sentence, and embeds nothing.
#[tokio::test]
async fn an_inline_policy_write_refuses_a_value_the_language_does_not_define() {
    let router = with_entities().await;
    for (write, read, member, entity) in INLINE_PATHS {
        for (index, row) in REFUSED.iter().enumerate() {
            let policy_name = format!("refused-{index:02}");
            let (status, body) = call(
                &router,
                write,
                &[
                    (member, entity),
                    ("PolicyName", policy_name.as_str()),
                    ("PolicyDocument", row.document),
                ],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {:?} on {write}: expected HTTP 400 for {:?}, got {status}: {body}",
                row.case, row.document
            );
            let code = error_code(&body);
            assert_eq!(
                code, MALFORMED,
                "case {:?} on {write}: expected the modelled awsQuery code {MALFORMED}, got {code:?}: {body}",
                row.case
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, row.message,
                "case {:?} on {write}: expected the refusal worded {:?}, got {message:?}",
                row.case, row.message
            );
            // Nothing a later caller can read was embedded by the refused write.
            let (read_status, read_body) = call(
                &router,
                read,
                &[(member, entity), ("PolicyName", policy_name.as_str())],
            )
            .await;
            assert_eq!(
                read_status, 404,
                "case {:?} on {write}: a refused write must embed nothing, got {read_status}: {read_body}",
                row.case
            );
            let read_code = error_code(&read_body);
            assert_eq!(
                read_code, NO_SUCH_ENTITY,
                "case {:?} on {write}: expected {NO_SUCH_ENTITY} from {read}, got {read_code:?}",
                row.case
            );
        }
    }
}

/// A document whose every declared value the language defines is embedded by each inline
/// write and is then reported by the read and by the listing of that entity's policies.
#[tokio::test]
async fn an_inline_policy_write_stores_a_document_the_language_defines() {
    let router = with_entities().await;
    for (write, read, member, entity) in INLINE_PATHS {
        let listing = match *write {
            "PutRolePolicy" => "ListRolePolicies",
            "PutUserPolicy" => "ListUserPolicies",
            _ => "ListGroupPolicies",
        };
        for (index, (case, document)) in ACCEPTED.iter().enumerate() {
            let policy_name = format!("accepted-{index:02}");
            let (status, body) = call(
                &router,
                write,
                &[
                    (member, entity),
                    ("PolicyName", policy_name.as_str()),
                    ("PolicyDocument", document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} on {write}: expected HTTP 200 for {document:?}, got {status}: {body}"
            );
            let (read_status, read_body) = call(
                &router,
                read,
                &[(member, entity), ("PolicyName", policy_name.as_str())],
            )
            .await;
            assert_eq!(
                read_status, 200,
                "case {case:?} on {write}: the embedded policy must be readable, got {read_status}: {read_body}"
            );
            let (list_status, list_body) = call(&router, listing, &[(member, entity)]).await;
            assert_eq!(
                list_status, 200,
                "case {case:?} on {write}: {listing} must succeed, got {list_status}: {list_body}"
            );
            let names = elements(&list_body, "member");
            assert!(
                names.contains(&policy_name),
                "case {case:?} on {write}: {listing} must name {policy_name:?}, got {names:?}"
            );
        }
    }
}

/// A managed policy's document is an identity-based policy, so both operations that write
/// one hold it to the same clauses: the create is refused and no policy exists, and a new
/// version is refused and the policy keeps the version it had.
#[tokio::test]
async fn a_managed_policy_write_is_held_to_the_same_clauses() {
    let router = router();
    let complete = ACCEPTED[0].1;
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[("PolicyName", "versioned"), ("PolicyDocument", complete)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreatePolicy must succeed, got {status}: {body}"
    );
    let arn = element(&body, "Arn");

    for (index, row) in REFUSED.iter().enumerate() {
        let policy_name = format!("refused-{index:02}");
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", policy_name.as_str()),
                ("PolicyDocument", row.document),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?} on CreatePolicy: expected HTTP 400 for {:?}, got {status}: {body}",
            row.case, row.document
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, row.managed_message,
            "case {:?} on CreatePolicy: expected the refusal worded {:?}, got {message:?}",
            row.case, row.managed_message
        );
        let (get_status, got) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", policy_arn(&policy_name).as_str())],
        )
        .await;
        assert_eq!(
            get_status, 404,
            "case {:?}: a refused create must persist nothing, got {get_status}: {got}",
            row.case
        );

        let (version_status, version_body) = call(
            &router,
            "CreatePolicyVersion",
            &[("PolicyArn", arn.as_str()), ("PolicyDocument", row.document)],
        )
        .await;
        assert_eq!(
            version_status, 400,
            "case {:?} on CreatePolicyVersion: expected HTTP 400, got {version_status}: {version_body}"
        , row.case);
        let version_message = element(&version_body, "Message");
        assert_eq!(
            version_message, row.managed_message,
            "case {:?} on CreatePolicyVersion: expected the refusal worded {:?}, got {version_message:?}",
            row.case, row.managed_message
        );
        let (second_status, second_body) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", arn.as_str()), ("VersionId", "v2")],
        )
        .await;
        assert_eq!(
            second_status, 404,
            "case {:?}: a refused version must not be stored, got {second_status}: {second_body}",
            row.case
        );
    }

    // The same policy takes a second version whose every declared value the language
    // defines, which is what shows the refusals above are the clauses and not the write.
    let (status, body) = call(
        &router,
        "CreatePolicyVersion",
        &[("PolicyArn", arn.as_str()), ("PolicyDocument", ACCEPTED[2].1)],
    )
    .await;
    assert_eq!(
        status, 200,
        "a document the language defines must be accepted as a new version, got {status}: {body}"
    );
}
