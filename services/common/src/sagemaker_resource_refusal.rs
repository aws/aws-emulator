// SPDX-License-Identifier: CC0-1.0

//! How the SageMaker family states a resource the account does not hold.
//!
//! # The rule
//!
//! The hosting operations of that family are recorded refusing a reference they could not
//! resolve with ONE sentence: the resource KIND as the service words it, then the identifier
//! AS THE CALLER SENT IT, in double quotes. The identifier is the caller's own — a name, or
//! whatever text the request carried — never an ARN the service synthesised from it, which is
//! an identifier the caller never supplied and which a consumer extracting the offending
//! resource from the message cannot match against what it sent.
//!
//! Here, once, because several services state that same fact: the SageMaker control plane's
//! own resources, the resources of the SageMaker data planes that resolve a reference of their
//! own, and the services that resolve a SageMaker resource on a caller's behalf. A second copy
//! of the sentence drifts into refusing the same fact two ways, and the wording is what a
//! client reads.
//!
//! # Copies still to be migrated
//!
//! Two service crates carry their own private copy of this sentence, written before there was
//! anywhere shared to put it: `service-com-amazonaws-sagemaker-sagemaker`'s
//! `state::could_not_find` and
//! `service-com-amazonaws-applicationautoscaling-anyscalefrontendservice`'s
//! `state::owning_service_resources`. They are to be MIGRATED onto this module; they are left
//! in place only because the change that moved the rule here does not own those services'
//! issues, and each is pinned by its own suite. Nothing new should inline the sentence again —
//! call this module.

/// The sentence the SageMaker family refuses a resource the account does not hold with: the
/// resource `kind` as the service words it, then `identifier` as the caller sent it.
#[must_use]
pub fn could_not_find(kind: &str, identifier: &str) -> String {
    format!("Could not find {kind} \"{identifier}\".")
}

#[cfg(test)]
mod tests {
    use super::could_not_find;

    /// THE RULE: the sentence names the kind and then the identifier the caller sent, quoted.
    ///
    /// The rows differ along what the family addresses a resource by: a name, a name carrying
    /// a separator, and an ARN a caller supplied as the identifier itself.
    #[test]
    fn the_sentence_names_the_kind_and_the_identifier_the_caller_sent() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "endpoint",
                "my-endpoint",
                "Could not find endpoint \"my-endpoint\".",
            ),
            (
                "inference router",
                "my-router",
                "Could not find inference router \"my-router\".",
            ),
            (
                "inference component",
                "arn:aws:sagemaker:us-east-1:000000000000:inference-component/ic-1",
                "Could not find inference component \
                 \"arn:aws:sagemaker:us-east-1:000000000000:inference-component/ic-1\".",
            ),
        ];
        for (kind, identifier, expected) in cases {
            let actual = could_not_find(kind, identifier);
            assert_eq!(
                &actual, expected,
                "case {kind:?} {identifier:?}: expected {expected}, got {actual}"
            );
        }
    }
}
