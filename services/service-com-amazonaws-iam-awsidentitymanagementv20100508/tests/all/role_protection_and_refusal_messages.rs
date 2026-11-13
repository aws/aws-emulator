// SPDX-License-Identifier: CC0-1.0

//! What IAM answers a caller that asks for something it will not do, end to end over the
//! awsQuery wire: the SENTENCE of the refusal, and WHICH role the service-linked-role
//! protection covers.
//!
//! Three rules, each over a family of operations rather than over one call:
//!
//! 1. a detach refused because the named entity does not carry the policy names the
//!    POLICY the request addressed — `Policy <arn> was not found.`, the sentence this
//!    service words every unresolvable `PolicyArn` with — and nothing of the entity, so
//!    the text does not vary with the entity kind or its name;
//! 2. a second role added to an instance profile is refused `LimitExceeded` naming the
//!    quota IAM names for it, `InstanceSessionsPerInstanceProfile`, with the quota's
//!    value — the quota NAME is the only machine-readable discriminator in the body;
//! 3. a role a SERVICE keeps provisioned under the name AWS publishes for it is a role
//!    that exists and is modifiable by AWS alone, so every operation of this service that
//!    protects a service-linked role refuses one addressed to such a name with
//!    `UnmodifiableEntity` (HTTP 400) and IAM's protected-role sentence, not with
//!    `NoSuchEntity`.
//!
//! And one rule about a document that is no policy at all: the validator that reads an
//! IDENTITY or resource policy gives no diagnosis of a document it cannot read as a
//! policy — `Syntax errors in policy.` whether the text failed to parse or parsed into an
//! object carrying no `Statement` — while a role's TRUST policy is read by IAM's own
//! trust-policy validator, which tells those two apart. The sentences themselves are
//! tabled by `tests/all/fixed_refusal_wording.rs`; what this suite adds is the pair of
//! document classes side by side on the SAME document.
//!
//! Each test is a TABLE whose rows differ along what its rule generalises over: which
//! operation of the family is called, which entity kind it addresses, the owner and path
//! of the policy named, the service a provisioned role is linked to, and the way a
//! document fails to be a policy. Every assertion compares the WHOLE message, so a
//! sentence that merely contains AWS's text is not enough.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    self, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, RoleSessionRecord,
};

use crate::protected_role_refusals::protected;
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which every fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// AWS's fixed sentence for an identity or resource policy document IAM cannot read as a
/// policy, written out here rather than taken from the implementation's constant: what
/// this suite pins is the sentence AWS answers, so a reworded constant has to fail here
/// rather than agree with itself.
const SYNTAX_ERRORS_IN_POLICY: &str = "Syntax errors in policy.";

/// AWS's fixed sentence for a role's TRUST policy that parses into an object carrying no
/// `Statement`: IAM reads a trust policy with a validator of its own.
const POLICY_HAS_NO_STATEMENTS: &str = "Policy has no statements";

/// One call that must succeed, so a row's fixture is really there.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {action} must succeed, got {status}: {body}"
    );
    body
}

/// One call a table row drives: the operation, and the form parameters that address this
/// row's fixture.
struct Call<'a> {
    action: &'a str,
    parameters: Vec<(&'a str, &'a str)>,
}

/// [`Call`], with the fixed sentence the operation's document class is refused with.
struct DocumentCall<'a> {
    call: Call<'a>,
    expected: &'a str,
}

/// RULE 1: what a detach addresses is the POLICY, so a policy the named entity does not
/// carry is refused 404 `NoSuchEntity` with `Policy <arn> was not found.` — the ARN
/// exactly as the request supplied it and nothing of the entity.
///
/// The rows span the three entity kinds a managed policy attaches to and the owner and
/// path of the policy named: one this account holds at the account root, one it holds
/// under a nested path, and one AWS owns that no account record carries.
#[tokio::test]
async fn a_detach_refusal_names_only_the_policy_it_was_asked_about() {
    let router = router();
    ok(&router, "fixture", "CreateUser", &[("UserName", "detach-user")]).await;
    ok(
        &router,
        "fixture",
        "CreateGroup",
        &[("GroupName", "DetachGroup")],
    )
    .await;
    ok(
        &router,
        "fixture",
        "CreateRole",
        &[
            ("RoleName", "DetachRole"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    for (name, path) in [("RootPolicy", "/"), ("NestedPolicy", "/app/audit/")] {
        ok(
            &router,
            name,
            "CreatePolicy",
            &[
                ("PolicyName", name),
                ("Path", path),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
    }

    // (the detach operation, the member it names its entity by, the fixture entity)
    let detaches = [
        ("DetachUserPolicy", "UserName", "detach-user"),
        ("DetachGroupPolicy", "GroupName", "DetachGroup"),
        ("DetachRolePolicy", "RoleName", "DetachRole"),
    ];
    // The policies no fixture entity carries: two of the account's own at different
    // paths, and one of AWS's own (an entry of the AWS Managed Policy Reference).
    let unattached = [
        format!("arn:aws:iam::{}:policy/RootPolicy", account_id()),
        format!("arn:aws:iam::{}:policy/app/audit/NestedPolicy", account_id()),
        "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess".to_string(),
    ];

    for (action, member, entity) in detaches {
        for policy_arn in &unattached {
            let case = format!("{action} on {policy_arn}");
            let (status, body) = call(
                &router,
                action,
                &[(member, entity), ("PolicyArn", policy_arn)],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                format!("Policy {policy_arn} was not found."),
                "case {case:?}: expected IAM's policy-not-found sentence, got {message:?}"
            );
            assert!(
                !message.contains(entity),
                "case {case:?}: the refusal must not name the entity, got {message:?}"
            );
        }
    }
}

/// RULE 2: "an instance profile can contain only one role, and this quota cannot be
/// increased" — so adding another is refused 409 `LimitExceeded` with
/// `Cannot exceed quota for InstanceSessionsPerInstanceProfile: <quota>`, and the profile
/// still holds the one role it held.
///
/// The rows span the profile's path and the role that is added second: a role at the
/// account root, one under a path, and the SAME role under another spelling of its name
/// (IAM names "are not distinguished by case"), so the refusal is asserted on the quota
/// rather than on one pair of names.
#[tokio::test]
async fn adding_a_second_role_to_an_instance_profile_names_the_instance_sessions_quota() {
    for (profile, profile_path) in [("RootProfile", "/"), ("NestedProfile", "/app/")] {
        let router = router();
        ok(
            &router,
            profile,
            "CreateInstanceProfile",
            &[("InstanceProfileName", profile), ("Path", profile_path)],
        )
        .await;
        for (role, role_path) in [("FirstRole", "/"), ("SecondRole", "/eng/")] {
            ok(
                &router,
                role,
                "CreateRole",
                &[
                    ("RoleName", role),
                    ("Path", role_path),
                    ("AssumeRolePolicyDocument", TRUST_POLICY),
                ],
            )
            .await;
        }
        ok(
            &router,
            profile,
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", profile),
                ("RoleName", "FirstRole"),
            ],
        )
        .await;

        for role in ["SecondRole", "secondrole", "SECONDROLE"] {
            let case = format!("{profile} + {role}");
            let (status, body) = call(
                &router,
                "AddRoleToInstanceProfile",
                &[("InstanceProfileName", profile), ("RoleName", role)],
            )
            .await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected HTTP 409, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "LimitExceeded",
                "case {case:?}: expected LimitExceeded, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                "Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1",
                "case {case:?}: expected the quota IAM names for this limit, got {message:?}"
            );
        }

        // The refused adds changed nothing: the profile still holds exactly the role that
        // was added first.
        let body = ok(
            &router,
            profile,
            "GetInstanceProfile",
            &[("InstanceProfileName", profile)],
        )
        .await;
        let held: Vec<String> = crate::user_lifecycle::elements(&body, "RoleName");
        assert_eq!(
            held,
            vec!["FirstRole".to_string()],
            "case {profile:?}: expected the first role still held alone, got {held:?}"
        );
    }
}

/// RULE 3: the service-linked-role protection is about the ROLE, not about which record
/// resolved it — so every operation of this service that protects a service-linked role
/// refuses one addressed to a role a SERVICE keeps provisioned under its published name
/// with 400 `UnmodifiableEntity` and IAM's protected-role sentence, and the role is still
/// reported afterwards.
///
/// The rows span the operations that carry the protection and the published role names:
/// a service's plain role, one whose AWS-assigned label differs from its principal's, and
/// one suffixed with the resource type it scales. Source for the names:
/// `state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES`, the catalog
/// `tests/all/service_provisioned_roles.rs` pins against the services' own "Using
/// service-linked roles" pages.
#[tokio::test]
async fn changing_a_role_a_service_keeps_provisioned_is_refused_as_unmodifiable() {
    let published = [
        "AWSServiceRoleForECS",
        "AWSServiceRoleForComputeOptimizer",
        "AWSServiceRoleForApplicationAutoScaling_DynamoDBTable",
    ];

    for role_name in published {
        let router = router();
        let policy_arn = {
            let body = ok(
                &router,
                role_name,
                "CreatePolicy",
                &[
                    ("PolicyName", "Attachable"),
                    ("PolicyDocument", POLICY_DOCUMENT),
                ],
            )
            .await;
            element(&body, "Arn")
        };
        ok(
            &router,
            role_name,
            "CreateInstanceProfile",
            &[("InstanceProfileName", "Profile")],
        )
        .await;
        let boundary = policy_arn.clone();

        // Every operation of this service that refuses a change to a service-linked role,
        // with the parameters that address this role.
        let calls = vec![
            Call {
                action: "DeleteRole",
                parameters: vec![("RoleName", role_name)],
            },
            Call {
                action: "UpdateAssumeRolePolicy",
                parameters: vec![("RoleName", role_name), ("PolicyDocument", TRUST_POLICY)],
            },
            Call {
                action: "PutRolePolicy",
                parameters: vec![
                    ("RoleName", role_name),
                    ("PolicyName", "inline"),
                    ("PolicyDocument", POLICY_DOCUMENT),
                ],
            },
            Call {
                action: "DeleteRolePolicy",
                parameters: vec![("RoleName", role_name), ("PolicyName", "inline")],
            },
            Call {
                action: "AttachRolePolicy",
                parameters: vec![("RoleName", role_name), ("PolicyArn", policy_arn.as_str())],
            },
            Call {
                action: "DetachRolePolicy",
                parameters: vec![("RoleName", role_name), ("PolicyArn", policy_arn.as_str())],
            },
            Call {
                action: "AddRoleToInstanceProfile",
                parameters: vec![("InstanceProfileName", "Profile"), ("RoleName", role_name)],
            },
            Call {
                action: "RemoveRoleFromInstanceProfile",
                parameters: vec![("InstanceProfileName", "Profile"), ("RoleName", role_name)],
            },
            Call {
                action: "PutRolePermissionsBoundary",
                parameters: vec![
                    ("RoleName", role_name),
                    ("PermissionsBoundary", boundary.as_str()),
                ],
            },
            Call {
                action: "DeleteRolePermissionsBoundary",
                parameters: vec![("RoleName", role_name)],
            },
        ];

        for Call { action, parameters } in &calls {
            let case = format!("{action} on {role_name}");
            let (status, body) = call(&router, action, parameters).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "UnmodifiableEntity",
                "case {case:?}: expected UnmodifiableEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                protected(role_name),
                "case {case:?}: expected IAM's protected-role sentence, got {message:?}"
            );
        }

        // The role the service keeps provisioned is still reported, under the
        // service-linked path — which is what shows every refusal above came from the
        // protection rather than from the name resolving to nothing.
        let body = ok(&router, role_name, "GetRole", &[("RoleName", role_name)]).await;
        let path = element(&body, "Path");
        assert!(
            path.starts_with("/aws-service-role/"),
            "case {role_name:?}: expected the service-linked path, got {path:?}"
        );
        assert_eq!(
            element(&body, "RoleName"),
            role_name,
            "case {role_name:?}: expected the role still reported, got {body}"
        );
    }
}

/// RULE: a name NEITHER the account holds NOR a service keeps provisioned is still
/// "a resource entity that does not exist" on every one of those operations — the
/// protection above adds no role.
///
/// The rows span names close to a published one but outside the catalog: the prefix with
/// nothing after it, a published name one character short and one character long, and a
/// plain name nothing created.
#[tokio::test]
async fn changing_a_role_nothing_holds_is_still_not_found() {
    let router = router();
    let absent = [
        "AWSServiceRoleFor",
        "AWSServiceRoleForEC",
        "AWSServiceRoleForECSs",
        "absent-role",
    ];
    for role_name in absent {
        for (action, params) in [
            ("DeleteRole", vec![("RoleName", role_name)]),
            (
                "UpdateAssumeRolePolicy",
                vec![("RoleName", role_name), ("PolicyDocument", TRUST_POLICY)],
            ),
            (
                "DeleteRolePolicy",
                vec![("RoleName", role_name), ("PolicyName", "inline")],
            ),
            ("DeleteRolePermissionsBoundary", vec![("RoleName", role_name)]),
        ] {
            let case = format!("{action} on {role_name}");
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {code:?}: {body}"
            );
        }
    }
}

/// RULE: the validator that reads an IDENTITY or resource policy document reports a
/// document it cannot read as a policy as a syntax error and gives no diagnosis —
/// `Syntax errors in policy.` for text that does not parse AND for an object carrying no
/// `Statement` — while a role's TRUST policy, read by IAM's own trust-policy validator,
/// answers `Policy has no statements` for the second. Nothing is persisted either way.
///
/// The rows span the forms an object with no `Statement` takes: the empty object, one
/// carrying only `Version`, one carrying every other top-level element of the IAM JSON
/// policy grammar, and one naming the element in another case.
#[tokio::test]
async fn a_document_carrying_no_statement_is_a_syntax_error_for_an_identity_policy() {
    let documents = [
        ("an empty object", "{}"),
        ("only Version", r#"{"Version":"2012-10-17"}"#),
        (
            "every top-level element but Statement",
            r#"{"Version":"2012-10-17","Id":"a-policy"}"#,
        ),
        (
            "the element named in another case",
            r#"{"Version":"2012-10-17","statement":[]}"#,
        ),
    ];

    for (index, (what, document)) in documents.iter().enumerate() {
        let router = router();
        ok(
            &router,
            what,
            "CreateRole",
            &[
                ("RoleName", "DocRole"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        ok(&router, what, "CreateUser", &[("UserName", "doc-user")]).await;
        ok(&router, what, "CreateGroup", &[("GroupName", "DocGroup")]).await;
        let managed = {
            let body = ok(
                &router,
                what,
                "CreatePolicy",
                &[
                    ("PolicyName", "DocPolicy"),
                    ("PolicyDocument", POLICY_DOCUMENT),
                ],
            )
            .await;
            element(&body, "Arn")
        };
        let fresh = format!("Fresh{index}");

        // Each operation that takes a policy document, with the sentence its document
        // class is refused with: the identity and resource policy writers, then the two
        // trust-policy writers.
        let calls = vec![
            DocumentCall {
                call: Call {
                    action: "CreatePolicy",
                    parameters: vec![
                        ("PolicyName", fresh.as_str()),
                        ("PolicyDocument", document),
                    ],
                },
                expected: SYNTAX_ERRORS_IN_POLICY,
            },
            DocumentCall {
                call: Call {
                    action: "CreatePolicyVersion",
                    parameters: vec![
                        ("PolicyArn", managed.as_str()),
                        ("PolicyDocument", document),
                    ],
                },
                expected: SYNTAX_ERRORS_IN_POLICY,
            },
            DocumentCall {
                call: Call {
                    action: "PutRolePolicy",
                    parameters: vec![
                        ("RoleName", "DocRole"),
                        ("PolicyName", "inline"),
                        ("PolicyDocument", document),
                    ],
                },
                expected: SYNTAX_ERRORS_IN_POLICY,
            },
            DocumentCall {
                call: Call {
                    action: "PutUserPolicy",
                    parameters: vec![
                        ("UserName", "doc-user"),
                        ("PolicyName", "inline"),
                        ("PolicyDocument", document),
                    ],
                },
                expected: SYNTAX_ERRORS_IN_POLICY,
            },
            DocumentCall {
                call: Call {
                    action: "PutGroupPolicy",
                    parameters: vec![
                        ("GroupName", "DocGroup"),
                        ("PolicyName", "inline"),
                        ("PolicyDocument", document),
                    ],
                },
                expected: SYNTAX_ERRORS_IN_POLICY,
            },
            DocumentCall {
                call: Call {
                    action: "CreateRole",
                    parameters: vec![
                        ("RoleName", fresh.as_str()),
                        ("AssumeRolePolicyDocument", document),
                    ],
                },
                expected: POLICY_HAS_NO_STATEMENTS,
            },
            DocumentCall {
                call: Call {
                    action: "UpdateAssumeRolePolicy",
                    parameters: vec![("RoleName", "DocRole"), ("PolicyDocument", document)],
                },
                expected: POLICY_HAS_NO_STATEMENTS,
            },
        ];

        for DocumentCall {
            call: Call { action, parameters },
            expected,
        } in &calls
        {
            let case = format!("{action} with {what}");
            let (status, body) = call(&router, action, parameters).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "MalformedPolicyDocument",
                "case {case:?}: expected MalformedPolicyDocument, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, *expected,
                "case {case:?}: expected the fixed sentence of this document class, got \
                 {message:?}"
            );
        }

        // Nothing the refusals named was written: the policy and the role a create would
        // have minted are absent, and the role whose trust policy an update was refused
        // still carries the document it was created with.
        let (status, body) = call(&router, "GetRole", &[("RoleName", fresh.as_str())]).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused create must persist no role, got {status}: {body}"
        );
        let body = ok(&router, what, "GetRole", &[("RoleName", "DocRole")]).await;
        assert_eq!(
            crate::user_lifecycle::document_element(&body, "AssumeRolePolicyDocument"),
            TRUST_POLICY,
            "case {what:?}: a refused update must leave the held document alone, got {body}"
        );
    }
}

/// The record of one role session, expiring `in_seconds` from the epoch.
fn session_record(in_seconds: i64) -> RoleSessionRecord {
    RoleSessionRecord {
        session_arn: format!("arn:aws:sts::{}:assumed-role/Reader/probe", account_id()),
        role_arn: format!("arn:aws:iam::{}:role/Reader", account_id()),
        expires: ::aws_smithy_types::DateTime::from_secs(in_seconds),
        scope: Default::default(),
    }
}

/// RULE: the record of which role a temporary credential speaks for lives exactly as long
/// as that credential — it answers for the key while the session is live, answers nothing
/// once the clock has passed its expiry, and the map holding it is bounded, giving up the
/// record CLOSEST TO EXPIRING rather than the one being written when the bound is reached.
///
/// The rows span the instants a record is read at (before, at and after its expiry) and
/// the two ways the map sheds a record (the clock, and the ceiling), which is what keeps a
/// request-driven store from growing without end.
#[test]
fn a_role_session_record_lives_as_long_as_its_credential_and_the_map_is_bounded() {
    let mut account = IamState::default();
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    account.record_role_session("ASIAPROBE", session_record(3_600), minted);

    // (the instant the record is read at, whether it answers)
    for (read_at, answers) in [(0, true), (3_599, true), (3_600, false), (7_200, false)] {
        let read = account.role_session("ASIAPROBE", ::aws_smithy_types::DateTime::from_secs(
            read_at,
        ));
        assert_eq!(
            read.is_some(),
            answers,
            "case read at {read_at}s: expected answered == {answers}, got {read:?}"
        );
    }

    // A write past the record's expiry sheds it: the credential was withdrawn at that
    // instant whether or not anybody called.
    account.record_role_session(
        "ASIAOTHER",
        session_record(10_800),
        ::aws_smithy_types::DateTime::from_secs(7_200),
    );
    assert!(
        !account.role_sessions.contains_key("ASIAPROBE"),
        "an expired record must be shed by the next write, got {:?}",
        account.role_sessions.keys().collect::<Vec<_>>()
    );

    // At the ceiling the record given up is the one closest to expiring, and the session
    // being recorded is always recorded: a session with no record is one the authorizer
    // does not bound.
    let mut account = IamState::default();
    for index in 0..state::MAX_ROLE_SESSIONS {
        // Every fixture session is live at the instant below, and they expire in the
        // order they were recorded.
        account.record_role_session(
            &format!("ASIALIVE{index}"),
            session_record(100_000 + index as i64),
            minted,
        );
    }
    assert_eq!(
        account.role_sessions.len(),
        state::MAX_ROLE_SESSIONS,
        "expected the map filled to its ceiling, got {}",
        account.role_sessions.len()
    );
    account.record_role_session("ASIAFRESH", session_record(200_000), minted);
    assert_eq!(
        account.role_sessions.len(),
        state::MAX_ROLE_SESSIONS,
        "expected the ceiling to hold, got {}",
        account.role_sessions.len()
    );
    assert!(
        account
            .role_session("ASIAFRESH", minted)
            .is_some(),
        "the session being recorded must be the one kept"
    );
    assert!(
        !account.role_sessions.contains_key("ASIALIVE0"),
        "expected the record closest to expiring to be the one given up"
    );
    assert!(
        account
            .role_session(
                &format!("ASIALIVE{}", state::MAX_ROLE_SESSIONS - 1),
                minted
            )
            .is_some(),
        "a record further from expiring must be kept"
    );
}
