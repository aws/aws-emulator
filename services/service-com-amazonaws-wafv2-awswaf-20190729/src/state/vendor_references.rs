// SPDX-License-Identifier: CC0-1.0

//! What the references a VENDOR's managed-rule-set publish carries have to satisfy, and
//! in which order.
//!
//! Each entry of `VersionsToPublish` names "the Amazon Resource Name (ARN) of the
//! vendor's rule group that's used in the published managed rule group version"
//! (`VersionToPublish$AssociatedRuleGroupArn`), so a publish carries a list of
//! references and WAF reads each of them for two different things:
//!
//! * WHERE the referenced resource lives — a reference naming a namespace other than the
//!   one the request addresses is one this call cannot use
//!   ([`reference_of_another_namespace`]), a comparison between two values the REQUEST
//!   carries, so it is answered before anything is resolved
//!   ([`publish_reference_of_another_namespace`]);
//! * WHAT KIND of resource it names — a value that is no rule-group ARN at all is no
//!   reference to a vendor's rule group ([`unacceptable_publish_reference`]), which is
//!   judged with the rest of the body, once the managed rule set the publish targets has
//!   itself resolved.
//!
//! Both answer the modelled `WAFInvalidParameterException` naming the reference
//! ([`invalid_arn_exception`]); whether a reference that passes both RESOLVES to a rule
//! group of this account is the question the store answers, in
//! `crate::ops::put_managed_rule_set_versions`.
//!
//! Each judgement is taken over the references in one stable order (the order the
//! operation hands them in, which is its version-key order), so a publish naming several
//! references WAF will not accept is refused with the same one on every call.

use super::*;

/// The first reference of `references` that belongs to a namespace other than the one a
/// request naming `scope` at an endpoint in `request_region` addresses, if any.
pub fn publish_reference_of_another_namespace<'a>(
    references: impl IntoIterator<Item = &'a str>,
    scope: &str,
    request_region: &str,
) -> Option<&'a str> {
    references
        .into_iter()
        .find(|arn| reference_of_another_namespace(arn, scope, request_region))
}

/// The first reference of `references` that is not a reference to a RULE GROUP, if any:
/// a value that is no wafv2 resource ARN at all, or one naming a resource of another
/// kind ([`arn_resource_kind`], [`RULE_GROUP_ARN_KIND`]).
///
/// Whether such an ARN names a rule group that exists is a different question, answered
/// against the store.
pub fn unacceptable_publish_reference<'a>(
    references: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    references
        .into_iter()
        .find(|arn| arn_resource_kind(arn) != Some(RULE_GROUP_ARN_KIND))
}
