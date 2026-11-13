// SPDX-License-Identifier: CC0-1.0

//! The REFUSALS whose text AWS fixes rather than builds from the request, end to end over
//! the awsQuery wire.
//!
//! Three rules, each over a family of operations rather than over one call:
//!
//! 1. a delete refused because the entity still has an attached subordinate entity carries
//!    AWS's fixed sentence for the CLASS of dependent that blocks it — the policies to
//!    detach, the policies to delete, the roles to remove from an instance profile, the
//!    users to remove from a group — and names no entity, no count and no policy
//!    (`DeleteConflictException`: "the error message describes these entities", which is
//!    the class);
//! 2. a permissions-boundary put naming a policy the account does not hold is refused with
//!    the boundary family's clause, `Scope ARN: <arn> …`, while the policy-ATTACHMENT
//!    family words the same absence `Policy <arn> …` — what a boundary call names is a
//!    SCOPE, not an attachment;
//! 3. a policy document past a size quota names the QUOTA where the quota is the account's
//!    (a managed policy's `PolicySize`) and the ENTITY where it is the entity's (an inline
//!    policy's, in `bytes`), and a document that is no IAM policy is refused with one fixed
//!    sentence per defect and per document CLASS — an identity or resource policy this
//!    emulator cannot read as a policy is `Syntax errors in policy.` whether it failed to
//!    parse or carried no `Statement`, while a role's TRUST policy tells the two apart,
//!    `This policy contains invalid Json` for one that does not parse and `Policy has no
//!    statements` for an object carrying no `Statement` — each carrying nothing of the
//!    document and nothing of the member that supplied it.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: which
//! operation of the family is called, which entity kind is addressed, the way the
//! document fails to be a policy, and the form of the value refused. Every assertion
//! compares the WHOLE message, so a sentence that merely contains AWS's text is not
//! enough.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A policy document with a single statement — the form `CreatePolicy`'s own modelled
/// example carries.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which a fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// AWS's own text for each class of blocking dependent, written out here rather than
/// taken from the implementation's constants: what these suites pin is the SENTENCE AWS
/// answers, so a reworded constant has to fail here rather than agree with itself.
///
/// Source: the `DeleteConflictException` messages AWS answers for these deletes, which
/// name the class of subordinate entity to remove and nothing of the account.
const MUST_DETACH_ALL_POLICIES: &str = "Cannot delete entity, must detach all policies first.";
const MUST_DELETE_POLICIES: &str = "Cannot delete entity, must delete policies first.";
const MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE: &str =
    "Cannot delete entity, must remove roles from instance profile first.";
const MUST_REMOVE_USERS_FROM_GROUP: &str =
    "Cannot delete entity, must remove users from group first.";

/// AWS's two fixed sentences for a value that is no IAM policy document.
const SYNTAX_ERRORS_IN_POLICY: &str = "Syntax errors in policy.";
const POLICY_HAS_NO_STATEMENTS: &str = "Policy has no statements";

/// AWS's fixed sentence for a role's TRUST policy that does not parse as a JSON object:
/// IAM validates a trust policy with its own validator, which words that defect its own
/// way.
const TRUST_POLICY_INVALID_JSON: &str = "This policy contains invalid Json";

/// The operations of this service whose document is a role's trust policy, so a document
/// this emulator cannot read as a policy at all expects the trust-policy validator's
/// sentence for them rather than the identity-policy validator's.
const TRUST_POLICY_WRITERS: &[&str] = &["CreateRole", "UpdateAssumeRolePolicy"];

/// What AWS answers a delete whose entity still carries one class of dependent:
/// `(the row's name, the delete that is refused, the fixed sentence)`.
struct DeleteConflict {
    case: &'static str,
    /// The operation whose delete is refused, and the member it names its entity by.
    action: &'static str,
    name_member: &'static str,
    /// The entity's name, created by the row's own fixture.
    entity: &'static str,
    /// AWS's fixed text for the class of dependent that blocks this delete.
    expected: &'static str,
}

/// RULE 1: every delete-conflict refusal in this service is AWS's fixed sentence for the
/// class of blocking dependent, with no entity name, no count and no policy in it.
///
/// The rows span the entity kinds a delete can be blocked on and the classes that block
/// them: a role with an attached managed policy, a role with an embedded inline policy, a
/// role still in an instance profile, that instance profile still holding the role, a user
/// with an attached policy, a user with an inline policy, and a group still holding a
/// member.
#[tokio::test]
async fn a_blocked_delete_carries_awss_fixed_sentence_for_the_blocking_class() {
    let rows = &[
        DeleteConflict {
            case: "a role with an attached managed policy",
            action: "DeleteRole",
            name_member: "RoleName",
            entity: "attached-role",
            expected: MUST_DETACH_ALL_POLICIES,
        },
        DeleteConflict {
            case: "a role with an embedded inline policy",
            action: "DeleteRole",
            name_member: "RoleName",
            entity: "embedded-role",
            expected: MUST_DELETE_POLICIES,
        },
        DeleteConflict {
            case: "a role still held by an instance profile",
            action: "DeleteRole",
            name_member: "RoleName",
            entity: "profiled-role",
            expected: MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE,
        },
        DeleteConflict {
            case: "an instance profile still holding a role",
            action: "DeleteInstanceProfile",
            name_member: "InstanceProfileName",
            entity: "holding-profile",
            expected: MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE,
        },
        DeleteConflict {
            case: "a user with an attached managed policy",
            action: "DeleteUser",
            name_member: "UserName",
            entity: "attached-user",
            expected: MUST_DETACH_ALL_POLICIES,
        },
        DeleteConflict {
            case: "a user with an embedded inline policy",
            action: "DeleteUser",
            name_member: "UserName",
            entity: "embedded-user",
            expected: MUST_DELETE_POLICIES,
        },
        DeleteConflict {
            case: "a group still holding a member",
            action: "DeleteGroup",
            name_member: "GroupName",
            entity: "holding-group",
            expected: MUST_REMOVE_USERS_FROM_GROUP,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        // One policy per row, so nothing but the row's own dependent can block its delete.
        let policy_name = format!("{}-policy", row.entity);
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", &policy_name),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture policy must be created, got {status}: {body}"
        );
        let policy_arn = element(&body, "Arn");

        // The row's fixture: the entity, and the ONE dependent that must block its delete.
        match row.case {
            "a role with an attached managed policy" => {
                create_role(&router, case, row.entity).await;
                expect_ok(
                    &router,
                    case,
                    "AttachRolePolicy",
                    &[("RoleName", row.entity), ("PolicyArn", &policy_arn)],
                )
                .await;
            }
            "a role with an embedded inline policy" => {
                create_role(&router, case, row.entity).await;
                expect_ok(
                    &router,
                    case,
                    "PutRolePolicy",
                    &[
                        ("RoleName", row.entity),
                        ("PolicyName", "embedded"),
                        ("PolicyDocument", POLICY_DOCUMENT),
                    ],
                )
                .await;
            }
            "a role still held by an instance profile" | "an instance profile still holding a role" => {
                let role = "profiled-role";
                let profile = "holding-profile";
                create_role(&router, case, role).await;
                expect_ok(
                    &router,
                    case,
                    "CreateInstanceProfile",
                    &[("InstanceProfileName", profile)],
                )
                .await;
                expect_ok(
                    &router,
                    case,
                    "AddRoleToInstanceProfile",
                    &[("InstanceProfileName", profile), ("RoleName", role)],
                )
                .await;
            }
            "a user with an attached managed policy" => {
                expect_ok(&router, case, "CreateUser", &[("UserName", row.entity)]).await;
                expect_ok(
                    &router,
                    case,
                    "AttachUserPolicy",
                    &[("UserName", row.entity), ("PolicyArn", &policy_arn)],
                )
                .await;
            }
            "a user with an embedded inline policy" => {
                expect_ok(&router, case, "CreateUser", &[("UserName", row.entity)]).await;
                expect_ok(
                    &router,
                    case,
                    "PutUserPolicy",
                    &[
                        ("UserName", row.entity),
                        ("PolicyName", "embedded"),
                        ("PolicyDocument", POLICY_DOCUMENT),
                    ],
                )
                .await;
            }
            _ => {
                expect_ok(&router, case, "CreateGroup", &[("GroupName", row.entity)]).await;
                expect_ok(&router, case, "CreateUser", &[("UserName", "member")]).await;
                expect_ok(
                    &router,
                    case,
                    "AddUserToGroup",
                    &[("GroupName", row.entity), ("UserName", "member")],
                )
                .await;
            }
        }

        let (status, body) = call(
            &router,
            row.action,
            &[(row.name_member, row.entity)],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the delete conflict's HTTP 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {case:?}: expected DeleteConflict, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected AWS's fixed sentence, got {message:?}"
        );
        // Nothing of the account's shape is in it: not the entity, not the policy.
        assert!(
            !message.contains(row.entity) && !message.contains(&policy_name),
            "case {case:?}: the refusal must name no entity of this account, got {message:?}"
        );
    }
}

/// Create the fixture role `name`, asserting the create succeeded.
async fn create_role(router: &smithy_server_runtime::MultiServiceRouter, case: &str, name: &str) {
    expect_ok(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
}

/// Drive one call a row's fixture needs, asserting it succeeded.
async fn expect_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture call {action} must succeed, got {status}: {body}"
    );
    body
}

/// RULE 2: a permissions-boundary put naming a policy the account holds nothing under is
/// refused with the boundary family's clause — `Scope ARN: <arn> does not exist or is not
/// attachable.` — while the policy-ATTACHMENT family words the same absence `Policy <arn>
/// …`, so the two families stay apart whichever entity kind the call addressed.
#[tokio::test]
async fn a_boundary_put_on_an_absent_policy_names_the_scope_arn() {
    let router = router();
    let account = account_id();
    create_role(&router, "the boundary fixtures", "boundary-role").await;
    expect_ok(
        &router,
        "the boundary fixtures",
        "CreateUser",
        &[("UserName", "boundary-user")],
    )
    .await;

    // Rows spanning the forms a well-formed managed-policy ARN takes that the account
    // holds no policy under, and both entity kinds a boundary can be set on.
    let unheld: Vec<(&str, String)> = vec![
        (
            "a name nothing was created under",
            format!("arn:aws:iam::{account}:policy/NeverCreated"),
        ),
        (
            "a deeply nested path",
            format!("arn:aws:iam::{account}:policy/app/audit/2026/Ghost"),
        ),
        (
            "another account's policy",
            "arn:aws:iam::210987654321:policy/Elsewhere".to_string(),
        ),
        (
            "a name of the punctuation the name alphabet admits",
            format!("arn:aws:iam::{account}:policy/ghost.name+2=x@y_-"),
        ),
    ];
    let boundary_calls: &[(&str, &str, &str)] = &[
        ("PutRolePermissionsBoundary", "RoleName", "boundary-role"),
        ("PutUserPermissionsBoundary", "UserName", "boundary-user"),
    ];

    for (what, arn) in &unheld {
        for (action, member, entity) in boundary_calls {
            let case = format!("{action} on {what}");
            let params = vec![
                param(member, entity),
                param("PermissionsBoundary", arn),
            ];
            let (status, body) = call(&router, action, &borrowed(&params)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                format!("Scope ARN: {arn} does not exist or is not attachable."),
                "case {case:?}: expected the boundary family's wording, got {message:?}"
            );
        }

        // The CONTRAST that keeps the clause sited per family: an attach of the same ARN
        // reports the absence with the attachment family's opening.
        let params = vec![param("RoleName", "boundary-role"), param("PolicyArn", arn)];
        let (status, body) = call(&router, "AttachRolePolicy", &borrowed(&params)).await;
        assert_eq!(
            status, 404,
            "case {what:?}: expected HTTP 404 from AttachRolePolicy, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("Policy {arn} does not exist or is not attachable."),
            "case {what:?}: expected the attachment family's wording"
        );
    }
}

/// RULE 3 (the quota half): a MANAGED policy document past the account's `PolicySize`
/// quota is refused naming that quota, while an INLINE policy document past the embedding
/// entity's quota is refused naming the entity and the quota in `bytes`.
///
/// The rows span both operations that take a managed-policy document (the create and the
/// version), all three entity kinds that embed an inline policy, and \u2014 per row \u2014 the
/// published bound itself: one significant character past it.
#[tokio::test]
async fn a_policy_document_past_a_quota_names_the_quota_it_broke() {
    let router = router();
    let quota = state::MAX_MANAGED_POLICY_CHARACTERS;

    // A document whose significant size is `size`, built around the smallest IAM policy
    // this service accepts: the filler rides in a statement's `Sid`, which the grammar
    // admits and the measure counts.
    const SKELETON: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"{}","Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;
    let document_of = |size: usize| {
        let overhead = state::policy_document_characters(&SKELETON.replace("{}", ""));
        let padding = size.saturating_sub(overhead);
        SKELETON.replace("{}", &"A".repeat(padding))
    };

    // The managed half: the create and the version call, both measured against the
    // account's `PolicySize` quota.
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[("PolicyName", "sized"), ("PolicyDocument", POLICY_DOCUMENT)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture policy must be created, got {status}: {body}"
    );
    let policy_arn = element(&body, "Arn");
    let over_quota = document_of(quota + 1);
    let managed_calls: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "CreatePolicy",
            vec![
                param("PolicyName", "over-quota"),
                param("PolicyDocument", &over_quota),
            ],
        ),
        (
            "CreatePolicyVersion",
            vec![
                param("PolicyArn", &policy_arn),
                param("PolicyDocument", &over_quota),
            ],
        ),
    ];
    for (action, params) in &managed_calls {
        let (status, body) = call(&router, action, &borrowed(params)).await;
        assert_eq!(
            status, 409,
            "case {action:?}: expected HTTP 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {action:?}: expected LimitExceeded, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("Cannot exceed quota for PolicySize: {quota}"),
            "case {action:?}: expected the quota's own wording, got {message:?}"
        );
        assert!(
            !message.contains("over-quota") && !message.contains(&policy_arn),
            "case {action:?}: a quota refusal names no policy, got {message:?}"
        );
    }

    // The inline half: the entity's quota, in `bytes`, naming the entity.
    create_role(&router, "the inline fixtures", "sized-role").await;
    expect_ok(
        &router,
        "the inline fixtures",
        "CreateUser",
        &[("UserName", "sized-user")],
    )
    .await;
    expect_ok(
        &router,
        "the inline fixtures",
        "CreateGroup",
        &[("GroupName", "sized-group")],
    )
    .await;
    let inline_rows: &[(&str, &str, &str, &str, usize)] = &[
        (
            "PutRolePolicy",
            "RoleName",
            "sized-role",
            "role",
            state::MAX_ROLE_INLINE_POLICY_CHARACTERS,
        ),
        (
            "PutUserPolicy",
            "UserName",
            "sized-user",
            "user",
            state::MAX_USER_INLINE_POLICY_CHARACTERS,
        ),
        (
            "PutGroupPolicy",
            "GroupName",
            "sized-group",
            "group",
            state::MAX_GROUP_INLINE_POLICY_CHARACTERS,
        ),
    ];
    for (action, member, entity, kind, bound) in inline_rows {
        let document = document_of(bound + 1);
        let params = vec![
            param(member, entity),
            param("PolicyName", "too-big"),
            param("PolicyDocument", &document),
        ];
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 409,
            "case {action:?}: expected HTTP 409, got {status}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("Maximum policy size of {bound} bytes exceeded for {kind} {entity}"),
            "case {action:?}: expected the entity's own quota wording, got {message:?}"
        );
    }
}

/// RULE 3 (the malformed half): a value supplied where an IAM policy document is expected
/// is refused with ONE fixed sentence per defect and per document class — an identity or
/// resource policy this emulator cannot read as a policy is `Syntax errors in policy.`
/// whether it failed to parse or carried no `Statement`, while a role's trust policy is
/// read by IAM's own trust-policy validator, which answers `This policy contains invalid
/// Json` for the first and `Policy has no statements` for the second — whichever operation
/// and whichever member supplied it.
#[tokio::test]
async fn a_document_that_is_no_policy_is_refused_with_one_sentence_per_defect() {
    // The rows span the whole class each defect generalises over: text that is not JSON at
    // all, JSON that is a list, a number or a string rather than an object, an empty
    // object, and an object carrying every top-level element of the grammar EXCEPT
    // `Statement`. The third column is the sentence the TRUST-policy validator answers,
    // which is the only one that tells the two defects apart; every other member's is
    // `SYNTAX_ERRORS_IN_POLICY`.
    let defects: &[(&str, &str, &str)] = &[
        ("not JSON at all", "trust me", TRUST_POLICY_INVALID_JSON),
        (
            "an unterminated object",
            r#"{"Version":"2012-10-17""#,
            TRUST_POLICY_INVALID_JSON,
        ),
        (
            "a JSON list",
            r#"[{"Effect":"Allow"}]"#,
            TRUST_POLICY_INVALID_JSON,
        ),
        ("a JSON number", "17", TRUST_POLICY_INVALID_JSON),
        (
            "a JSON string",
            r#""Statement""#,
            TRUST_POLICY_INVALID_JSON,
        ),
        ("an empty object", "{}", POLICY_HAS_NO_STATEMENTS),
        (
            "an object with every element but Statement",
            r#"{"Version":"2012-10-17","Id":"a-policy"}"#,
            POLICY_HAS_NO_STATEMENTS,
        ),
        (
            "an object naming the element in another case",
            r#"{"Version":"2012-10-17","statement":[]}"#,
            POLICY_HAS_NO_STATEMENTS,
        ),
    ];

    for (index, (what, document, expected)) in defects.iter().enumerate() {
        let router = router();
        // The fixtures every row's calls address, created fresh so a refused call is the
        // only thing that could have changed the account.
        create_role(&router, what, "doc-role").await;
        expect_ok(&router, what, "CreateUser", &[("UserName", "doc-user")]).await;
        expect_ok(&router, what, "CreateGroup", &[("GroupName", "doc-group")]).await;
        let (_, body) = call(
            &router,
            "CreatePolicy",
            &[("PolicyName", "doc-policy"), ("PolicyDocument", POLICY_DOCUMENT)],
        )
        .await;
        let policy_arn = element(&body, "Arn");

        // Every operation of this service that takes a policy document, and the member it
        // takes it in — a trust policy, a managed policy, a managed policy version, and
        // the three inline policy puts.
        let name = format!("refused-{index}");
        let calls: Vec<(&str, Vec<(String, String)>)> = vec![
            (
                "CreateRole",
                vec![
                    param("RoleName", &name),
                    param("AssumeRolePolicyDocument", document),
                ],
            ),
            (
                "UpdateAssumeRolePolicy",
                vec![
                    param("RoleName", "doc-role"),
                    param("PolicyDocument", document),
                ],
            ),
            (
                "CreatePolicy",
                vec![
                    param("PolicyName", &name),
                    param("PolicyDocument", document),
                ],
            ),
            (
                "CreatePolicyVersion",
                vec![
                    param("PolicyArn", &policy_arn),
                    param("PolicyDocument", document),
                ],
            ),
            (
                "PutRolePolicy",
                vec![
                    param("RoleName", "doc-role"),
                    param("PolicyName", "inline"),
                    param("PolicyDocument", document),
                ],
            ),
            (
                "PutUserPolicy",
                vec![
                    param("UserName", "doc-user"),
                    param("PolicyName", "inline"),
                    param("PolicyDocument", document),
                ],
            ),
            (
                "PutGroupPolicy",
                vec![
                    param("GroupName", "doc-group"),
                    param("PolicyName", "inline"),
                    param("PolicyDocument", document),
                ],
            ),
        ];

        for (action, params) in &calls {
            let case = format!("{action} with {what}");
            let (status, body) = call(&router, action, &borrowed(params)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "MalformedPolicyDocument",
                "case {case:?}: expected MalformedPolicyDocument, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            // A role's trust policy is read by IAM's trust-policy validator, which words
            // each of these two defects its own way; every other member here takes an
            // identity or resource policy, whose validator gives no diagnosis of a
            // document it cannot read as a policy at all and answers one sentence for
            // both.
            let expected = if TRUST_POLICY_WRITERS.contains(action) {
                *expected
            } else {
                SYNTAX_ERRORS_IN_POLICY
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected AWS's fixed sentence for this defect, got \
                 {message:?}"
            );
        }

        // A refused document persisted nothing: the role the create would have minted is
        // not there, and the role whose trust policy an update was refused still carries
        // the one it was created with.
        let (status, body) = call(&router, "GetRole", &[("RoleName", &name)]).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused create must persist nothing, got {status}: {body}"
        );
        let (status, body) = call(&router, "GetRole", &[("RoleName", "doc-role")]).await;
        assert_eq!(status, 200, "case {what:?}: the fixture role must survive");
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "AssumeRolePolicyDocument"),
            TRUST_POLICY,
            "case {what:?}: a refused update must leave the held document alone"
        );
    }
}
