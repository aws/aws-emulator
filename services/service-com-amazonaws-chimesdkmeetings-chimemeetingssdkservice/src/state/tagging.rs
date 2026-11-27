// SPDX-License-Identifier: CC0-1.0

//! WHETHER a tagging call may address the `ResourceARN` it named, and with which refusal
//! when it may not — decided here ONCE for the three generic tagging operations.
//!
//! The verdict is two steps, in this order: what THIS service's own store says about an ARN
//! naming one of its meetings
//! ([`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::tagging_refusal`]), and then
//! what the family's shared directory says about an ARN whose OWNING member has deleted the
//! resource ([`::service_common::chime_tags::ChimeTagDirectory::withdrawn_refusal`]). Both
//! steps answer a class of refusal rather than a shape, because each of `TagResource`,
//! `UntagResource` and `ListTagsForResource` declares its own error enum; this module
//! resolves the class AND the payload, and the handler only lifts that payload into the
//! variant its own model names.
//!
//! One decision for the three routes is the point: the same ARN has to earn the same verdict
//! whichever of them a client happened to call, and a copy of the branch per handler is a
//! copy that can gain a case or swap the two steps on its own. The payloads these refusals
//! carry are already stated once in [`super`]; the ORDER they are chosen in is stated here.

use super::*;

/// How a tagging call that may NOT address the ARN it named is refused: the error CLASS
/// every one of the three tagging operations declares a variant for, carrying the payload
/// already worded.
///
/// A class, not a shape of one operation's enum: the three operations declare three enums
/// over the same modelled shapes, so the handler maps this onto its own variant and nothing
/// here needs to know which route is serving the call.
#[derive(Debug)]
pub enum TaggingAddressRefusal {
    /// `BadRequestException` (HTTP 400): the ARN carries an identifier no resource is held
    /// under, so the input is invalid.
    BadRequest(BadRequestException),
    /// `ForbiddenException` (HTTP 403): access to the resource the ARN names is revoked for
    /// good.
    Forbidden(ForbiddenException),
    /// `ResourceNotFoundException` (HTTP 404): the ARN names no resource this service could
    /// resolve at all.
    ResourceNotFound(ResourceNotFoundException),
}

/// The refusal a tagging call naming `resource_arn` earns, or `None` when the call may be
/// served.
///
/// `state` is this service's own store and `directory` the family's shared ARN-keyed tag
/// directory, in the lock order [`chime_tags`] documents: the store is read and released
/// first, then the directory. Read by `TagResource`, `UntagResource` and
/// `ListTagsForResource`, so one ARN gets one verdict whichever of them read it.
#[must_use]
pub fn tagging_address_refusal(
    state: &::engine::ServiceState<ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState>,
    directory: &::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>,
    resource_arn: &str,
) -> Option<TaggingAddressRefusal> {
    // A meeting's tags live exactly as long as the meeting, so an ARN of this service whose
    // meeting the store does not hold carries an id that is not one of this service's; a
    // value that names no resource of this family at all resolves to nothing.
    if let Some(refusal) = state.lock().tagging_refusal(resource_arn) {
        return Some(match refusal {
            TaggingRefusal::InvalidMeetingId => {
                TaggingAddressRefusal::BadRequest(invalid_meeting_id())
            }
            TaggingRefusal::UnresolvedResource => {
                TaggingAddressRefusal::ResourceNotFound(tagged_resource_not_found(resource_arn))
            }
        });
    }
    // An ARN another family member has WITHDRAWN — the resource it named was deleted by the
    // service that owns it — is refused with that owner's class and wording, read from the
    // shared directory because this store cannot see a sibling's resources.
    let withdrawn = directory
        .lock()
        .withdrawn_refusal(resource_arn)
        .map(|refusal| (refusal.class, refusal.message.to_string()));
    withdrawn.map(|(class, message)| match class {
        ::service_common::chime_tags::WithdrawnClass::InvalidRequest => {
            TaggingAddressRefusal::BadRequest(bad_request(message))
        }
        ::service_common::chime_tags::WithdrawnClass::Forbidden => {
            TaggingAddressRefusal::Forbidden(forbidden(message))
        }
    })
}
