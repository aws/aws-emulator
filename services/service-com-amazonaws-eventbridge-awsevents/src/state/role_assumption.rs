// SPDX-License-Identifier: CC0-1.0

//! Whether EventBridge may ASSUME the role a rule's target names.
//!
//! `Target$RoleArn` is "the Amazon Resource Name (ARN) of the IAM role to be
//! used for this target when the rule is triggered", and EventBridge makes that
//! target's call ACTING AS the role — so assuming the role is the first step of
//! every such invocation, taken before the call itself is made. The step is
//! about the ROLE alone: which API call would have followed does not enter into
//! it, and a role whose trust policy does not admit [`SERVICE_PRINCIPAL`] fails
//! the delivery whatever the target was going to be.
//!
//! That is why it is stated here on its own rather than only inside the whole
//! delivery decision ([`super::delivery_refusal`], which goes on to ask what the
//! policies in force on the role allow): a handover to a target this emulator
//! makes no API call for still reaches the assume-role step, so that step is
//! what decides it, and the event is dead-lettered exactly as one whose target
//! call was refused.
//!
//! ## The assume is for ONE rule
//!
//! A trust policy's grant is only as wide as its `Condition`, and the assume
//! EventBridge makes carries the keys AWS publishes for a service that acts on a
//! caller's behalf: `aws:SourceArn`, whose value is the ARN of the RULE whose
//! target is being delivered to, and `aws:SourceAccount`, the account that rule
//! belongs to. So a role trusted for one rule's ARN cannot be assumed for
//! another's, and a role whose trust asks for a key this assume does not carry
//! (`sts:ExternalId`, which EventBridge sends none of) cannot be assumed at all
//! — the fail-closed reading [`::service_common::iam_authorization`] takes of a
//! clause it cannot decide. The decision is therefore the context-aware reading
//! of a trust document
//! ([`::service_common::iam_trust_policy::grants_service_principal`]) rather
//! than the weaker question of whether the role trusts the principal at all.

use super::*;

/// The condition key naming the RESOURCE on whose behalf a service assumes a
/// role: the rule whose target is being delivered to. AWS publishes it for
/// exactly this confused-deputy guard, so a trust policy scoped to one rule's
/// ARN is decided against the rule that is actually invoking.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key naming the ACCOUNT that resource belongs to, the coarser
/// half of the same guard.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// Why EventBridge cannot assume `role_arn` to deliver `rule_arn`'s event to the
/// target that names it, as the condition and sentence a dead-lettered event
/// carries — or `None` when nothing of the kind can be shown.
///
/// The decision is taken against the account's OWN IAM records (the ones
/// `CreateRole`/`UpdateAssumeRolePolicy` wrote) and through the shared reading of
/// a trust document ([`::service_common::iam_trust_policy`]), so no two services
/// handed the same document answer differently.
///
/// The emulator refuses only what it can SHOW, so each of these is evidence of
/// nothing and lets the delivery go ahead: a target naming no role at all, a
/// `RoleArn` the store holds no role for (it may belong to another account), a
/// role carrying no trust document, and a document that cannot be read as a
/// policy.
pub fn assume_role_refusal(
    role_arn: Option<&str>,
    rule_arn: &str,
    iam: &IamState,
) -> Option<(String, String)> {
    let role_arn = role_arn?;
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let context = assume_context(rule_arn);
    let granted = role
        .assume_role_policy_document
        .as_deref()
        .and_then(|document| {
            ::service_common::iam_trust_policy::grants_service_principal(
                document,
                SERVICE_PRINCIPAL,
                role_arn,
                &context,
            )
        });
    if granted != Some(false) {
        return None;
    }
    Some((
        FAILED_TO_ASSUME_ROLE.to_string(),
        format!(
            "Event bus failed to assume role {role_arn}: its trust policy does not let \
             {SERVICE_PRINCIPAL} perform {} for {rule_arn}.",
            ::service_common::iam_trust_policy::ASSUME_ROLE_ACTION
        ),
    ))
}

/// The condition context one such assume carries: the invoking rule and the
/// account it belongs to (see the module docs). The account is the rule ARN's
/// own, read through the ONE ARN parse, so a rule of another account is named as
/// that account.
fn assume_context(rule_arn: &str) -> Vec<(String, String)> {
    let account = ::service_common::arn::parse(rule_arn)
        .map(|arn| arn.account.to_string())
        .filter(|account| !account.is_empty())
        .unwrap_or_else(|| ::engine::emulator_account_id().to_string());
    vec![
        (SOURCE_ARN_KEY.to_string(), rule_arn.to_string()),
        (SOURCE_ACCOUNT_KEY.to_string(), account),
    ]
}
