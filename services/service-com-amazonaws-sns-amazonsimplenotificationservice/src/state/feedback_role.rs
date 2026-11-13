// SPDX-License-Identifier: CC0-1.0

//! Whether a delivery-status feedback role a write of a topic attribute NAMES is one this
//! service could ever assume, asked before the attribute lands on the topic.
//!
//! "The <ENDPOINT>SuccessFeedbackRoleArn and <ENDPOINT>FailureFeedbackRoleArn attributes are
//! used to give Amazon SNS write access to use CloudWatch Logs on your behalf"
//! (`SetTopicAttributesInput$AttributeName` doc): the value names an IAM role Amazon SNS
//! ASSUMES to write a delivery's status record ([`super::delivery_status`]), so a role whose
//! trust policy does not admit this service's principal ([`super::SERVICE_PRINCIPAL`]) for
//! `sts:AssumeRole` is a role the feature could never work under. SNS refuses such a write
//! rather than storing a configuration it cannot honour, and the topic reads back carrying no
//! such attribute.
//!
//! The question is the tree's one reading of a role's trust relationship
//! (`service_common::iam_role::held_role_is_unassumable`), so the document that decides it is
//! the one a client's `CreateRole` or `UpdateAssumeRolePolicy` left on the role, and the
//! reading of it is the same one the publish-time assume takes. It is the HELD reading: a role
//! this account's IAM holds nothing under is no evidence about the role the caller named — a
//! feedback role is routinely provisioned out of band — so only a stored trust document that
//! itself withholds the assume is refused.

use std::collections::HashMap;

/// The refusal a write of the topic attribute `name` to `value` answers when the value names
/// a role this service could not assume, or `None` for every other value.
///
/// Answers `None` for a name that is no feedback-role attribute, so a caller hands it every
/// attribute a write carries and the rule decides which of them it is about. The sentence is
/// the one SNS states for a value it cannot use as a logging role
/// ([`super::topic_attributes::not_a_valid_logging_role`]), whichever half of the rule — the
/// ARN's form or the trust policy behind it — the value broke.
///
/// Call it with NO store guard held: it asks IAM over the in-process callback.
pub async fn untrusted_role_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    value: &str,
) -> Option<String> {
    if !name.ends_with(super::FEEDBACK_ROLE_ARN_SUFFIX) {
        return None;
    }
    ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        region,
        value,
        super::SERVICE_PRINCIPAL,
    )
    .await
    .then(|| super::topic_attributes::not_a_valid_logging_role(name, value))
}

/// The refusal the feedback roles `attributes` names answer, or `None` when every one of them
/// is a role this service could assume.
///
/// The map form of [`untrusted_role_refusal`], for the write path that carries a whole
/// attribute map: the entries are asked about in name order, so a map naming two unusable
/// roles is refused with the same sentence whichever order the request spelled them in.
pub async fn untrusted_roles_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    attributes: &HashMap<String, String>,
) -> Option<String> {
    let mut names: Vec<&String> = attributes
        .keys()
        .filter(|name| name.ends_with(super::FEEDBACK_ROLE_ARN_SUFFIX))
        .collect();
    names.sort();
    for name in names {
        if let Some(message) =
            untrusted_role_refusal(ctx, region, name, &attributes[name]).await
        {
            return Some(message);
        }
    }
    None
}
