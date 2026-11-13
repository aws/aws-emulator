// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal a CONTROL-PLANE request speaks for perform the action it
//! asks for on the resource it addresses.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at what the
//! request asks for: a principal whose identity-based policies do not allow the action — or
//! that an explicit `Deny` covers — is refused rather than served, and the call performs
//! nothing. Nothing is minted, no configuration is replaced, no status moves and nothing is
//! read back. This model declares no error shape for that decision (no operation of this
//! service lists an access error, `smithy_query(query="digest")`), because the refusal is the
//! AWS COMMON `AccessDeniedException` answered by the layer in front of a service's own code:
//! it travels here through [`crate::scaffolded::Scaffolded::AccessDenied`], which renders
//! HTTP 403 with the error code `AccessDeniedException`.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read by the ONE reading
//!    of a request's caller this crate has ([`super::invoke_authorization::caller`]): the
//!    session STS minted that key for, else the IAM user that owns the long-term key, else the
//!    account's own published credential. It is the same record `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for the
//!    credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors, where the collection rule itself lives — so a document
//!    `iam:PutRolePolicy` wrote takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision and its wording** — [`::service_common::iam_authorization`], the tree's
//!    ONE evaluation of policy documents against `(action, resource)` and the one sentence
//!    every service in this tree refuses an unauthorized request with, so two services handed
//!    the same documents answer alike and word the refusal alike.
//!
//! The resource is the ARN the REQUEST addresses, built from the request alone, so the
//! decision does not depend on whether that resource exists — which is also what keeps a
//! refused call from telling an unauthorized caller what the account holds. A request that
//! addresses no single resource of its own is judged against
//! [`super::invoke_authorization::ANY_RESOURCE`], as IAM writes
//! the resource of such a request.
//!
//! This is the identity-based decision ALONE, which is the whole decision for a control-plane
//! call: a resource-based policy of this service grants an INVOCATION of a function
//! ([`super::invoke_authorization`], which evaluates both halves and is the gate those
//! operations take), and no other resource of this service carries one.
//!
//! ## The callers this gate does not bound
//!
//! Exactly the ones the invocation gate does not bound, for the reasons stated there: the
//! account's own published credential (the position AWS account root credentials are in,
//! which every in-process call another emulated service makes is signed with), a principal
//! holding an attachment this emulator cannot read (an AWS-managed policy, whose document the
//! IAM store deliberately does not hold — refusing on a grant this emulator cannot see would
//! deny access AWS permits), and an engine carrying no STS or IAM store, where no session and
//! no role can exist. An explicit `Deny` in a document that CAN be read still refuses every
//! one of them, and a permissions boundary the store DOES hold still caps the grant.
//!
//! LOCK ORDER: the STS registry's lock and then the IAM store's are taken, each in turn, by
//! the caller reading above — and BEFORE this service's own store lock, which is why every
//! handler takes this decision before it locks its own store. Neither of those services reads
//! this one's state.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::invoke_authorization::{caller, Caller, ACTION_PREFIX};
use crate::scaffolded::AccessDeniedException;

/// Whether IAM lets this request perform `operation` on `resource`.
///
/// `operation` is the operation's own name ([`smithy_server_runtime::Operation::NAME`]),
/// which names the IAM action `lambda:<OperationName>` the IAM Service Authorization
/// Reference publishes for it ("Actions defined by AWS Lambda"); `resource` is the ARN the
/// request ADDRESSES, or [`super::invoke_authorization::ANY_RESOURCE`] for a request that
/// addresses no single resource.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see the
/// module docs); the refusal otherwise, which each operation returns through
/// [`crate::scaffolded::Scaffolded::AccessDenied`]. Call it BEFORE this service's store is
/// locked and before anything is resolved, so a refused call changes nothing and tells an
/// unauthorized caller nothing about what the account holds.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation may take this decision
/// too.
pub fn authorize_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    authorize_action(&caller(engine, ctx), &action, resource)
}

/// The decision itself, over a principal already read: separated from the reading above so it
/// is a pure function of the documents, the action and the resource.
fn authorize_action(
    caller: &Caller,
    action: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let refusal = |reason: &str| {
        AccessDeniedException::new(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            reason,
        ))
    };
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element is matched against; an identity-based document states no such
    // element, so the decision rests on the action and the resource.
    let request = iam::Request::new(action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // Taken FIRST: an explicit `Deny` in a document that CAN be read refuses the request
    // whatever else the principal carries — including a grant this emulator cannot read, which
    // only lifts the DEFAULT deny of a request no readable statement allows.
    if identity_said == Decision::Denied {
        return Err(refusal(iam::IDENTITY_POLICY_EXPLICIT_DENY));
    }
    if !(iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Err(refusal(&iam::no_identity_policy_allows(action)));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names it;
            // a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(action),
        };
        return Err(refusal(&reason));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A principal holding `documents` and nothing else — no boundary, not the account's own
    /// credential, no unreadable attachment.
    fn principal(documents: &[&str]) -> Caller {
        Caller {
            arn: "arn:aws:sts::000000000000:assumed-role/caller/session".to_string(),
            principals: vec!["arn:aws:sts::000000000000:assumed-role/caller/session".to_string()],
            documents: documents.iter().map(|held| (*held).to_string()).collect(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
        }
    }

    /// RULE: an explicit `Deny` about the request WINS over every `Allow` beside it, whatever
    /// width either is written at and in whichever order they are written — and the sentence
    /// names that document as the rule that refused.
    #[test]
    fn an_explicit_deny_refuses_whatever_a_grant_beside_it_says() {
        let action = "lambda:UpdateCapacityProvider";
        let resource = "arn:aws:lambda:us-west-2:000000000000:capacity-provider:scale";
        // (case, the documents in force, whether the request is permitted)
        let rows: &[(&str, Vec<String>, bool)] = &[
            (
                "a wildcard allow alone",
                vec![r#"{"Statement":[{"Effect":"Allow","Action":"lambda:*","Resource":"*"}]}"#
                    .to_string()],
                true,
            ),
            (
                "a wildcard allow with the action denied after it",
                vec![format!(
                    r#"{{"Statement":[{{"Effect":"Allow","Action":"lambda:*","Resource":"*"}},
                        {{"Effect":"Deny","Action":"{action}","Resource":"*"}}]}}"#
                )],
                false,
            ),
            (
                "the action denied BEFORE a wildcard allow",
                vec![format!(
                    r#"{{"Statement":[{{"Effect":"Deny","Action":"{action}","Resource":"*"}},
                        {{"Effect":"Allow","Action":"lambda:*","Resource":"*"}}]}}"#
                )],
                false,
            ),
            (
                "the deny in a SECOND document beside an allowing one",
                vec![
                    r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#.to_string(),
                    format!(
                        r#"{{"Statement":[{{"Effect":"Deny","Action":"{action}","Resource":"*"}}]}}"#
                    ),
                ],
                false,
            ),
            (
                "a deny scoped to THIS resource",
                vec![format!(
                    r#"{{"Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},
                        {{"Effect":"Deny","Action":"*","Resource":"{resource}"}}]}}"#
                )],
                false,
            ),
            (
                "a deny scoped to ANOTHER resource",
                vec![format!(
                    r#"{{"Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},
                        {{"Effect":"Deny","Action":"*","Resource":"{resource}-other"}}]}}"#
                )],
                true,
            ),
        ];

        for (case, documents, permitted) in rows {
            let held: Vec<&str> = documents.iter().map(String::as_str).collect();
            let answered = authorize_action(&principal(&held), action, resource);
            match (permitted, answered) {
                (true, Ok(())) => {}
                (false, Err(denial)) => assert!(
                    denial.message.ends_with(iam::IDENTITY_POLICY_EXPLICIT_DENY),
                    "case {case:?}: expected the explicit-deny rule to be named, got {:?}",
                    denial.message
                ),
                (expected, answered) => {
                    panic!("case {case:?}: expected permitted={expected}, got {answered:?}")
                }
            }
        }
    }
}
