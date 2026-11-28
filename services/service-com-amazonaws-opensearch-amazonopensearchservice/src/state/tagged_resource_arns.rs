// SPDX-License-Identifier: CC0-1.0

//! The `ARN` the tagging operations take, read for its FORM.
//!
//! `AddTags`, `ListTags` and `RemoveTags` each take an `ARN` the model constrains by LENGTH
//! alone (`@length(min: 20, max: 2048)` with pattern `.*`), so the request boundary admits
//! any text of that size and reading the value is the service's own job. The service tags
//! three resources — a domain, an application and a direct query data source — and it
//! addresses each of them by an ARN of its own namespace, so a value that cannot be read as
//! one of those ARNs names no resource this service could ever tag, whatever the account
//! holds: that is a fact of the VALUE and is refused as a malformed ARN.
//!
//! It is a FORM test only, and the two answers are deliberately different: a value this
//! module rejects could name nothing in any account, while one it admits is an ARN whose
//! resource the account either holds or does not — which the store is what answers
//! ([`super::ComAmazonawsOpensearchAmazonopensearchserviceState::tagged_resource`]).
//!
//! Nothing here reaches the clock or the store: each rule is a pure function of the value a
//! request carried, so the same value decides the same way twice.

use ::service_common::arn;

use crate::ops::types::ValidationException;

/// The resource types the three tagging operations' `ARN` can name: the resources this
/// service tags, as the ARN each of them is addressed by spells its type —
/// [`super::domain_arn`], [`super::application_arn`] and
/// [`super::direct_query_data_source_arn`].
///
/// `AddTags` documents the member as "the Amazon Resource Name (ARN) of the domain, data
/// source, or application to attach resource tags to", so these three ARE the set, and an
/// ARN naming any other kind of resource is not a value the operation takes.
const TAGGED_RESOURCE_TYPES: [&str; 3] = ["domain", "application", "datasource"];

/// Whether `value` can be read as the ARN of a resource this service tags.
///
/// Every such ARN is minted by this service ([`::engine::emulator_arn`]), so the form is
/// the one it mints: the six ARN fields, this service's own namespace in the vendor field
/// — the domain plane's for a domain and a data source, the application plane's for an
/// application ([`super::ARN_NAMESPACE`], [`super::APPLICATION_ARN_NAMESPACE`]) — a Region
/// LABEL, an account id, and a resource field naming one of the types this service tags
/// ([`TAGGED_RESOURCE_TYPES`]) with a non-empty id after it.
///
/// The Region and the account are not compared against this emulator's: an ARN of another
/// Region or another account is a well-formed ARN that names a resource this account does
/// not hold here, which is the resolution's answer rather than this one's.
#[must_use]
pub fn names_a_tagged_resource(value: &str) -> bool {
    let Some(parsed) = arn::parse(value) else {
        return false;
    };
    let of_this_service =
        parsed.vendor == super::ARN_NAMESPACE || parsed.vendor == super::APPLICATION_ARN_NAMESPACE;
    let addressable =
        arn::region_label(parsed.region).is_some() && arn::is_account_id(parsed.account);
    let names_a_resource = TAGGED_RESOURCE_TYPES.contains(&parsed.resource_type())
        && arn::resource_id_of_type(value, parsed.resource_type())
            .is_some_and(|id| !id.is_empty());
    of_this_service && addressable && names_a_resource
}

/// How this service states that a tagging `ARN` is not an ARN of a resource it tags: one
/// fixed sentence with no variable part.
///
/// The value could not be READ, so there is nothing of it to report back — what the caller
/// has to fix is the form of the input, not which resource it named. Distinct from
/// [`super::tagged_resource_absent`], which is what an ARN that IS one of this service's
/// answers when the account holds no such resource.
pub const MALFORMED_TAGGING_ARN: &str = "Invalid ARN format.";

/// The refusal a tagging operation answers an `ARN` that is not an ARN of a resource this
/// service tags with, or `None` for one that is.
///
/// Stated before the account's resources are consulted, because it rests on the VALUE
/// alone: a value no resource of this service could be addressed by resolves to nothing in
/// every account, so there is nothing to look up. All three tagging operations state it,
/// so a reader and the writers of one tag set refuse the same value identically.
pub fn malformed_tagging_arn_refusal(value: &str) -> Option<ValidationException> {
    if names_a_tagged_resource(value) {
        return None;
    }
    Some(ValidationException {
        message: Some(MALFORMED_TAGGING_ARN.to_string()),
    })
}
