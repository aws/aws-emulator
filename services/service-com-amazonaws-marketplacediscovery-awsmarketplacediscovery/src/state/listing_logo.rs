// SPDX-License-Identifier: CC0-1.0

//! The logo image a catalog record publishes: the media record the catalog holds for it, the
//! address this emulator announces it at, and the image served there.
//!
//! `ListingSummary$logoThumbnailUrl`, `GetListingOutput$logoThumbnailUrl` and
//! `GetProductOutput$logoThumbnailUrl` are all "The URL of the logo thumbnail image for the
//! listing" (`smithy_query(query="shape",shape="ListingSummary")`) — an address a client
//! that renders a result tile FETCHES next, rather than a value it displays. AWS serves it
//! off the marketplace's own media host, which this emulator neither owns nor is reached on,
//! so what it publishes is an address of its OWN: the address this process answers requests
//! at ([`announced_page_base`]) followed by the path minted here, which `crate::service`
//! answers with the image.
//!
//! The per-record part of that address is the MEDIA RECORD the catalog holds for the record's
//! logo ([`LogoMedia`]) — one media id per listing, minted over the listing's own id and
//! carrying none of the text the listing is displayed by — so no absolute URL is stored for a
//! logo at all: the authority is the one this process answers requests at, read at the moment
//! of the read, and the path is the record's. A URL published on any other address is one a
//! client can render but never dereference.
//!
//! The image served there is the one every service that publishes a picture of a record it
//! holds no uploaded bytes for serves (`service_common::record_identicon`), derived from that
//! media record's id.

use super::saas_fulfillment::request_target_path;
use super::*;

/// The segment that names this service's listing-media plane, behind the reserved namespace
/// segment every plane this emulator serves at its own front door is addressed under
/// (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`):
/// `/_aws/marketplace-discovery-listing-media/{mediaId}/{authenticator}/logo.png`.
///
/// Shared by the URL a record announces and by the route that answers there, so the address
/// this service publishes is the address it serves.
pub const LISTING_MEDIA_PLANE_SEGMENT: &str = "marketplace-discovery-listing-media";

/// The last path segment of a logo address: WHICH image of the media record is served there.
const LOGO_FILE_NAME: &str = "logo.png";

/// The media type a logo is served as, and the format the shared renderer writes: the PNG
/// the `.png` address names
/// (`service_common::record_identicon::IDENTICON_MEDIA_TYPE`).
pub const LOGO_MEDIA_TYPE: &str = ::service_common::record_identicon::IDENTICON_MEDIA_TYPE;

/// The namespace the catalog mints its media identifiers in: one fixed identifier of its own,
/// so the id a record's logo is issued is the same on every boot.
const LOGO_MEDIA_NAMESPACE: ::uuid::Uuid =
    ::uuid::Uuid::from_u128(0x1d8e_4f27_63ba_4c91_9f05_7c3e_2ab4_d618);

/// The logo image of a catalog record, as the catalog stores it: the identifier of the media
/// record, which is the only part of the published address that belongs to the record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogoMedia {
    media_id: String,
}

impl LogoMedia {
    /// The media record the catalog holds for the logo of the listing `listing_id`.
    ///
    /// The id is minted name-based over the listing's own id, so it is opaque, stable across
    /// restarts, one id per listing, and carries nothing of the name or description the
    /// listing is displayed by — a client that persists a logo address handles an identifier
    /// of the shape this service issues rather than a form of the text it renders.
    ///
    /// A listing's PRODUCTS carry the same record: the logo of a listing's product is that
    /// listing's logo, one image at one address.
    #[must_use]
    pub fn of_listing(listing_id: &str) -> Self {
        Self {
            media_id: ::uuid::Uuid::new_v5(&LOGO_MEDIA_NAMESPACE, listing_id.as_bytes())
                .to_string(),
        }
    }

    /// The identifier of this media record, which the address it is served at names.
    #[must_use]
    pub fn media_id(&self) -> &str {
        &self.media_id
    }

    /// Where a client fetches this logo, for a response announced on `endpoint_base`.
    #[must_use]
    pub fn announced(&self, endpoint_base: &str) -> String {
        logo_url(endpoint_base, &self.media_id)
    }
}

/// The request path the logo of the media record `media_id` is served at.
///
/// The address carries the authenticator this process minted for that media record
/// ([`super::media_address`]), because a fetch of it presents no credential of its own: an
/// address assembled from the media id alone is one this service never handed out.
#[must_use]
pub fn logo_path(media_id: &str) -> String {
    format!(
        "/{}/{LISTING_MEDIA_PLANE_SEGMENT}/{media_id}/{}/{LOGO_FILE_NAME}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE,
        super::media_address::authenticator(media_id)
    )
}

/// Where a client reaches the logo of the media record `media_id`: the endpoint THIS emulator
/// is served at (`endpoint_base`) followed by that image's own path.
#[must_use]
pub fn logo_url(endpoint_base: &str, media_id: &str) -> String {
    format!(
        "{}{}",
        endpoint_base.trim_end_matches('/'),
        logo_path(media_id)
    )
}

/// The media record a request `uri` addresses the logo of, for a path of the form
/// [`logo_path`] mints, and `None` for every other path.
///
/// The inverse of the minting above, read through the shared addressing module, so a URL this
/// service published resolves back to the record behind it. The authenticator the address
/// carries is checked here, before any store is reached, so a path a caller assembled
/// addresses nothing. Nothing below the image's own segment is this route: an image is one
/// document, not a path space.
#[must_use]
pub fn logo_route(uri: &str) -> Option<&str> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};
    let (namespace, rest) = first_segment(request_target_path(uri))?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != LISTING_MEDIA_PLANE_SEGMENT {
        return None;
    }
    let (media_id, rest) = first_segment(rest)?;
    let (authenticator, rest) = first_segment(rest)?;
    let (file, below) = first_segment(rest)?;
    if media_id.is_empty() || file != LOGO_FILE_NAME || below != "/" {
        return None;
    }
    super::media_address::authenticates(media_id, authenticator).then_some(media_id)
}

impl ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    /// The image this emulator serves at the address the media record `media_id` publishes its
    /// logo at, or `None` when no record of this catalog holds that media id — so only an
    /// address this service actually published answers with an image.
    ///
    /// The image itself is the one every service that publishes a picture of a record it holds
    /// no uploaded bytes for serves: `service_common::record_identicon`, DERIVED from the media
    /// record's own id, so the catalog needs no image file beside it, two listings never share
    /// a logo, and nothing about the image depends on the request or the clock.
    ///
    /// A READ of the catalog: the image is derived from the stored media record, so fetching
    /// it changes nothing a later caller sees.
    #[must_use]
    pub fn listing_logo_image(&self, media_id: &str) -> Option<Vec<u8>> {
        let held = self
            .listings
            .values()
            .any(|listing| listing.logo.media_id() == media_id)
            || self
                .products
                .values()
                .any(|product| product.logo.media_id() == media_id);
        held.then(|| ::service_common::record_identicon::identicon_png(media_id))
            .flatten()
    }
}
