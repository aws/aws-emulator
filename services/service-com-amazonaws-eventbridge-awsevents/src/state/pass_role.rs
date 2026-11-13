// SPDX-License-Identifier: CC0-1.0

//! Whether the caller may HAND EventBridge an IAM role — the authorization pass every
//! operation of this service that takes a role ARN makes before it stores one.
//!
//! A request that configures a resource of this service with a role ARN asks EventBridge
//! to act AS that role later: a rule's `RoleArn`, the `RoleArn` of one of a rule's
//! targets, and the role an endpoint replicates with. Handing a service a role is the
//! action [`PASS_ROLE_ACTION`] on that ROLE, which AWS separates from the permission to
//! make the call itself: "to pass a role to a service, a user must have permission to
//! pass the role to the service", so a caller that may write the resource but holds no
//! `iam:PassRole` on the role it names is refused and nothing of the request is stored.
//! Without that separation any caller that may write a rule or a target could make
//! EventBridge assume any role of the account, which is the privilege escalation AWS
//! closes here.
//!
//! WHO the caller is, and which documents are in force on it, is
//! [`super::authorization::request_caller`] — the same resolution every other gate of
//! this service takes, so one credential speaks for one principal whatever it asks for —
//! and the decision itself is [`::service_common::iam_authorization`], the tree's ONE
//! evaluation of policy documents against `(action, resource, condition context)`.
//!
//! A role carries no resource-based policy that could grant this (its trust policy says
//! who may ASSUME it, which is a different question, decided when the assumption is made
//! — [`super::role_assumption`]), so the caller's identity-based documents are the whole
//! decision.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the hand-over in an identity-based
//! document refuses it whatever grants it; otherwise it needs an `Allow` about it in those
//! documents; with neither it is refused, which is IAM's implicit deny.
//!
//! The condition context carries [`PASSED_TO_SERVICE_KEY`], whose value is the service the
//! role is being handed TO ([`super::SERVICE_PRINCIPAL`]) — "use this key to restrict
//! which service principals a role may be passed to" — so a grant written against it
//! decides this hand-over and refuses the same role being handed to another service.
//!
//! The two callers this gate does not bound are the two
//! [`super::authorization`] names, for the same reasons: the account's own published
//! credential, which the IAM store holds no policies for and which has full control of the
//! account's own roles, and a principal carrying an attachment whose document this
//! emulator cannot read, where refusing would deny access AWS permits.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::authorization::Caller;
use super::*;

/// The IAM action a caller performs by handing a role to a service: `iam:PassRole`, in
/// IAM's own action namespace rather than this service's, because the resource it is
/// performed on is the ROLE.
pub const PASS_ROLE_ACTION: &str = "iam:PassRole";

/// The condition key naming the service a role is being handed to — the one AWS publishes
/// for this action, so a grant can be scoped to the services the account means a role to
/// be usable by.
const PASSED_TO_SERVICE_KEY: &str = "iam:PassedToService";

/// The condition keys a hand-over DECLARES it models: the service the role goes to is
/// known in full, which is what lets a clause written with IAM's `IfExists` suffix, or a
/// negated clause, be answered. A key outside this set leaves its statement undecided,
/// which never grants and never weakens a deny.
const PASS_ROLE_CONDITION_KEYS: [&str; 1] = [PASSED_TO_SERVICE_KEY];

/// The `AccessDeniedException` message a caller handing a role to this service earns when
/// the identity-based policies in force on it do not permit that, or `None` when they do —
/// asked over every role ONE request hands over, in the request's own order, so the first
/// role the policies withhold refuses the call and the request is accepted or refused as a
/// WHOLE rather than applied for the roles that were granted.
///
/// A member that carries no role, and one carrying the empty string, hands nothing over
/// and is skipped. Whether a role EXISTS is not asked — IAM decides the hand-over before
/// the service looks at the resource — so a caller holding no grant reads that it is not
/// authorized whether or not the account holds that role.
///
/// Decided with no store reached at all: `caller` is resolved BEFORE this service's store
/// is locked ([`super::authorization::request_caller`]), and this is a pure evaluation of
/// the documents it carries. Those documents are READ ONCE, however many roles the request
/// hands over ([`iam::prepare_documents`], and the decision entry point a request-time gate
/// takes, [`iam::identity_decision_prepared`]): a request naming many targets costs one
/// statement walk per role rather than one parse of every document in force per role, so
/// what this gate does is bounded by the roles the request names and not by their product
/// with the size of the policies behind them. A request that hands over no role reads
/// nothing.
#[must_use]
pub fn handed_roles_denial<'a>(
    caller: &Caller,
    roles: impl IntoIterator<Item = Option<&'a str>>,
) -> Option<String> {
    let mut roles = roles
        .into_iter()
        .flatten()
        .filter(|role_arn| !role_arn.is_empty())
        .peekable();
    roles.peek()?;
    let context = [(
        PASSED_TO_SERVICE_KEY.to_string(),
        SERVICE_PRINCIPAL.to_string(),
    )];
    let documents = iam::prepare_documents(&caller.documents);
    roles.find_map(|role_arn| role_denial(caller, &documents, &context, role_arn))
}

/// The refusal `documents` state against handing `role_arn` over, or `None` when they
/// permit it — the module docs' rule, in IAM's own order, over documents already read.
fn role_denial(
    caller: &Caller,
    documents: &[iam::PreparedDocument],
    context: &[(String, String)],
    role_arn: &str,
) -> Option<String> {
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action, the resource and the context; the principal spellings travel with the
    // request because that is what such an element would be matched against.
    let request = iam::Request::new(PASS_ROLE_ACTION, role_arn, &caller.principals)
        .with_context(context)
        .with_published_keys(&PASS_ROLE_CONDITION_KEYS);
    match iam::identity_decision_prepared(documents, &request) {
        Decision::Denied => Some(not_authorized(
            &caller.arn,
            PASS_ROLE_ACTION,
            role_arn,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        )),
        Decision::Allowed => None,
        // The two callers this gate does not bound (see the module docs).
        Decision::NotStated if caller.account_credential || caller.unreadable_attachment => None,
        Decision::NotStated => Some(not_authorized(
            &caller.arn,
            PASS_ROLE_ACTION,
            role_arn,
            &iam::no_identity_policy_allows(PASS_ROLE_ACTION),
        )),
    }
}
