// SPDX-License-Identifier: CC0-1.0

//! The repository policy document in the form this service REPORTS it.
//!
//! `SetRepositoryPolicy` does not store the caller's bytes: the document is parsed by the
//! policy engine and re-serialized, so what that call answers with — and what
//! `GetRepositoryPolicy` and `DeleteRepositoryPolicy` report afterwards — is the engine's
//! own rendering of the policy. Two things make up that rendering, and the tree states both
//! in one place ([`::service_common::iam_policy`]):
//!
//! * the LAYOUT, which is the tree's one rendering of an attached IAM policy;
//! * the CONDITION values, which the policy language defines as strings, so a value a
//!   caller wrote as a JSON number or boolean comes back in its string form.
//!
//! The rendering is a fixed point, so canonicalising once on the write is what makes every
//! later read report it.

/// A repository policy document as this service reports it: the canonical IAM-policy layout
/// with every condition value written as the string the policy language defines it as
/// ([`::service_common::iam_policy::canonical_document_with_condition_values_as_strings`]).
///
/// ECR's policy engine parses the submitted document and re-serializes it, which is where
/// both halves come from: a `Condition` written `{"NumericLessThanEquals":
/// {"aws:MultiFactorAuthAge": 300}}` is reported with the string `"300"`, while the
/// document's member order, its statement list and every string literal's own escapes are
/// reported exactly as they were submitted.
pub fn canonical_policy_document(text: &str) -> String {
    ::service_common::iam_policy::canonical_document_with_condition_values_as_strings(text)
}
