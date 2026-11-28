// SPDX-License-Identifier: CC0-1.0

//! What the credentials this service VENDS are scoped to: the session policies the
//! credential-vending exchange attaches to the role session it hands back, stated once here.
//!
//! The enhanced flow (`GetCredentialsForIdentity`) performs the assume itself — "it will be
//! passed through to Security Token Service with the appropriate role for the token" (that
//! operation's own doc) — so it is the flow that decides what the session it answers with is
//! scoped by. The basic flow leaves the assume to the caller (`GetOpenIdToken` and then
//! `sts:AssumeRoleWithWebIdentity`), which passes whatever session policy that caller passes
//! and nothing of this service's, so a session minted that way is capped by the role's own
//! documents alone.
//!
//! The exchange this service scopes is the UNAUTHENTICATED one — the guest exchange, which
//! presents no login at all: the credentials it hands an anonymous caller reach only the
//! services an identity pool limits a guest to, so an action outside that scope is refused by
//! the service that receives the call even where the unauthenticated role's own documents
//! allow it, while an action inside it is left to those documents exactly as before. An
//! AUTHENTICATED exchange stands for a user an identity provider vouched for and is scoped by
//! nothing of this service's.
//!
//! A session policy CAPS and grants nothing: "the resulting session's permissions are the
//! intersection of the role's identity-based policy and the session policies"
//! (`AssumeRoleRequest$PolicyArns`, the STS model). The scope travels with the session as the
//! policies its mint passed ([`SessionPolicies`]), which is where the gate of every service
//! handed the vended credential reads it, so none of them can cap the session differently —
//! and that is also why a guest's reach is stated as a SCOPE rather than written into any one
//! service's gate.

/// The session policies ONE mint passed, as the token service records them beside the session
/// and every service deciding a request signed with that session reads them.
pub use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::SessionPolicies;

/// The `Version` an IAM policy document this service writes is dated, the one every policy
/// language element AWS publishes belongs to.
const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The services a GUEST session may reach, each named by the ACTION NAMESPACE that service
/// publishes its actions under — `s3` for Amazon S3, `execute-api` for Amazon API Gateway —
/// so the scope allows every action of the service and nothing of a service absent from the
/// list.
///
/// SOURCE: the Amazon Cognito Developer Guide's "Authentication flow" page, whose
/// enhanced-flow note names the services the credentials of an unauthenticated identity are
/// limited to. Neither IAM nor this service's model carries the list — it is the content of a
/// policy rather than part of any API — so it is written out here, in one place, and read as
/// a set.
///
/// Sorted, which is also the order the document's `Action` element lists them in: what the
/// scope says does not depend on the order it is written in, and a document whose actions
/// come out in one order every time is one two reads of a refusal's authorization detail
/// cannot differ over.
const GUEST_SCOPE_SERVICE_NAMESPACES: &[&str] = &[
    "amplify",
    "appsync",
    "cognito-identity",
    "cognito-idp",
    "cognito-sync",
    "comprehend",
    "dynamodb",
    "execute-api",
    "firehose",
    "geo",
    "iot",
    "kinesis",
    "lambda",
    "lex",
    "machinelearning",
    "mobileanalytics",
    "mobiletargeting",
    "personalize",
    "polly",
    "rekognition",
    "s3",
    "sagemaker",
    "ses",
    "sns",
    "sqs",
    "states",
    "textract",
    "transcribe",
    "translate",
];

/// The AWS KMS actions a GUEST session may reach: the cryptographic operations a client
/// encrypting its own data under a customer managed key performs, and the key read those
/// operations need — and NOT the key administration a guest has no business with, which is
/// why this service is named by action rather than by namespace.
///
/// SOURCE: the Amazon Cognito Developer Guide's "Authentication flow" page, as
/// [`GUEST_SCOPE_SERVICE_NAMESPACES`]: the guest scope covers a subset of AWS KMS rather than
/// the whole of it.
///
/// Sorted, for the reason that list is.
const GUEST_SCOPE_KMS_ACTIONS: &[&str] = &[
    "kms:Decrypt",
    "kms:DescribeKey",
    "kms:Encrypt",
    "kms:GenerateDataKey",
    "kms:GenerateDataKeyWithoutPlaintext",
    "kms:ReEncryptFrom",
    "kms:ReEncryptTo",
];

/// The session policies the vend of ONE exchange attaches to the session it mints:
/// [`guest_scope_document`] for an exchange that presented NO login — the guest exchange —
/// and none at all for an authenticated one.
///
/// `unauthenticated` is whether the exchange presented no login, which is what makes it the
/// guest flow ([`crate::state::authentication_methods`] names the same exchange
/// `unauthenticated` to the assumed role's trust policy).
///
/// The scope carries no `none_means_no_permissions`: a session of a role keeps the role's own
/// permissions and these documents only cap them, unlike a federated-user session, which has
/// no permissions but the ones its session policies grant.
#[must_use]
pub fn vended_session_policies(unauthenticated: bool) -> SessionPolicies {
    if !unauthenticated {
        return SessionPolicies::default();
    }
    SessionPolicies {
        inline: Some(guest_scope_document()),
        ..SessionPolicies::default()
    }
}

/// The document the guest scope is: one `Allow` of every action of each service
/// [`GUEST_SCOPE_SERVICE_NAMESPACES`] names, and one `Allow` of each action
/// [`GUEST_SCOPE_KMS_ACTIONS`] names, on every resource.
///
/// Two statements because they are two grants — a whole service each, and a chosen set of one
/// service's actions — and a scope's statements are read together as what it allows, so what
/// the document says is their union. `Resource` is `*` throughout: the scope caps which
/// ACTIONS a guest may ask for and leaves which resources it may ask them on to the
/// unauthenticated role's own documents and to each service's own policies.
#[must_use]
fn guest_scope_document() -> String {
    let services: Vec<String> = GUEST_SCOPE_SERVICE_NAMESPACES
        .iter()
        .map(|namespace| format!("{namespace}:*"))
        .collect();
    ::serde_json::json!({
        "Version": POLICY_LANGUAGE_VERSION,
        "Statement": [
            {"Effect": "Allow", "Action": services, "Resource": "*"},
            {"Effect": "Allow", "Action": GUEST_SCOPE_KMS_ACTIONS, "Resource": "*"},
        ],
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    //! What the scope SAYS is asserted end to end through the services that decide a request
    //! signed with a vended credential (`core/router/tests/all/`); these cover the document
    //! this module writes and the one choice it makes.

    use super::*;

    #[test]
    fn a_guest_exchange_is_scoped_and_an_authenticated_one_is_not() {
        let guest = vended_session_policies(true);
        assert!(
            guest.inline.is_some(),
            "a guest exchange must carry the scope, got {guest:?}"
        );
        assert!(
            guest.managed_arns.is_empty() && guest.scope_downs.is_empty(),
            "the scope is the one document the vend attaches, got {guest:?}"
        );
        assert!(
            !guest.none_means_no_permissions,
            "a session of a role keeps the role's permissions, got {guest:?}"
        );
        assert_eq!(
            vended_session_policies(false),
            SessionPolicies::default(),
            "an authenticated exchange must be scoped by nothing of this service's"
        );
    }

    #[test]
    fn the_scope_names_every_service_it_carries_and_no_other() {
        let document: ::serde_json::Value = ::serde_json::from_str(&guest_scope_document())
            .unwrap_or_else(|why| panic!("the scope must be a JSON policy document: {why}"));
        let stated: Vec<String> = document["Statement"]
            .as_array()
            .unwrap_or_else(|| panic!("the scope states statements, got {document}"))
            .iter()
            .flat_map(|statement| {
                statement["Action"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|action| action.as_str().map(ToString::to_string))
            })
            .collect();
        for namespace in GUEST_SCOPE_SERVICE_NAMESPACES {
            let expected = format!("{namespace}:*");
            assert!(
                stated.contains(&expected),
                "the scope must allow {expected}, got {stated:?}"
            );
        }
        for action in GUEST_SCOPE_KMS_ACTIONS {
            assert!(
                stated.contains(&(*action).to_string()),
                "the scope must allow {action}, got {stated:?}"
            );
        }
        // The two namespaces the recorded guest refusals rest on: a scope naming either of
        // them would stop capping what AWS caps.
        for namespace in ["iam", "kms"] {
            let unexpected = format!("{namespace}:*");
            assert!(
                !stated.contains(&unexpected),
                "the scope must not allow {unexpected}, got {stated:?}"
            );
        }
    }
}
