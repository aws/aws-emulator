// SPDX-License-Identifier: CC0-1.0

//! The resource policy a secret carries: the NAME it has, and the layout this
//! service REPORTS its document in.
//!
//! A resource policy travels as an opaque JSON DOCUMENT (`GetResourcePolicy`
//! "retrieves the JSON text of the resource-based policy document attached to the
//! secret", this service's Smithy model), and a service that stores one does not
//! echo the caller's bytes: it parses the document and re-serializes it, so what a
//! read answers is the one canonical rendering of the policy, whatever layout the
//! attach was written in.
//!
//! That rendering is not this service's own — it is the layout every emulated
//! service that attaches an IAM policy document reports, so it lives in the shared
//! crate ([`::service_common::iam_policy::canonical_document`], which documents the
//! layout and pins it) and is only reached from here.

/// A resource policy document in the layout this service reports it in, which is
/// the layout a secret holds it in.
///
/// Applied where a policy is attached, so no path stores a document in any other
/// layout and every reader of [`super::Secret::resource_policy`] reports the one
/// rendering: `GetResourcePolicy` answers the same text whatever layout the
/// document was submitted in, and a caller that re-attaches what it read reads the
/// same text again.
///
/// Text that is not a document this service would accept is rendered as it came;
/// which documents are acceptable is [`super::parse_resource_policy`]'s reading, and
/// it runs before this.
pub fn attached_resource_policy(document: &str) -> String {
    ::service_common::iam_policy::canonical_document(document)
}

/// The name of the one resource policy a secret carries: "If you don't specify
/// this field, Secrets Manager uses `default`"
/// (`PutResourcePolicyRequest$PolicyName`, this service's Smithy model), and
/// `default` is also the only policy a caller may attach, modify or delete ("You
/// can modify only the default policy", `ListResourcePolicies`).
pub const DEFAULT_RESOURCE_POLICY_NAME: &str = "default";

/// The policy a request's `PolicyName` addresses: the name it carries, else
/// [`DEFAULT_RESOURCE_POLICY_NAME`] — which is also what the response reports
/// ("The name of the resource policy. If you didn't specify PolicyName in the
/// request, this is `default`", `PutResourcePolicyResponse$PolicyName`).
#[must_use]
pub fn addressed_resource_policy_name(policy_name: Option<&str>) -> &str {
    policy_name.unwrap_or(DEFAULT_RESOURCE_POLICY_NAME)
}

/// Whether a request's `PolicyName` addresses the one policy a secret carries.
#[must_use]
pub fn names_default_resource_policy(policy_name: Option<&str>) -> bool {
    addressed_resource_policy_name(policy_name) == DEFAULT_RESOURCE_POLICY_NAME
}

/// The `InvalidParameterException` message a WRITE of any other policy name is
/// refused with: "You can only attach the `default` policy; if you specify any
/// other name, the operation returns an `InvalidParameterException`"
/// (`PutResourcePolicyRequest$PolicyName`, this service's Smithy model), and the
/// same for deleting one (`DeleteResourcePolicyRequest$PolicyName`).
///
/// ONE fixed text for both writes: the name the request carried is not quoted
/// back, exactly as this service's other parameter refusals state the rule rather
/// than the value.
pub const ONLY_DEFAULT_RESOURCE_POLICY_MESSAGE: &str =
    "Invalid PolicyName. You can only attach, modify or delete the default resource policy.";

/// The `ResourceNotFoundException` message a READ of any other policy name is
/// refused with: "If you specify a name other than `default` and no policy with
/// that name exists, the operation returns a `ResourceNotFoundException`"
/// (`GetResourcePolicyRequest$PolicyName`, this service's Smithy model).
///
/// The same form as [`super::SECRET_NOT_FOUND_MESSAGE`]: one fixed sentence
/// naming the kind of resource that could not be found.
pub const RESOURCE_POLICY_NOT_FOUND_MESSAGE: &str =
    "Secrets Manager can't find the specified resource policy.";
