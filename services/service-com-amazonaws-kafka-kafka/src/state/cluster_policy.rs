// SPDX-License-Identifier: CC0-1.0

//! What this service admits as one cluster's resource-based POLICY, and the
//! refusals a document — or a version quoted over one — that it cannot act on
//! earns.
//!
//! A cluster policy is a document in the IAM JSON policy language, not any JSON
//! value: the service PARSES it and refuses one that breaks the language's
//! grammar, so a caller that mis-renders a policy is told so instead of being
//! told the document is live. The grammar itself is the one reading every
//! service of this emulator applies ([`::service_common::iam_policy`]); only how
//! Amazon MSK WORDS the refusal of a document breaking it is this service's, and
//! that wording lives here rather than in the handler so the two writers of a
//! policy (the attach, and any later reader quoting a version back) cannot drift
//! apart from each other.
//!
//! The version a put quotes is an OPTIMISTIC LOCK over the stored policy: the
//! first attachment quotes none, a replacement must quote the version the stored
//! policy is actually at, and both refusals carry one fixed sentence apiece with
//! no part of the request and no stored version in them. A value that is not a
//! version TOKEN of this service at all is the parameter class instead, answered
//! wherever this service reads a version ([`malformed_current_version`]), so the
//! sentences below are reached only by a token the service could have minted.
//!
//! A function of the submitted document and the stored policy's own version
//! rather than of anything the parent module HOLDS, which is why it sits beside
//! that module rather than in it.

use super::*;

/// The input member a cluster-policy refusal names when the fault is the
/// DOCUMENT, and the one it names when the fault is the VERSION the request
/// quoted (or did not quote).
///
/// Named as the API documents the member on the wire — `Policy`'s `jsonName` —
/// which is how every refusal of this service reports the parameter at fault,
/// whether the value travelled in the body or in a path label. The version a put
/// quotes is named by the one spelling every version-quoting operation of this
/// service answers with ([`CURRENT_VERSION_MEMBER`]), so one member is never
/// named two ways by two refusals of the same request.
pub const CLUSTER_POLICY_MEMBER: &str = "policy";

/// How Amazon MSK opens the refusal of a document that is not a valid cluster
/// policy. The clause the document broke follows it, in the words AWS's policy
/// validator states that clause in ([`invalid_cluster_policy`]); the offending
/// document itself is never echoed back.
pub const CLUSTER_POLICY_NOT_VALID_MESSAGE: &str = "The cluster policy is not valid.";

/// The message Amazon MSK answers a put that would REPLACE an existing cluster
/// policy without quoting the version it is replacing.
///
/// A CONSTANT of the service, fixed end to end: the version the policy is at is
/// not in it, because the whole point of the refusal is that the caller has not
/// read it. The member at fault travels in
/// `BadRequestException$InvalidParameter`.
pub const CLUSTER_POLICY_VERSION_REQUIRED_MESSAGE: &str =
    "Current cluster policy version needed for Update";

/// The message Amazon MSK answers a put whose `CurrentVersion` is not the
/// version the stored policy is at.
///
/// A CONSTANT of the service, fixed end to end — neither the quoted version nor
/// the stored one appears in it, and the caller is told to read the current
/// version back instead (the apostrophe is U+2019, as the service writes it).
/// The POLICY's own sentence, beside the one the service answers for a stale
/// CLUSTER version ([`CLUSTER_VERSION_NOT_CURRENT_MESSAGE`]): MSK words them per
/// resource, so they are named separately here rather than evened out.
pub const CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE: &str =
    "The version of the cluster policy isn\u{2019}t current. \
     Check the current version and try again.";

/// The refusal a document that is not a resource-based policy earns, or `None`
/// for a document this service stores.
///
/// The grammar is the shared reading of the IAM JSON policy language
/// ([`::service_common::iam_policy::resource_policy_defect_of`]): the document
/// is an object, it declares statements, and every statement carries the
/// elements the language requires of a resource-policy statement — an `Effect`,
/// an action and a principal. Only the WORDING is this service's:
///
/// * a required element the document or one of its statements OMITS is named, in
///   the sentence AWS's policy validator states that clause in
///   ([`::service_common::iam_policy::AttachedPolicyDefect::MissingElement`]),
///   after [`CLUSTER_POLICY_NOT_VALID_MESSAGE`];
/// * a `Version` the language does not define is reported with that validator's
///   own sentence for the version clause, after the same opening;
/// * everything else the language cannot read as a policy document is the
///   service's generic parameter refusal ([`BAD_REQUEST_MESSAGE`]), which is the
///   one thing it says about a value it cannot act on: a text that is not JSON,
///   one that is not an object, a statement that is not an object, an element
///   declaring no statement, and an element whose VALUE the language does not
///   define — an `Effect` that is DECLARED but is no spelling of either of the
///   language's two words is a value the policy parser cannot read
///   ([`::service_common::iam_policy::undefined_effect_of`]) rather than an
///   element the statement left out, so it is not worded as a missing one.
///
/// Nothing of the submitted document reaches any of those sentences beyond the
/// name of the element that is missing, which is a fixed element name of the
/// policy language rather than a value of the request.
pub fn invalid_cluster_policy(document: &str) -> Option<BadRequestException> {
    use ::service_common::iam_policy::{
        attached_policy_defect, resource_policy_defect_of, undefined_effect_of,
        AttachedPolicyDefect, PolicyDefect, INVALID_POLICY_VERSION_MESSAGE,
        REQUIRED_POLICY_ELEMENT,
    };

    let unreadable = || parameter_refusal(CLUSTER_POLICY_MEMBER, BAD_REQUEST_MESSAGE);
    // Parsed here by `serde_json` (which bounds its own nesting depth) rather
    // than by a walk of this service's own over caller text, and the grammar's
    // clauses are read off the parsed value.
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return Some(unreadable());
    };
    let defect = resource_policy_defect_of(&parsed)?;
    // A statement that DECLARES an effect outside the language's two words
    // declares a value the parser cannot read, not an element it omitted, so it
    // is not reported as a missing field: only a statement that declares no
    // effect at all reaches the clause below.
    if defect == PolicyDefect::MissingEffect && undefined_effect_of(&parsed).is_some() {
        return Some(unreadable());
    }
    // The elements the IAM JSON policy language requires, named as the language
    // names them: a document's statements, and a statement's effect, action and
    // principal. A statement that names the complement of an element
    // (`NotAction`, `NotPrincipal`) names that element, so the clause is only
    // reached by a statement that names neither.
    let missing: Option<&'static str> = match defect {
        PolicyDefect::MissingEffect => Some("Effect"),
        PolicyDefect::MissingAction => Some("Action"),
        PolicyDefect::MissingPrincipal => Some("Principal"),
        // Whether the document carries the one element the language REQUIRES is
        // the shared validator's own reading, so this surface keeps no second
        // copy of it: a document carrying none of it is missing it, while one
        // that carries it while naming no statement under it broke a different
        // clause and is answered as a document the language cannot read. The
        // shared reading takes the text, so it reads it again — only on this
        // refusal path, and over a document already bounded by the caller's
        // ceiling.
        PolicyDefect::NoStatement => matches!(
            attached_policy_defect(document, |_| true),
            Some(AttachedPolicyDefect::MissingElement(REQUIRED_POLICY_ELEMENT))
        )
        .then_some(REQUIRED_POLICY_ELEMENT),
        _ => None,
    };
    let message = match (missing, defect) {
        (Some(element), _) => not_valid(&AttachedPolicyDefect::MissingElement(element).message()),
        (None, PolicyDefect::UnsupportedVersion) => not_valid(INVALID_POLICY_VERSION_MESSAGE),
        (None, _) => return Some(unreadable()),
    };
    Some(parameter_refusal(CLUSTER_POLICY_MEMBER, &message))
}

/// One clause of the policy validator stated after
/// [`CLUSTER_POLICY_NOT_VALID_MESSAGE`], which is how MSK words what a document
/// it will not attach failed to be.
fn not_valid(clause: &str) -> String {
    format!("{CLUSTER_POLICY_NOT_VALID_MESSAGE} {clause}")
}

/// The refusal a put that would replace an existing policy without quoting a
/// version earns: [`CLUSTER_POLICY_VERSION_REQUIRED_MESSAGE`], naming the member
/// that carries the version it did not quote.
pub fn cluster_policy_version_required() -> BadRequestException {
    parameter_refusal(
        CURRENT_VERSION_MEMBER,
        CLUSTER_POLICY_VERSION_REQUIRED_MESSAGE,
    )
}

/// The refusal a put whose quoted version is not the stored policy's earns:
/// [`CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE`], naming the same member.
///
/// ONE envelope for the condition however it arose — a version quoted against a
/// policy at some other version, and one quoted against a cluster carrying no
/// policy at all — so neither the quoted version nor the stored one can reach
/// the text through a sentence composed per case.
pub fn cluster_policy_version_not_current() -> BadRequestException {
    parameter_refusal(
        CURRENT_VERSION_MEMBER,
        CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE,
    )
}

/// Whether a put may go through against the policy `stored` version
/// (`None` while the cluster carries none), given the version it `quoted`, and
/// the refusal it earns when it may not.
///
/// The optimistic lock, in one place rather than once per caller: the first
/// attachment quotes nothing, a replacement must quote the version the stored
/// policy is at, and a quoted version is checked whether or not a policy is
/// there to be at it.
///
/// `quoted` is a value whose FORM is already a version token of this service
/// ([`is_current_version_token`]); one that is not names no version of anything
/// and is the parameter refusal every version-quoting operation answers
/// ([`malformed_current_version`]), which is a different class from the two here.
pub fn cluster_policy_write_refused(
    stored: Option<&str>,
    quoted: Option<&str>,
) -> Option<BadRequestException> {
    match (stored, quoted) {
        // No policy yet: the attachment creates one, so there is no version to
        // quote — and a request that quotes one anyway quotes a version nothing
        // is at.
        (None, None) => None,
        (None, Some(_)) => Some(cluster_policy_version_not_current()),
        // A policy is there, so the put REPLACES it: the version it is at has to
        // be quoted, and has to be the one it is at.
        (Some(_), None) => Some(cluster_policy_version_required()),
        (Some(stored), Some(quoted)) => {
            (stored != quoted).then(cluster_policy_version_not_current)
        }
    }
}
