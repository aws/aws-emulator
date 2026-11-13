// SPDX-License-Identifier: CC0-1.0

//! `UpdateAssumeRolePolicy` — replacing a role's trust policy — end to end over the
//! awsQuery wire.
//!
//! RULE: `UpdateAssumeRolePolicy` REPLACES the trust policy held on the account's own
//! record of the named role with the document the request carried, stored verbatim, so
//! every later read of that record (`GetRole`) reports the new document and nothing
//! else about the role changes; the role is matched by its case-folded name; a name the
//! account does not hold is the modelled `NoSuchEntityException` (HTTP 404,
//! awsQuery code `NoSuchEntity`); a document that is not an IAM JSON policy is
//! `MalformedPolicyDocumentException` (HTTP 400, `MalformedPolicyDocument`); and a
//! service-linked role is a protected resource, so the update is
//! `UnmodifiableEntityException` (HTTP 400, `UnmodifiableEntity`). A refused call
//! leaves the held document exactly as it was.
//!
//! No single recorded request is the specification, so every suite here is a TABLE
//! whose rows differ along what the rule generalises over: the form of the policy
//! document (statement list, bare statement object, several statements, a `Condition`
//! block, the tab/line-feed/carriage-return and Latin-1 characters
//! `policyDocumentType`'s pattern admits, the smallest document the IAM policy grammar
//! admits), the number of successive updates, the role's path, the alphabet and case of
//! the role name, and the service a role is linked to. The awsQuery harness (router,
//! call, element, elements, error_code) is the one `tests/all/user_lifecycle.rs` owns
//! and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::protected_role_refusals::protected;
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The trust policy every role this suite creates starts out with: an IAM policy
/// document, which is all `CreateRole` asks of one.
const INITIAL_TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// Trust-policy documents spanning the forms the rule generalises over, each with the text
/// IAM stores for it. Each submitted document is an IAM JSON policy document — a JSON object
/// carrying a `Statement` element — and each stored one is that document RENDERED (the
/// whitespace between its JSON tokens gone, a one-element string-or-array element written as
/// its string), which is what every read of the role reports.
const DOCUMENTS: &[(&str, &str, &str)] = &[
    (
        "a statement list naming an account principal",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"sts:AssumeRole"}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"sts:AssumeRole"}]}"#,
    ),
    (
        "a bare statement object naming a service principal",
        r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}}"#,
        r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}}"#,
    ),
    (
        "a statement carrying an MFA condition",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:user/alice"},"Action":"sts:AssumeRole","Condition":{"Bool":{"aws:MultiFactorAuthPresent":"true"}}}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:user/alice"},"Action":"sts:AssumeRole","Condition":{"Bool":{"aws:MultiFactorAuthPresent":"true"}}}]}"#,
    ),
    (
        "a statement carrying a principal-ARN condition",
        r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"sts:AssumeRole","Condition":{"ArnEquals":{"aws:PrincipalArn":"arn:aws:iam::123456789012:user/bob"}}}]}"#,
        r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"sts:AssumeRole","Condition":{"ArnEquals":{"aws:PrincipalArn":"arn:aws:iam::123456789012:user/bob"}}}]}"#,
    ),
    (
        "a statement carrying a session-name condition",
        r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"sts:AssumeRole","Condition":{"StringLike":{"sts:RoleSessionName":"build-*"}}}]}"#,
        r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"sts:AssumeRole","Condition":{"StringLike":{"sts:RoleSessionName":"build-*"}}}]}"#,
    ),
    (
        "two statements, one allowing and one denying",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"},{"Effect":"Deny","Principal":{"AWS":"*"},"Action":"sts:AssumeRole","Condition":{"BoolIfExists":{"aws:MultiFactorAuthPresent":"false"}}}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"},{"Effect":"Deny","Principal":{"AWS":"*"},"Action":"sts:AssumeRole","Condition":{"BoolIfExists":{"aws:MultiFactorAuthPresent":"false"}}}]}"#,
    ),
    (
        // `policyDocumentType`'s pattern admits tab (\u0009), line feed (\u000A) and
        // carriage return (\u000D), so an indented document is a legal one to SUBMIT; what
        // IAM stores for it is the rendered document, without that whitespace.
        "an indented document carrying tabs, line feeds and a carriage return",
        "{\r\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Principal\": {\"Service\": \"ecs.amazonaws.com\"},\n\t\t\t\"Action\": \"sts:AssumeRole\"\n\t\t}\n\t]\n}",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ecs.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
    ),
    (
        // The pattern also admits "the printable characters in the Basic Latin and
        // Latin-1 Supplement character set (through \u00FF)"
        // (`UpdateAssumeRolePolicy$PolicyDocument`), so a Latin-1 `Sid` survives too.
        "a document whose Sid carries Latin-1 Supplement characters",
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-münster","Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-münster","Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
    ),
    (
        // Validation stops at the top level of the IAM policy grammar
        // (`state::is_policy_document`), so the smallest document the grammar admits is
        // stored rather than refused — and `Statement` is not an element the rendering
        // collapses, so its one-element array stays an array.
        "the smallest document the IAM policy grammar admits",
        r#"{"Statement":[]}"#,
        r#"{"Statement":[]}"#,
    ),
];

/// One role to drive the rule over: the path it is created at, and the spelling the
/// update names it by (IAM entity names "are not distinguished by case", so a spelling
/// that differs from the created one still names the same role).
struct RoleCase {
    case: &'static str,
    role_name: &'static str,
    path: &'static str,
    /// How the update and the read spell the role's name.
    spelling: &'static str,
}

/// Rows spanning the account root, a one-segment path and a nested path; names drawn
/// from the `^[\w+=,.@-]+$` alphabet `roleNameType` admits
/// (`smithy_query(query="operation", operation="UpdateAssumeRolePolicy")`), in the
/// created case, upper-cased and lower-cased.
const ROLES: &[RoleCase] = &[
    RoleCase {
        case: "the account root, named exactly as created",
        role_name: "IngestRole",
        path: "/",
        spelling: "IngestRole",
    },
    RoleCase {
        case: "a one-segment path, named in upper case",
        role_name: "build_role+2",
        path: "/service-role/",
        spelling: "BUILD_ROLE+2",
    },
    RoleCase {
        case: "a nested path, named in lower case",
        role_name: "Team.Role=A@x-1",
        path: "/application/eu/team-1/",
        spelling: "team.role=a@x-1",
    },
];

/// Create one role carrying [`INITIAL_TRUST_POLICY`].
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", INITIAL_TRUST_POLICY),
        ],
    )
    .await;
    assert!(
        status == 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// One `UpdateAssumeRolePolicy` call.
async fn update(
    router: &MultiServiceRouter,
    role_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateAssumeRolePolicy",
        &[("RoleName", role_name), ("PolicyDocument", document)],
    )
    .await
}

/// The trust policy `GetRole` reports for a role the account holds, percent-decoded back to
/// plain JSON as IAM's contract tells a caller to read a policy document.
async fn reported_trust_policy(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert!(
        status == 200,
        "case {role_name:?}: GetRole must succeed, got {status}: {body}"
    );
    crate::user_lifecycle::document_element(&body, "AssumeRolePolicyDocument")
}

/// Every member `GetRole` reports for a role, apart from its trust policy — the record
/// an update must leave alone.
async fn reported_role_identity(
    router: &MultiServiceRouter,
    role_name: &str,
) -> Vec<(&'static str, String)> {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert!(
        status == 200,
        "case {role_name:?}: GetRole must succeed, got {status}: {body}"
    );
    ["RoleName", "RoleId", "Arn", "Path", "CreateDate", "MaxSessionDuration"]
        .into_iter()
        .map(|tag| (tag, element(&body, tag)))
        .collect()
}

/// RULE: the update replaces the role's trust policy with the document the request
/// carried, stored verbatim, and a later `GetRole` reports it — however many times the
/// policy is replaced, and whatever form each document takes.
#[tokio::test]
async fn an_update_replaces_the_trust_policy_and_get_role_reads_it_back() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
        let identity_before = reported_role_identity(&router, role.role_name).await;
        assert_eq!(
            reported_trust_policy(&router, role.role_name).await,
            INITIAL_TRUST_POLICY,
            "case {:?}: the created role must report the document the create carried",
            role.case
        );

        // Each row of DOCUMENTS is a further replacement of the SAME role's policy, so
        // the rule is asserted over a sequence of updates rather than one.
        for (document_case, document, stored) in DOCUMENTS {
            let case = format!("{} / {document_case}", role.case);
            let (status, body) = update(&router, role.spelling, document).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateAssumeRolePolicy to answer 200, got {status}: \
                 {body}"
            );
            assert!(
                elements(&body, "Code").is_empty(),
                "case {case:?}: a successful update must carry no error code: {body}"
            );

            let reported = reported_trust_policy(&router, role.spelling).await;
            assert_eq!(
                reported, *stored,
                "case {case:?}: expected the stored document to read back, got {reported:?}"
            );
            // The reported text is still the JSON policy that was sent, not an escaped
            // or re-encoded form of it.
            let parsed: serde_json::Value = serde_json::from_str(&reported)
                .unwrap_or_else(|error| panic!("case {case:?}: {error} in {reported:?}"));
            let sent: serde_json::Value = serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("case {case:?}: {error} in {document:?}"));
            assert_eq!(
                parsed, sent,
                "case {case:?}: expected the reported policy to parse to the sent one, got \
                 {parsed:?}"
            );

            // Nothing else about the role moved: the update changes one member.
            let identity_now = reported_role_identity(&router, role.spelling).await;
            assert_eq!(
                identity_now, identity_before,
                "case {case:?}: expected the rest of the role to be untouched, got \
                 {identity_now:?}"
            );
        }
    }
}

/// RULE: the trust policy of one role is the trust policy of that role alone — an
/// update changes no other role's document.
#[tokio::test]
async fn an_update_changes_no_other_roles_trust_policy() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
    }

    for (index, role) in ROLES.iter().enumerate() {
        let (document_case, document, _) = DOCUMENTS[index];
        let case = format!("{} / {document_case}", role.case);
        let (status, body) = update(&router, role.spelling, document).await;
        assert_eq!(status, 200, "case {case:?}: update failed: {body}");

        for (other_index, other) in ROLES.iter().enumerate() {
            let expected = if other_index <= index {
                DOCUMENTS[other_index].2
            } else {
                INITIAL_TRUST_POLICY
            };
            let reported = reported_trust_policy(&router, other.role_name).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {:?} to report {expected:?}, got {reported:?}",
                other.role_name
            );
        }
    }
}

/// RULE: a name the ACCOUNT does not hold and no service keeps provisioned "referenced a
/// resource entity that does not exist" — the modelled `NoSuchEntityException` (HTTP 404)
/// — and the call persists nothing.
#[tokio::test]
async fn updating_a_role_the_account_does_not_hold_is_not_found() {
    let router = router();
    create_role(&router, "HeldRole", "/").await;

    // Rows: a plain absent name, names one character either side of a held one, and the
    // whole admitted alphabet. A role name AWS PUBLISHES for a service's own
    // service-linked role is NOT among them: that name resolves to the role the service
    // keeps provisioned, which `updating_a_service_linked_roles_trust_policy_is_refused`
    // asserts is refused as unmodifiable rather than reported absent.
    let absent = [
        "absent",
        "HeldRol",
        "HeldRoles",
        "role_with+every=allowed.char@-",
    ];
    for role_name in absent {
        let (status, body) = update(&router, role_name, DOCUMENTS[0].1).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected 404 for an unheld role, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refused update minted nothing: the name still resolves to no role of the
        // account's.
        let (get_status, got) = call(&router, "GetRole", &[("RoleName", role_name)]).await;
        assert!(
            get_status == 404 || element(&got, "Path").starts_with("/aws-service-role/"),
            "case {role_name:?}: a refused update must persist no role, got {get_status}: {got}"
        );
    }

    // The role the account does hold kept the document it was created with.
    assert_eq!(
        reported_trust_policy(&router, "HeldRole").await,
        INITIAL_TRUST_POLICY,
        "the held role's trust policy must be untouched by the refused updates"
    );
}

/// RULE: a trust policy must be an IAM policy document — a JSON object carrying a
/// `Statement` element, the rule `state::is_policy_document` holds for every operation
/// in this crate that takes one — and anything else is refused with the modelled
/// `MalformedPolicyDocumentException` (HTTP 400), whatever `policyDocumentType`'s own
/// pattern allows. The role keeps the document it already held.
#[tokio::test]
async fn a_trust_policy_that_is_not_a_json_policy_is_refused() {
    let router = router();
    create_role(&router, "MalformedTarget", "/").await;

    // Rows spanning the forms the rule generalises over: not JSON at all, each JSON
    // type that is not an object, an object with no Statement, and a truncated object.
    let refused = [
        ("not JSON at all", "this is not a policy"),
        ("a JSON string", r#""Statement""#),
        ("a JSON array of statements", r#"[{"Effect":"Allow"}]"#),
        ("a JSON number", "17"),
        ("a JSON boolean", "true"),
        ("a JSON null", "null"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        (
            "an object carrying every top-level element but Statement",
            r#"{"Version":"2012-10-17","Id":"one"}"#,
        ),
        ("an empty object", "{}"),
        ("a truncated object", r#"{"Statement":[{"Effect":"Allow"}"#),
        (
            "an object whose Statement key is mis-cased",
            r#"{"Version":"2012-10-17","statement":[{"Effect":"Allow"}]}"#,
        ),
    ];
    for (case, document) in refused {
        let (status, body) = update(&router, "MalformedTarget", document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a malformed trust policy, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "MalformedPolicyDocument",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        let reported = reported_trust_policy(&router, "MalformedTarget").await;
        assert_eq!(
            reported, INITIAL_TRUST_POLICY,
            "case {case:?}: a refused update must leave the held policy alone, got {reported:?}"
        );
    }
}

/// RULE: "service-linked roles are protected Amazon Web Services resources. Only the
/// service that depends on the service-linked role can modify or delete the role on
/// your behalf" — so a caller's update of such a role's trust policy is refused with
/// the modelled `UnmodifiableEntityException` (HTTP 400) and the role keeps the trust
/// policy the service's own role carries. The protection is about the ROLE, so it holds
/// both for one this account created through `CreateServiceLinkedRole` and for one a
/// service keeps provisioned under its published name, which `GetRole` reports without
/// the account creating anything.
#[tokio::test]
async fn updating_a_service_linked_roles_trust_policy_is_refused() {
    let router = router();

    // Rows spanning the service the role is linked to, and HOW the role came to be: the
    // first three are created by this account, the last two are names AWS publishes that
    // nothing here created (source: `state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES`, the
    // catalog `tests/all/service_provisioned_roles.rs` pins).
    let principals = [
        "lambda.amazonaws.com",
        "elasticbeanstalk.amazonaws.com",
        "autoscaling.amazonaws.com",
    ];
    let mut roles: Vec<String> = Vec::new();
    for principal in principals {
        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: CreateServiceLinkedRole must succeed, got {status}: {created}"
        );
        roles.push(element(&created, "RoleName"));
    }
    roles.push("AWSServiceRoleForBackup".to_string());
    roles.push("AWSServiceRoleForECS".to_string());

    for role_name in roles {
        let held = reported_trust_policy(&router, &role_name).await;
        assert!(
            !held.is_empty(),
            "case {role_name:?}: the role must be reported before its update is refused, \
             got {held:?}"
        );

        for (document_case, document, _) in DOCUMENTS.iter().take(3) {
            let case = format!("{role_name} / {document_case}");
            let (status, body) = update(&router, &role_name, document).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for a service-linked role, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnmodifiableEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                protected(&role_name),
                "case {case:?}: expected IAM's protected-role sentence, got {message:?}"
            );
            let reported = reported_trust_policy(&router, &role_name).await;
            assert_eq!(
                reported, held,
                "case {case:?}: a refused update must leave the service's trust policy alone, \
                 got {reported:?}"
            );
        }
    }
}
