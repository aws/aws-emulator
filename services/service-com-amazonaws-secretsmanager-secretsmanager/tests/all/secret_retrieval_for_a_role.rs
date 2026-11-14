// SPDX-License-Identifier: CC0-1.0

//! The retrieval of a secret this service serves ANOTHER service on a ROLE's behalf
//! (`state::secret_retrieval`), driven over this crate's own wire with the IAM and STS
//! services mounted beside it on ONE shared engine — so the secret, the policy attached to
//! it and the documents in force on the role are the ones those services hold.
//!
//! RULE: a retrieval made for a role is decided exactly as a signed `GetSecretValue` of that
//! role's own would be — `secretsmanager:GetSecretValue` on the secret's ARN, over the
//! documents in force on the role and the secret's OWN resource-based policy, in IAM's
//! evaluation order: an explicit `Deny` in either document wins, a grant in the secret's own
//! policy stands on its own, and a request no readable statement allows falls to IAM's
//! default deny — and a retrieval nothing refuses is served only while there is a value to
//! serve: a Region that holds no such secret, and a secret scheduled for deletion, carry
//! this service's own sentence about it.
//!
//! The rows span what the rule generalises over: which KIND of document decides (an
//! identity-based one, the secret's own resource-based one, neither), the effect it states
//! (`Allow`, `Deny`), whether the account holds the secret and whether it is scheduled for
//! deletion, and whether the retrieval names a principal at all.
//!
//! The refusal's own sentence is read back from the verdict, and the "not found" sentences
//! are this service's published ones, so a wrong wording cannot make a test pass.

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::secret_retrieval::{
    serve_secret_retrieval, RetrievalPrincipal, RetrievalRefusal,
};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    MARKED_FOR_DELETION_MESSAGE, SECRET_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_based_authorization::{
    call_as, given_role_policy, given_session, role_arn, router_with_identity, session_arn,
    ACCOUNT_KEY_ID,
};

/// The Region every case addresses, which is the Region the secret lives in.
const REGION: &str = "us-west-2";

/// The action a retrieval takes, fully qualified as IAM publishes it.
const ACTION: &str = "secretsmanager:GetSecretValue";

/// Which document decides the retrieval in one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Document {
    /// Nothing is in force on the role, and the secret carries no policy.
    None,
    /// An inline policy of the ROLE states the effect.
    Identity(&'static str),
    /// The SECRET's own resource-based policy states the effect, naming the role.
    Resource(&'static str),
    /// The secret's policy denies while an identity policy allows: the deny wins.
    ResourceDenyOverIdentityAllow,
}

/// What a row expects the retrieval to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The retrieval is served.
    Served,
    /// It is refused, and the refusal states this clause.
    NotAuthorized(&'static str),
    /// There is no value to serve, and this is the sentence that says why.
    NotFound(&'static str),
}

/// One row of the table.
struct Case {
    name: &'static str,
    document: Document,
    /// Whether the secret is created at all.
    held: bool,
    /// Whether the secret is scheduled for deletion before the retrieval.
    deleted: bool,
    /// Whether the retrieval names the role at all.
    names_principal: bool,
    expected: Expected,
}

/// An identity-based policy with one statement: `effect` on the retrieval's action, scoped to
/// `resource`.
fn identity_document(effect: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": [ACTION], "Resource": [resource] }],
    })
}

/// A resource-based policy with one statement: `effect` on the retrieval's action for the
/// role the retrieval is made as — the shape that grants a principal with no identity-based
/// grant at all.
fn resource_document(effect: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "AWS": role_arn() },
            "Action": [ACTION],
            "Resource": [resource],
        }],
    })
}

/// Create the secret `name` through this service's own API and answer its ARN.
async fn create_secret(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let (status, body) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "CreateSecret",
        &json!({ "Name": name, "SecretString": "the-stored-credential" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateSecret to answer 200, got {status} with {body}"
    );
    body["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {body}"))
        .to_string()
}

const ROWS: &[Case] = &[
    Case {
        name: "an identity policy allowing the action on the secret",
        document: Document::Identity("Allow"),
        held: true,
        deleted: false,
        names_principal: true,
        expected: Expected::Served,
    },
    Case {
        name: "no document in force at all",
        document: Document::None,
        held: true,
        deleted: false,
        names_principal: true,
        expected: Expected::NotAuthorized(
            "because no identity-based policy allows the secretsmanager:GetSecretValue action",
        ),
    },
    Case {
        name: "an identity policy denying the action",
        document: Document::Identity("Deny"),
        held: true,
        deleted: false,
        names_principal: true,
        expected: Expected::NotAuthorized("with an explicit deny in an identity-based policy"),
    },
    Case {
        name: "the secret's own policy naming the role, with no identity grant",
        document: Document::Resource("Allow"),
        held: true,
        deleted: false,
        names_principal: true,
        expected: Expected::Served,
    },
    Case {
        name: "the secret's own policy denying the role beside an identity allow",
        document: Document::ResourceDenyOverIdentityAllow,
        held: true,
        deleted: false,
        names_principal: true,
        expected: Expected::NotAuthorized("with an explicit deny in a resource-based policy"),
    },
    Case {
        name: "a secret scheduled for deletion",
        document: Document::Identity("Allow"),
        held: true,
        deleted: true,
        names_principal: true,
        expected: Expected::NotFound(MARKED_FOR_DELETION_MESSAGE),
    },
    Case {
        name: "a secret this Region does not hold",
        document: Document::Identity("Allow"),
        held: false,
        deleted: false,
        names_principal: true,
        expected: Expected::NotFound(SECRET_NOT_FOUND_MESSAGE),
    },
    Case {
        name: "a retrieval that names no principal",
        document: Document::None,
        held: true,
        deleted: false,
        names_principal: false,
        expected: Expected::Served,
    },
];

/// RULE: see the module docs — the retrieval a compute service makes for a role is decided by
/// the documents in force on that role and by the secret's own policy, and served only while
/// there is a value to serve.
#[tokio::test]
async fn a_retrieval_made_for_a_role_is_decided_by_the_documents_in_force() {
    for (index, case) in ROWS.iter().enumerate() {
        let name = case.name;
        // A fresh engine per row, so one row's policies cannot decide another's.
        let (router, engine) = router_with_identity();
        given_session(&engine);

        let secret_name = format!("retrieved-{index}");
        let secret_arn = if case.held {
            create_secret(&router, &secret_name, name).await
        } else {
            // The ARN a secret of that name would carry, which this Region holds nothing
            // under.
            format!(
                "arn:aws:secretsmanager:{REGION}:{}:secret:{secret_name}-AbCdEf",
                ::engine::emulator_account_id()
            )
        };

        match case.document {
            Document::None => {}
            Document::Identity(effect) => {
                given_role_policy(&engine, Some(&identity_document(effect, &secret_arn)));
            }
            Document::Resource(effect) => {
                let (status, body) = call_as(
                    &router,
                    ACCOUNT_KEY_ID,
                    "PutResourcePolicy",
                    &json!({
                        "SecretId": secret_arn,
                        "ResourcePolicy": resource_document(effect, &secret_arn).to_string(),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected PutResourcePolicy to answer 200, got {status} \
                     with {body}"
                );
            }
            Document::ResourceDenyOverIdentityAllow => {
                given_role_policy(&engine, Some(&identity_document("Allow", &secret_arn)));
                let (status, body) = call_as(
                    &router,
                    ACCOUNT_KEY_ID,
                    "PutResourcePolicy",
                    &json!({
                        "SecretId": secret_arn,
                        "ResourcePolicy": resource_document("Deny", &secret_arn).to_string(),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected PutResourcePolicy to answer 200, got {status} \
                     with {body}"
                );
            }
        }

        if case.deleted {
            let (status, body) = call_as(
                &router,
                ACCOUNT_KEY_ID,
                "DeleteSecret",
                &json!({ "SecretId": secret_arn }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected DeleteSecret to answer 200, got {status} with {body}"
            );
        }

        let session = session_arn();
        let role = role_arn();
        let principal = case.names_principal.then(|| RetrievalPrincipal {
            role_arn: &role,
            session_arn: Some(&session),
        });
        let verdict = serve_secret_retrieval(&*engine, REGION, &secret_arn, principal);

        match case.expected {
            Expected::Served => assert!(
                verdict.is_ok(),
                "case {name:?}: expected the retrieval to be served, got {verdict:?}"
            ),
            Expected::NotAuthorized(clause) => {
                let Err(RetrievalRefusal::NotAuthorized { action, message }) = &verdict else {
                    panic!("case {name:?}: expected an authorization refusal, got {verdict:?}");
                };
                assert_eq!(
                    action, ACTION,
                    "case {name:?}: expected the refusal to name {ACTION}, got {action}"
                );
                // The sentence names the SESSION the retrieval was made in, the action and
                // the secret, and the rule that withheld the grant.
                let expected = format!(
                    "User: {session} is not authorized to perform: {ACTION} on resource: \
                     {secret_arn} {clause}"
                );
                assert_eq!(
                    message, &expected,
                    "case {name:?}: expected {expected:?}, got {message:?}"
                );
            }
            Expected::NotFound(sentence) => {
                let Err(RetrievalRefusal::NotFound { message }) = &verdict else {
                    panic!("case {name:?}: expected a not-found refusal, got {verdict:?}");
                };
                assert_eq!(
                    message, sentence,
                    "case {name:?}: expected {sentence:?}, got {message:?}"
                );
            }
        }
    }
}
