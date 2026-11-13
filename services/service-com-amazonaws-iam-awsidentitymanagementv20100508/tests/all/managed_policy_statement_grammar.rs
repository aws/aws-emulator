// SPDX-License-Identifier: CC0-1.0

//! The GRAMMAR a managed policy's statements are held to, end to end over the awsQuery
//! wire.
//!
//! The rule under test: **a managed policy's document is an identity-based policy, so
//! every statement of it must name the actions it is about (`Action` or `NotAction`) and
//! the resources those actions apply to (`Resource` or `NotResource`)** — a statement
//! omitting one describes no permission IAM could enforce, so the write is refused with
//! the modelled `MalformedPolicyDocumentException` (awsQuery code
//! `MalformedPolicyDocument`, HTTP 400) and the fixed sentence IAM words that missing
//! element with, and nothing is stored. Both operations that write such a document —
//! `CreatePolicy` and `CreatePolicyVersion` — answer the same way.
//!
//! The rows differ along what the rule generalises over: which required element is
//! missing, which spelling of the other one the statement uses, the form `Statement`
//! takes (a list, a single object, several statements), the form the element's value
//! takes (a string, a list, an empty list, null) and the operation the document is sent
//! to. A document that names both is accepted and reads back verbatim, which is what
//! keeps the refusals above from being a blanket one.
//!
//! Everything goes over the wire IAM speaks, through the `MultiServiceRouter` harness
//! `tests/all/user_lifecycle.rs` owns, so what is under test is the HTTP behaviour of a
//! PERSISTED policy rather than a handler called directly.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// The awsQuery error code the model fixes for `MalformedPolicyDocumentException`
/// (`awsQueryError` on the error shape), which is what a client branches on.
const MALFORMED: &str = "MalformedPolicyDocument";

/// The sentence IAM words a statement that names no resources with.
const NO_RESOURCES: &str = "Policy statement must contain resources.";

/// The sentence IAM words a statement that names no actions with.
const NO_ACTIONS: &str = "Policy statement must contain actions.";

/// The sentence IAM words a value that is no policy document at all with — the defect a
/// `Statement` entry that is not a statement OBJECT falls under.
const SYNTAX_ERRORS: &str = "Syntax errors in policy.";

/// A document whose one statement names both required elements: the shape every accepted
/// row below is a variation of.
const COMPLETE: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#;

/// A managed policy ARN is `arn:aws:iam::<account>:policy<path><name>` — IAM is global,
/// so the region segment is EMPTY.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy/{policy_name}", account_id())
}

/// One `CreatePolicy` call.
async fn create_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "CreatePolicy",
        &[("PolicyName", policy_name), ("PolicyDocument", document)],
    )
    .await
}

/// One `CreatePolicyVersion` call.
async fn create_version(
    router: &MultiServiceRouter,
    arn: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "CreatePolicyVersion",
        &[("PolicyArn", arn), ("PolicyDocument", document)],
    )
    .await
}

/// The document `GetPolicyVersion` serves for one version of a policy.
async fn read_document(router: &MultiServiceRouter, arn: &str, version_id: &str) -> String {
    let (status, body) = call(
        router,
        "GetPolicyVersion",
        &[("PolicyArn", arn), ("VersionId", version_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "{arn} {version_id}: the stored version must be readable, got {status}: {body}"
    );
    crate::user_lifecycle::document_element(&body, "Document")
}

/// One row of the refusal table: a document, and the sentence IAM words its defect with.
struct RefusedCase {
    case: &'static str,
    document: &'static str,
    message: &'static str,
}

/// Every way a managed policy document's statements can fail the grammar, each with the
/// sentence that defect is refused with.
///
/// A statement missing BOTH required elements is reported for the one the grammar lists
/// first (`Action` before `Resource`), so the sentence a client reads is stable rather
/// than depending on which element the reader happened to look at first.
const REFUSED: &[RefusedCase] = &[
    RefusedCase {
        case: "an action with no resource",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject"}]}"#,
        message: NO_RESOURCES,
    },
    RefusedCase {
        case: "a NotAction with no resource",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":"iam:*"}]}"#,
        message: NO_RESOURCES,
    },
    RefusedCase {
        case: "a single statement object with no resource",
        document: r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Action":"s3:GetObject"}}"#,
        message: NO_RESOURCES,
    },
    RefusedCase {
        case: "the second of two statements with no resource",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"},{"Effect":"Deny","Action":"s3:PutObject"}]}"#,
        message: NO_RESOURCES,
    },
    RefusedCase {
        case: "a resource element naming nothing",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":[]}]}"#,
        message: NO_RESOURCES,
    },
    RefusedCase {
        case: "a null resource element",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":null}]}"#,
        message: NO_RESOURCES,
    },
    RefusedCase {
        case: "a statement carrying only a Sid",
        document: r#"{"Statement":[{"Sid":"nothing-at-all"}]}"#,
        message: NO_ACTIONS,
    },
    RefusedCase {
        case: "a resource with no action",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Resource":"*"}]}"#,
        message: NO_ACTIONS,
    },
    RefusedCase {
        case: "a NotResource with no action",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotResource":"arn:aws:s3:::reports/*"}]}"#,
        message: NO_ACTIONS,
    },
    RefusedCase {
        case: "an action element naming nothing",
        document: r#"{"Statement":[{"Effect":"Allow","Action":[],"Resource":"*"}]}"#,
        message: NO_ACTIONS,
    },
    RefusedCase {
        case: "a statement that is not an object",
        document: r#"{"Statement":[17]}"#,
        message: SYNTAX_ERRORS,
    },
    RefusedCase {
        case: "a statement that is a string",
        document: r#"{"Statement":["Allow"]}"#,
        message: SYNTAX_ERRORS,
    },
];

/// Every document whose statements DO name both required elements, in each form the
/// grammar admits for them.
const ACCEPTED: &[(&str, &str)] = &[
    ("an action and a resource", COMPLETE),
    (
        "a NotAction with a resource",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":"iam:*","Resource":"*"}]}"#,
    ),
    (
        "an action with a NotResource",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"s3:*","NotResource":"arn:aws:s3:::public/*"}]}"#,
    ),
    (
        "both elements as lists",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":["arn:aws:s3:::a/*","arn:aws:s3:::b/*"]}]}"#,
    ),
    (
        "a single statement object",
        r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Action":"sqs:SendMessage","Resource":"*"}}"#,
    ),
    (
        "two complete statements",
        r#"{"Version":"2012-10-17","Id":"two","Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"},{"Effect":"Deny","Action":"logs:DeleteLogGroup","Resource":"*"}]}"#,
    ),
    (
        "a complete statement with a condition",
        r#"{"Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#,
    ),
    // A document that declares NO statement has no statement to hold to the rule, which
    // is why the refusals above are about what a statement carries and not about the
    // document's size.
    ("no statements at all", r#"{"Statement":[]}"#),
    (
        "pretty-printed with tabs and newlines",
        "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Action\": \"sts:GetCallerIdentity\",\n\t\t\t\"Resource\": \"*\"\n\t\t}\n\t]\n}",
    ),
];

/// `CreatePolicy` refuses a document whose statement omits a required element, with the
/// sentence that element is worded by, and stores nothing.
#[tokio::test]
async fn create_policy_refuses_a_statement_that_names_no_actions_or_no_resources() {
    let router = router();
    for (index, row) in REFUSED.iter().enumerate() {
        let policy_name = format!("refused-{index:02}");
        let (status, body) = create_policy(&router, &policy_name, row.document).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for {:?}, got {status}: {body}",
            row.case, row.document
        );
        assert_eq!(
            error_code(&body),
            MALFORMED,
            "case {:?}: expected the modelled awsQuery code {MALFORMED}, got {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Message"),
            row.message,
            "case {:?}: expected the refusal to be worded {:?}, got {body}",
            row.case,
            row.message
        );
        // Nothing a later caller can read was created for a refused document.
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
    }
}

/// A document whose every statement names both required elements is created and reads
/// back verbatim through `GetPolicyVersion` — the round trip that shows the refusals
/// above are the rule and not a blanket refusal.
#[tokio::test]
async fn create_policy_stores_a_document_whose_statements_name_both_elements() {
    let router = router();
    for (index, (case, document)) in ACCEPTED.iter().enumerate() {
        let policy_name = format!("accepted-{index:02}");
        let (status, body) = create_policy(&router, &policy_name, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {document:?}, got {status}: {body}"
        );
        let arn = element(&body, "Arn");
        assert_eq!(
            arn,
            policy_arn(&policy_name),
            "case {case:?}: expected the policy's own ARN, got {arn:?}"
        );
        let stored = read_document(&router, &arn, "v1").await;
        assert_eq!(
            stored, *document,
            "case {case:?}: the stored document must be the one sent, got {stored:?}"
        );
    }
}

/// `CreatePolicyVersion` holds a new version's document to the same rule: a statement
/// omitting a required element is refused with that element's sentence, the policy keeps
/// the version it had, and a complete document then becomes `v2`.
#[tokio::test]
async fn create_policy_version_holds_a_new_version_to_the_same_rule() {
    let router = router();
    let (status, body) = create_policy(&router, "versioned", COMPLETE).await;
    assert_eq!(
        status, 200,
        "the fixture CreatePolicy must succeed, got {status}: {body}"
    );
    let arn = element(&body, "Arn");

    for row in REFUSED {
        let (status, body) = create_version(&router, &arn, row.document).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for {:?}, got {status}: {body}",
            row.case, row.document
        );
        assert_eq!(
            error_code(&body),
            MALFORMED,
            "case {:?}: expected the modelled awsQuery code {MALFORMED}, got {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Message"),
            row.message,
            "case {:?}: expected the refusal to be worded {:?}, got {body}",
            row.case,
            row.message
        );
        // A refused version changed nothing: the policy still serves what it held, and
        // no second version was minted.
        let stored = read_document(&router, &arn, "v1").await;
        assert_eq!(
            stored, COMPLETE,
            "case {:?}: a refused version must leave v1 as it was, got {stored:?}",
            row.case
        );
        let (version_status, got) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", arn.as_str()), ("VersionId", "v2")],
        )
        .await;
        assert_eq!(
            version_status, 404,
            "case {:?}: a refused version must not be stored, got {version_status}: {got}",
            row.case
        );
    }

    let (status, body) = create_version(&router, &arn, ACCEPTED[1].1).await;
    assert_eq!(
        status, 200,
        "a complete document must be accepted as a new version, got {status}: {body}"
    );
    let stored = read_document(&router, &arn, "v2").await;
    assert_eq!(
        stored, ACCEPTED[1].1,
        "the new version must serve the document it was created from, got {stored:?}"
    );
}
