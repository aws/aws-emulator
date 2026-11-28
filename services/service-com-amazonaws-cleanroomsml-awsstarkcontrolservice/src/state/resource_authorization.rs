// SPDX-License-Identifier: CC0-1.0

//! Whether a resource-based policy lets a call reach the resource it REFERENCES.
//!
//! ## The rule this module implements
//!
//! A Clean Rooms ML job start names the resource the job runs — the configured audience model
//! an audience generation job generates an audience from, the trained model an inference job
//! runs predictions with. That reference is RESOLVED, and the policies on the resource it
//! names asked whether they allow the action, before anything is reported about the shape of
//! the request: a reference this endpoint resolves to no resource of its own has no
//! resource-based policy that could allow the call, so it is refused with the modelled
//! `AccessDeniedException` (HTTP 403) — the error that reports insufficient access — and never
//! with the `ValidationException` (HTTP 400) a member outside its modelled constraints
//! produces. The seam that answers such a refusal BEFORE the framework's constraint walk is
//! `crate::service`'s, because the walk has already chosen a status by the time a handler
//! could speak.
//!
//! Nothing is started by a refused call, so a later read reports no such job.
//!
//! ## What every refusal of the class shares
//!
//! The reason — that no policy on the referenced resource admits the action the call asks for
//! — is the same sentence whichever reference failed to resolve
//! ([`denied_by_no_resource_based_policy`]), and the action it names is spelled as a policy
//! spells it ([`iam_action`]). Both are held here, so two refusals of one class cannot be
//! worded differently; what differs is only what each reference states about itself.

/// The sentence `stated` closed with WHY the call was not allowed: no resource-based policy on
/// the resource the call referenced admits `action`.
///
/// Every authorization refusal this service words ends in this clause, including the one for a
/// call that names no role to act as ([`super::role_authorization::role_arn_not_present_message`]).
#[must_use]
pub fn denied_by_no_resource_based_policy(stated: &str, action: &str) -> String {
    format!("{stated} because no resource-based policy allows the {action} action")
}

/// The IAM action `operation` asks for, spelled as a policy names it: this service's ARN
/// namespace — its SigV4 signing name, which is the same token — then the operation. The
/// qualification is the shared one (`service_common::iam_authorization::iam_action`), so this
/// service and every other that names an action in a refusal spell it the same way.
#[must_use]
pub fn iam_action(signing_name: &str, operation: &str) -> String {
    ::service_common::iam_authorization::iam_action(signing_name, operation)
}

/// The shape a configured audience model reference belongs to, as a refusal names it.
///
/// `StartAudienceGenerationJobRequest$configuredAudienceModelArn` targets
/// `ConfiguredAudienceModelArn`
/// (`smithy_query(query="operation",operation="StartAudienceGenerationJob")`), and the refusal
/// states the SHAPE the value was supplied for rather than the member it arrived at.
pub const CONFIGURED_AUDIENCE_MODEL_ARN_SHAPE: &str = "ConfiguredAudienceModelArn";

/// The sentence Clean Rooms ML words for an ARN that resolves to no resource this endpoint
/// serves: fixed text with the shape the reference belongs to and the value the request
/// carried, verbatim.
#[must_use]
pub fn invalid_arn_message(shape: &str, arn: &str) -> String {
    format!("Provided {shape}: {arn} is invalid. Please provide a valid arn.")
}

/// The sentence Clean Rooms ML words for a trained model whose configured model algorithm
/// association it cannot resolve: fixed text with the two kinds of resource
/// ([`super::KIND_CONFIGURED_MODEL_ALGORITHM_ASSOCIATION`], [`super::KIND_TRAINED_MODEL`]) and
/// the trained model ARN the request carried, verbatim.
#[must_use]
pub fn association_not_found_message(trained_model_arn: &str) -> String {
    format!(
        "{} not found for {} {trained_model_arn}.",
        super::KIND_CONFIGURED_MODEL_ALGORITHM_ASSOCIATION,
        super::KIND_TRAINED_MODEL
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolving a configured audience model reference comes to whether the value names one of
    /// this service's configured audience models at all. The rows span every field of the form
    /// `ConfiguredAudienceModelArn`'s pattern declares — partition, service, region, account
    /// and the resource segment — on both sides of the decision.
    #[test]
    fn a_configured_audience_model_reference_resolves_only_from_that_form() {
        for (case, value, resolves) in [
            (
                "the canonical form",
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-audience-model/model-1",
                true,
            ),
            (
                "another partition, region and account",
                "arn:aws-us-gov:cleanrooms-ml:us-gov-west-1:210987654321:\
                 configured-audience-model/model-2",
                true,
            ),
            (
                "a resource id carrying the punctuation the pattern admits",
                "arn:aws:cleanrooms-ml:eu-west-1:000000000000:configured-audience-model/\
                 team_a/model.v2-final",
                true,
            ),
            (
                "another kind of this service's resource",
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:audience-model/model-1",
                false,
            ),
            (
                "a training dataset of this service",
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:training-dataset/dataset-1",
                false,
            ),
            (
                "another service's resource",
                "arn:aws:cleanrooms:us-west-2:000000000000:configured-audience-model/model-1",
                false,
            ),
            (
                "an account field that is not twelve digits",
                "arn:aws:cleanrooms-ml:us-west-2:12345:configured-audience-model/model-1",
                false,
            ),
            (
                "an empty region field",
                "arn:aws:cleanrooms-ml::000000000000:configured-audience-model/model-1",
                false,
            ),
            (
                "a partition that is not an AWS one",
                "arn:gcp:cleanrooms-ml:us-west-2:000000000000:configured-audience-model/model-1",
                false,
            ),
            (
                "an empty resource id",
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-audience-model/",
                false,
            ),
            (
                "a resource segment of the right kind with no id at all",
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-audience-model",
                false,
            ),
            ("no ARN at all", "configured-audience-model/model-1", false),
            ("the empty value", "", false),
        ] {
            let resolved = super::super::is_configured_audience_model_arn(value);
            assert_eq!(
                resolved, resolves,
                "case {case:?}: expected {resolves} for {value:?}, got {resolved}"
            );
        }
    }

    /// Each sentence is fixed text plus the values the request carried, character for
    /// character, closed by the one clause every refusal of the class ends in. The expected
    /// sentences are spelled out rather than built from the formatters under test, so a
    /// changed template fails here. The rows span the partition, region, account and resource
    /// segments an ARN varies along.
    #[test]
    fn a_refusal_states_the_reference_the_request_carried() {
        for (arn, expected) in [
            (
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-audience-model/absent",
                "Provided ConfiguredAudienceModelArn: \
                 arn:aws:cleanrooms-ml:us-west-2:000000000000:configured-audience-model/absent \
                 is invalid. Please provide a valid arn. because no resource-based policy \
                 allows the cleanrooms-ml:StartAudienceGenerationJob action",
            ),
            (
                "arn:aws-us-gov:cleanrooms-ml:eu-west-1:210987654321:audience-model/wrong.kind",
                "Provided ConfiguredAudienceModelArn: \
                 arn:aws-us-gov:cleanrooms-ml:eu-west-1:210987654321:audience-model/wrong.kind \
                 is invalid. Please provide a valid arn. because no resource-based policy \
                 allows the cleanrooms-ml:StartAudienceGenerationJob action",
            ),
        ] {
            let stated = denied_by_no_resource_based_policy(
                &invalid_arn_message(CONFIGURED_AUDIENCE_MODEL_ARN_SHAPE, arn),
                &iam_action(super::super::ARN_SERVICE, "StartAudienceGenerationJob"),
            );
            assert_eq!(
                stated, expected,
                "case {arn:?}: expected {expected:?}, got {stated:?}"
            );
        }

        for (arn, expected) in [
            (
                "arn:aws:cleanrooms-ml:us-west-2:000000000000:membership/\
                 3e6b1f90-7a2c-4d8e-b4a1-b5130da826ad/trained-model/model-1",
                "Configured Model Algorithm Association not found for Trained Model \
                 arn:aws:cleanrooms-ml:us-west-2:000000000000:membership/\
                 3e6b1f90-7a2c-4d8e-b4a1-b5130da826ad/trained-model/model-1. because no \
                 resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob \
                 action",
            ),
            (
                "arn:aws-cn:cleanrooms-ml:cn-north-1:210987654321:membership/\
                 00000000-0000-4000-8000-000000000001/trained-model/other.model",
                "Configured Model Algorithm Association not found for Trained Model \
                 arn:aws-cn:cleanrooms-ml:cn-north-1:210987654321:membership/\
                 00000000-0000-4000-8000-000000000001/trained-model/other.model. because no \
                 resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob \
                 action",
            ),
        ] {
            let stated = denied_by_no_resource_based_policy(
                &association_not_found_message(arn),
                &iam_action(super::super::ARN_SERVICE, "StartTrainedModelInferenceJob"),
            );
            assert_eq!(
                stated, expected,
                "case {arn:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
