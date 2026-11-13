// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH principal a clause over the calling identity is decided
//! against, driven over the real wire path (an awsJson1_1 request through the multi-service
//! router into the mounted service), so what is asserted is what a client holding those
//! credentials sees.
//!
//! THE RULE: the global condition keys naming the caller are answered with the IDENTITY
//! whose permissions are in force on the request — "for IAM roles, the request context
//! returns the ARN of the role, not the ARN of the user that assumed the role"
//! (`aws:PrincipalArn`, IAM's global condition keys) — so a request signed by an
//! assumed-role session answers them with the ROLE's ARN and with that role's account, not
//! with the session ARN STS reports for the credential. A statement locked to one role by
//! such a clause therefore applies to every session of that role, wherever the statement
//! lives: in the caller's identity-based policy or in the addressed secret's own
//! resource-based one, and whichever effect it states.
//!
//! The table spans what the rule generalises over: the document the clause lives in
//! (identity-based, resource-based), the effect it qualifies (`Allow`, `Deny`), the operator
//! (`StringEquals`, `StringNotEquals`, `ArnEquals`, `ArnLike`, `StringLike`, and
//! `StringEquals` over the principal's ACCOUNT), the ARN form the clause names (the role's,
//! the session's, another role's, a wildcard over the account's roles) and the operation the
//! decision is taken for (a value read and a metadata read).
//!
//! `tests/all/identity_based_authorization.rs` owns the harness these cases reuse — the
//! router over one shared engine, the seeded role and session, and the policy writes.

use serde_json::{json, Value};

use crate::identity_based_authorization::{
    call_as, error_code, error_message, given_role_policy, given_secret, given_session, role_arn,
    router_with_identity, session_arn, ACCESS_DENIED, ACCESS_DENIED_STATUS, SECRET_VALUE,
    SESSION_KEY_ID,
};

/// The ARN of a role of this account the session is NOT a session of, so a clause naming it
/// names another principal.
fn other_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/another-reader",
        ::engine::emulator_account_id()
    )
}

/// A wildcard over every role of this account, which is the form a clause scoped to the
/// account's roles rather than to one of them takes.
fn any_role_of_this_account() -> String {
    format!("arn:aws:iam::{}:role/*", ::engine::emulator_account_id())
}

/// An identity-based policy allowing `action` on every resource, qualified by `condition`.
fn allow_if(action: &str, condition: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": action,
            "Resource": "*",
            "Condition": condition,
        }]
    })
}

/// An identity-based policy allowing every action of this service, unconditioned — the grant
/// a case puts in force when what it is testing is a resource-based clause.
fn allow_everything() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "secretsmanager:*", "Resource": "*" }]
    })
}

/// A resource-based policy stating `effect` on `action` for every principal, qualified by
/// `condition`.
fn resource_policy_if(effect: &str, action: &str, condition: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "LockedToOneIdentity",
            "Effect": effect,
            "Principal": "*",
            "Action": action,
            "Resource": "*",
            "Condition": condition,
        }]
    })
    .to_string()
}

/// Which side of the decision a row's clause lives on.
#[derive(Debug, Clone, Copy)]
enum Document {
    /// The caller's own identity-based policy carries the conditioned statement.
    Identity,
    /// The addressed secret's resource-based policy carries it, and the caller holds an
    /// unconditioned grant of its own — so what the row decides is the resource-based clause.
    Resource,
}

/// One case: where the clause lives, what it says, and whether the session's request is
/// served.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    document: Document,
    /// The effect the clause qualifies.
    effect: &'static str,
    /// The operation the decision is taken for.
    operation: &'static str,
    /// The `Condition` block, built once the seeded role's ARN is known.
    condition: Value,
    /// Whether the session's call must be SERVED.
    served: bool,
}

/// RULE: a clause over the caller's principal ARN or account is decided against the role an
/// assumed-role session is a session of, in an identity-based document and in a secret's own
/// resource-based one alike.
#[tokio::test]
async fn a_clause_over_the_caller_is_decided_against_the_role_behind_the_session() {
    let role = role_arn();
    let session = session_arn();
    let other = other_role_arn();
    let any_role = any_role_of_this_account();
    let account = ::engine::emulator_account_id();
    let cases: Vec<Case> = vec![
        Case {
            name: "an identity grant locked to the role's ARN",
            document: Document::Identity,
            effect: "Allow",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:PrincipalArn": role.clone()}}),
            served: true,
        },
        Case {
            name: "an identity grant locked to the SESSION's ARN, which the key does not answer",
            document: Document::Identity,
            effect: "Allow",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:PrincipalArn": session.clone()}}),
            served: false,
        },
        Case {
            name: "an identity grant locked to the role's ARN under the ARN operator",
            document: Document::Identity,
            effect: "Allow",
            operation: "DescribeSecret",
            condition: json!({"ArnEquals": {"aws:PrincipalArn": role.clone()}}),
            served: true,
        },
        Case {
            name: "an identity grant over every role of this account",
            document: Document::Identity,
            effect: "Allow",
            operation: "GetSecretValue",
            condition: json!({"ArnLike": {"aws:PrincipalArn": any_role.clone()}}),
            served: true,
        },
        Case {
            name: "an identity grant locked to another role of this account",
            document: Document::Identity,
            effect: "Allow",
            operation: "GetSecretValue",
            condition: json!({"ArnEquals": {"aws:PrincipalArn": other.clone()}}),
            served: false,
        },
        Case {
            name: "an identity grant locked to the principal's account",
            document: Document::Identity,
            effect: "Allow",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:PrincipalAccount": account.to_string()}}),
            served: true,
        },
        Case {
            name: "a resource-policy deny of everything but the role's ARN",
            document: Document::Resource,
            effect: "Deny",
            operation: "GetSecretValue",
            condition: json!({"StringNotEquals": {"aws:PrincipalArn": role.clone()}}),
            served: true,
        },
        Case {
            name: "a resource-policy deny of everything but the SESSION's ARN",
            document: Document::Resource,
            effect: "Deny",
            operation: "GetSecretValue",
            condition: json!({"StringNotEquals": {"aws:PrincipalArn": session.clone()}}),
            served: false,
        },
        Case {
            name: "a resource-policy deny of everything but another role, on a metadata read",
            document: Document::Resource,
            effect: "Deny",
            operation: "DescribeSecret",
            condition: json!({"StringNotEquals": {"aws:PrincipalArn": other.clone()}}),
            served: false,
        },
        Case {
            name: "a resource-policy deny naming the role's ARN itself",
            document: Document::Resource,
            effect: "Deny",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:PrincipalArn": role.clone()}}),
            served: false,
        },
        Case {
            name: "a resource-policy deny over every role of this account",
            document: Document::Resource,
            effect: "Deny",
            operation: "GetSecretValue",
            condition: json!({"StringLike": {"aws:PrincipalArn": any_role.clone()}}),
            served: false,
        },
    ];

    for (row, case) in cases.into_iter().enumerate() {
        let Case {
            name,
            document,
            effect,
            operation,
            condition,
            served,
        } = case;
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // Named by the row's position: a secret's name takes ASCII letters, numbers and
        // `/_+=.@-` only, which a case's prose does not.
        let secret = format!("principal-clause-{row}");
        let arn = given_secret(&router, &secret).await;

        let action = format!("secretsmanager:{operation}");
        match document {
            Document::Identity => {
                given_role_policy(&engine, Some(&allow_if(&action, condition.clone())));
            }
            Document::Resource => {
                given_role_policy(&engine, Some(&allow_everything()));
                let attached = resource_policy_if(effect, &action, condition.clone());
                let (status, answered) = call_as(
                    &router,
                    SESSION_KEY_ID,
                    "PutResourcePolicy",
                    &json!({"SecretId": &arn, "ResourcePolicy": attached}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected the policy to attach, got {status} {answered}"
                );
            }
        }

        let (status, answered) =
            call_as(&router, SESSION_KEY_ID, operation, &json!({"SecretId": &arn})).await;
        if served {
            assert_eq!(
                status, 200,
                "case {name:?}: expected the session's {operation} to be served, got {status} \
                 {answered}"
            );
            if operation == "GetSecretValue" {
                assert_eq!(
                    answered.get("SecretString").and_then(Value::as_str),
                    Some(SECRET_VALUE),
                    "case {name:?}: expected the stored value, got {answered}"
                );
            }
        } else {
            assert_eq!(
                status, ACCESS_DENIED_STATUS,
                "case {name:?}: expected the session's {operation} to be refused, got {status} \
                 {answered}"
            );
            assert_eq!(
                error_code(&answered),
                ACCESS_DENIED,
                "case {name:?}: expected {ACCESS_DENIED}, got {answered}"
            );
            assert!(
                error_message(&answered).contains(&session),
                "case {name:?}: expected the refusal to name the session the credential speaks \
                 for, got {:?}",
                error_message(&answered)
            );
        }
    }
}
