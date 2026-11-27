// SPDX-License-Identifier: CC0-1.0

//! The PAIRING an origin access control an origin names has to satisfy: the control's
//! `OriginAccessControlOriginType` against the kind of origin it is attached to.
//!
//! `OriginAccessControlConfig$OriginAccessControlOriginType` is "the type of origin that this
//! origin access control is for", and every value of it names ONE service
//! (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`: `s3`, `mediastore`,
//! `mediapackagev2`, `lambda`). What the control does is sign the origin request
//! CloudFront makes to THAT service — the signing name of the credential scope is that
//! service's, and the grant admitting the signed fetch is written in that service's own
//! resource policy — so a control is the one that may front an origin only while the origin is
//! of the kind its type names. An origin of another kind could never accept the signature the
//! control's type says CloudFront will send, which is why the pairing is answered at the call
//! that stores the configuration rather than left to surface as an origin error on the data
//! plane.
//!
//! The kind an origin IS is read from BOTH halves of what the configuration says about it, and
//! a disagreement with either half is the refusal:
//!
//! * the configuration member the origin is SPECIFIED with
//!   ([`origin_configured_as_an_s3_bucket`]), which is the model's own statement of the kind:
//!   `S3OriginConfig` is "an origin that is an Amazon S3 bucket that is not configured with
//!   static website hosting", and `CustomOriginConfig` is "an origin that is not an Amazon S3
//!   bucket" — the exception being a bucket's static-website surface, which is specified as a
//!   custom origin and which no `s3` control may front either, because what answers there is
//!   the website endpoint rather than the bucket's REST API. So an `s3` control pairs with an
//!   `S3OriginConfig` origin and a control of any other type with one specified as a custom
//!   origin;
//! * the ADDRESS it is reached at ([`crate::service::origin_access::addresses_an_origin_of_type`]),
//!   which is the one reading of an `Origin$DomainName` this crate has — including the address a
//!   service of THIS emulator published a resource's own data plane at, which is what a Lambda
//!   function URL origin of this emulator is addressed by.
//!
//! Checked on the way IN, by every call that stores a configuration its CALLER submitted —
//! `CreateDistribution`, `CreateDistributionWithTags` and `UpdateDistribution` — so no stored
//! origin carries a control of another kind, and a call that copies a configuration already
//! stored (`CopyDistribution`, `UpdateDistributionWithStagingConfig`) carries one this rule has
//! already admitted. It stands beside the two pairing rules already there: the origin that names
//! BOTH mechanisms ([`origin_with_both_access_mechanisms`]) and the one that names a control the
//! account does not hold
//! ([`ComAmazonawsCloudfrontCloudfront20200531State::unknown_origin_access_control`]). All three
//! are refusals of the same modelled class, the first and this one answering
//! `IllegalOriginAccessConfiguration` (HTTP 400).
//!
//! The read takes and releases this service's own store lock inside the call, with no guard of
//! the caller's held — the shape [`holds_origin_access_identity`] keeps — and the published
//! address records are read under it, which is the order those records document (they are taken
//! last and take nothing else).

use super::*;

/// The `DomainName` of the first origin of `config` whose origin access control is of ANOTHER
/// kind than the origin itself, or `None` when every attached control matches the origin it
/// fronts.
///
/// Read against the STORED control, so the type compared is the one that control holds now: an
/// id that resolves to no control is not this rule's refusal but
/// [`ComAmazonawsCloudfrontCloudfront20200531State::unknown_origin_access_control`]'s, and an
/// origin that names no control at all — an absent or empty `OriginAccessControlId`, which
/// [`resolve_distribution_config`] stores as the empty string — is fronted by nothing and pairs
/// with everything. A type this emulator cannot place is not judged either, so a control is
/// refused only where the pairing is KNOWN to be wrong.
pub fn mismatched_origin_access_control<Eng: Send + Sync + 'static>(
    engine: &Eng,
    config: &DistributionConfig,
) -> Option<String> {
    let published = ::service_common::published_data_planes::PublishedPlanes::of(engine);
    let state = service_state(engine)?;
    let store = state.lock();
    config.origins.items.iter().find_map(|origin| {
        let id = origin
            .origin_access_control_id
            .as_deref()
            .filter(|id| !id.is_empty())?;
        let control = store.origin_access_control(id)?;
        let origin_type = control.config.origin_access_control_origin_type.as_str();
        // What the CONFIGURATION says the origin is: an `s3` control fronts the one kind
        // specified as an Amazon S3 bucket, and a control of every other type fronts an origin
        // specified as a custom one. An origin that states neither member says nothing about
        // its kind, so only its address is read.
        if let Some(bucket) = origin_configured_as_an_s3_bucket(origin) {
            if bucket != (origin_type == crate::service::origin_access::S3_ORIGIN_TYPE) {
                return Some(origin.domain_name.clone());
            }
        }
        // Which service answers at the address this origin is reached at, for an address this
        // emulator published one of its own resource's data planes at.
        let answering = published.as_ref().and_then(|published| {
            published.answering_service(crate::service::origin_access::origin_authority(
                &origin.domain_name,
            ))
        });
        let fronts_this_origin = crate::service::origin_access::addresses_an_origin_of_type(
            origin_type,
            &origin.domain_name,
            answering.as_deref(),
        )?;
        (!fronts_this_origin).then(|| origin.domain_name.clone())
    })
}

/// Whether the configuration member `origin` is SPECIFIED with says it is an Amazon S3 bucket
/// — `Some(true)` — or says it is not — `Some(false)` — and `None` for an origin that states
/// no such member and so says nothing about its kind.
///
/// The model lays the kinds out on the members themselves: "use `S3OriginConfig` to specify an
/// Amazon S3 bucket that is not configured with static website hosting", "use
/// `VpcOriginConfig` to specify a VPC origin", and "use `CustomOriginConfig` to specify all
/// other kinds of origins, including an Amazon S3 bucket that is configured with static
/// website hosting" — so the bucket's REST API is the one kind named by the first member, and
/// everything reached over its own HTTP surface, that static-website surface included, is named
/// by the other two.
///
/// `S3OriginConfig` decides it where a configuration states more than one of the three: it is
/// the only member that names a BUCKET at all, so a configuration carrying it has stated the
/// kind an `s3` control is for, whatever else it carries beside it.
fn origin_configured_as_an_s3_bucket(origin: &Origin) -> Option<bool> {
    if origin.s3_origin_config.is_some() {
        return Some(true);
    }
    (origin.custom_origin_config.is_some() || origin.vpc_origin_config.is_some()).then_some(false)
}

/// The modelled `IllegalOriginAccessConfiguration` (HTTP 400) for a configuration whose origin
/// names a control of another kind than the origin itself.
///
/// The sibling refusal of the same code for an origin naming both access mechanisms
/// ([`illegal_origin_access_configuration`]), and worded the same way: the sentence states the
/// condition the configuration broke, which is a property of the PAIRING rather than of any
/// value in it, so neither the control's id, nor its type, nor the origin's domain name belongs
/// in the text.
pub fn illegal_origin_access_control_origin_type()
-> crate::ops::types::IllegalOriginAccessConfiguration {
    crate::ops::types::IllegalOriginAccessConfiguration {
        message: Some(ILLEGAL_ORIGIN_ACCESS_CONTROL_ORIGIN_TYPE_MESSAGE.to_string()),
    }
}

/// The sentence `IllegalOriginAccessConfiguration` carries for a control attached to an origin
/// of another kind, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. Like [`ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE`], the sentence of the
/// other configuration this code refuses, it states the CONDITION and names nothing out of the
/// request: the two members that differ are named by their roles — the origin's type and the
/// control's — so neither the control's id, nor its type's value, nor the origin's domain name
/// is any part of it.
pub const ILLEGAL_ORIGIN_ACCESS_CONTROL_ORIGIN_TYPE_MESSAGE: &str =
    "Illegal configuration: The origin type and OAC origin type differ";
