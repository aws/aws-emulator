// SPDX-License-Identifier: CC0-1.0

//! What the catalog's review source holds for a listing: the ratings it reports, the reviews
//! page it keeps for that listing (when it keeps one), and the address this emulator serves
//! that page at.
//!
//! `ReviewSourceSummary$sourceUrl` is "The URL where the reviews can be accessed at the
//! source" and is OPTIONAL, while `sourceName`, `sourceId`, `averageRating` and
//! `totalReviews` are required (`smithy_query(query="shape",shape="ReviewSourceSummary")`).
//! Two things follow, and both are properties of the stored record rather than of the
//! projection:
//!
//! * the member is PRESENT only for a listing the source keeps a reviews page for, because
//!   its absence is what tells a client not to render a "read the reviews" link at all; and
//! * where it is present it is the page of THAT listing's reviews, one address per listing,
//!   never one address shared by every listing in the catalog.
//!
//! `ReviewSourceId` declares exactly one value, `AWS_MARKETPLACE`
//! (`smithy_query(query="shape",shape="ReviewSourceId")`) — the marketplace this emulator
//! stands in for. So the page its reviews are read at is a page of THIS emulator's, announced
//! on the address this process answers requests at ([`announced_page_base`]) and served by
//! `crate::service` from the stored record, exactly as the buyer-facing pages of a SaaS
//! fulfillment option are ([`super::saas_fulfillment`]).

use super::saas_fulfillment::{escaped, request_target_path};
use super::*;

/// The segment that names this service's listing-reviews plane, behind the reserved namespace
/// segment every plane this emulator serves at its own front door is addressed under:
/// `/_aws/marketplace-discovery-listing-reviews/reviews-list/{reviewsListId}`.
///
/// Shared by the URL a record announces and by the route that answers there, so the address
/// this service publishes is the address it serves.
pub const LISTING_REVIEWS_PLANE_SEGMENT: &str = "marketplace-discovery-listing-reviews";

/// The segment that names the collection a reviews page belongs to: the list of the reviews
/// the source holds for one listing.
const REVIEWS_LIST_SEGMENT: &str = "reviews-list";

/// The media type a reviews page is served as: a web page, from this emulator's own origin.
pub const REVIEWS_PAGE_MEDIA_TYPE: &str = "text/html; charset=utf-8";

/// The review source every record of this catalog carries its reviews at, as
/// `(sourceId, sourceName)`.
///
/// `ReviewSourceId` declares exactly one value, and `sourceName` is "The name of the review
/// source, such as AWS Marketplace"
/// (`smithy_query(query="shape",shape="ReviewSourceSummary")`).
pub const REVIEW_SOURCE: (&str, &str) = ("AWS_MARKETPLACE", "AWS Marketplace");

/// The namespace the catalog mints its reviews-list identifiers in: one fixed identifier of
/// its own, so the id a listing's reviews list is issued is the same on every boot.
const REVIEWS_LIST_NAMESPACE: ::uuid::Uuid =
    ::uuid::Uuid::from_u128(0x7a2c_95d4_31fe_4b68_8c17_e0d9_45b2_36af);

/// The identifier of the reviews list a source keeps for one listing, minted name-based over
/// the listing and the source it is kept by: opaque, stable across restarts, and one id per
/// listing per source, so no two listings' reviews pages are the same page.
#[must_use]
pub fn reviews_list_id(listing_id: &str, source_id: &str) -> String {
    ::uuid::Uuid::new_v5(
        &REVIEWS_LIST_NAMESPACE,
        format!("{source_id}/{listing_id}").as_bytes(),
    )
    .to_string()
}

/// What a listing's review sources hold for it, as the catalog stores it.
///
/// The stored summary carries NO `sourceUrl`: where a source's reviews are read is an address
/// this emulator serves, which only the response being composed knows the authority of, so it
/// is composed when the record is announced ([`Self::announced`]) and never stored. Which
/// sources keep a reviews page for the listing is the record's own
/// ([`Self::reviews_lists`]), so a listing announces a reviews page exactly where the catalog
/// holds one for it.
#[derive(Debug, Clone, Default)]
pub struct ReviewRecord {
    /// One entry per review source, with the ratings that source reports.
    summary: ReviewSummary,
    /// The reviews list each source keeps for this listing, keyed by `sourceId`. A source
    /// that keeps none for it has no entry, and then its summary carries no `sourceUrl`.
    reviews_lists: BTreeMap<String, String>,
}

impl ReviewRecord {
    /// Store `summary` as what this listing's review sources report about it. The record
    /// keeps no reviews page until one is recorded ([`Self::keeping`]), so a listing
    /// announces none unless the catalog holds one.
    #[must_use]
    pub fn new(summary: ReviewSummary) -> Self {
        Self {
            summary,
            reviews_lists: BTreeMap::new(),
        }
    }

    /// Record that the source `source_id` keeps the reviews list `reviews_list_id` for this
    /// listing, which is the page its `sourceUrl` then announces.
    #[must_use]
    pub fn keeping(mut self, source_id: &str, reviews_list_id: String) -> Self {
        self.reviews_lists
            .insert(source_id.to_string(), reviews_list_id);
        self
    }

    /// The summary as the catalog holds it, which is what the rating a listing is filtered and
    /// sorted by is read from ([`average_customer_rating`]).
    #[must_use]
    pub fn stored(&self) -> &ReviewSummary {
        &self.summary
    }

    /// This summary as it is ANNOUNCED on `endpoint_base`: each source's stored entry,
    /// carrying the address this emulator serves
    /// that source's reviews page for this listing at — and no such member at all for a
    /// source the record keeps no page for.
    #[must_use]
    pub fn announced(&self, endpoint_base: &str) -> ReviewSummary {
        let mut summary = self.summary.clone();
        for source in &mut summary.review_source_summaries {
            source.source_url = self
                .reviews_lists
                .get(&source.source_id)
                .map(|list_id| reviews_page_url(endpoint_base, list_id));
        }
        summary
    }

    /// The reviews list the source `source_id` keeps for this listing, or `None` when it
    /// keeps none.
    #[must_use]
    pub fn reviews_list(&self, source_id: &str) -> Option<&str> {
        self.reviews_lists.get(source_id).map(String::as_str)
    }
}

/// The request path the reviews list `reviews_list_id` is served at.
///
/// The address carries the authenticator this process minted for that list
/// ([`super::media_address`]), because a fetch of it presents no credential of its own: an
/// address assembled from the list id alone is one this service never handed out.
#[must_use]
pub fn reviews_page_path(reviews_list_id: &str) -> String {
    format!(
        "/{}/{LISTING_REVIEWS_PLANE_SEGMENT}/{REVIEWS_LIST_SEGMENT}/{reviews_list_id}/{}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE,
        super::media_address::authenticator(reviews_list_id)
    )
}

/// Where a client reads the reviews of the list `reviews_list_id`: the endpoint THIS emulator
/// is served at (`endpoint_base`) followed by that page's own path.
#[must_use]
pub fn reviews_page_url(endpoint_base: &str, reviews_list_id: &str) -> String {
    format!(
        "{}{}",
        endpoint_base.trim_end_matches('/'),
        reviews_page_path(reviews_list_id)
    )
}

/// The reviews list a request `uri` addresses, for a path of the form [`reviews_page_path`]
/// mints, and `None` for every other path.
///
/// The inverse of the minting above, so a URL this service published resolves back to the
/// record behind it. The authenticator the address carries is checked here, before any store
/// is reached, so a path a caller assembled addresses nothing. Nothing below the page's own
/// segment is this route: a page is one document, not a path space.
#[must_use]
pub fn reviews_page_route(uri: &str) -> Option<&str> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};
    let (namespace, rest) = first_segment(request_target_path(uri))?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != LISTING_REVIEWS_PLANE_SEGMENT {
        return None;
    }
    let (collection, rest) = first_segment(rest)?;
    if collection != REVIEWS_LIST_SEGMENT {
        return None;
    }
    let (reviews_list_id, rest) = first_segment(rest)?;
    let (authenticator, below) = first_segment(rest)?;
    if reviews_list_id.is_empty() || below != "/" {
        return None;
    }
    super::media_address::authenticates(reviews_list_id, authenticator).then_some(reviews_list_id)
}

impl ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    /// The page this emulator serves at the address a reviews list is published at.
    ///
    /// `None` when no listing of this catalog keeps that reviews list — so only an address
    /// this service actually published answers with a page.
    ///
    /// A READ of the catalog: the page restates the stored record, so opening it changes
    /// nothing a later caller sees.
    #[must_use]
    pub fn listing_reviews_page(&self, reviews_list_id: &str) -> Option<String> {
        let (listing, source) = self.listings.values().find_map(|listing| {
            let record = listing.reviews.as_ref()?;
            let source = record.summary.review_source_summaries.iter().find(|source| {
                record.reviews_list(&source.source_id) == Some(reviews_list_id)
            })?;
            Some((listing, source))
        })?;
        Some(reviews_page(listing, source))
    }
}

/// The page a fetch of a published reviews address renders: the listing whose reviews they
/// are and what the source reports about it, as the catalog holds them right now.
///
/// Every value comes from the stored record, so the page a client opens cannot disagree with
/// what `SearchListings` and `GetListing` report about the same listing. Each value is written
/// through the escaping of the `html-escape` crate, so no stored string can open or close an
/// element of the document it is written into.
fn reviews_page(listing: &Listing, source: &ReviewSourceSummary) -> String {
    let name = &listing.text.text(DEFAULT_LOCALE).name;
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>{title}</title>\n</head>\n<body>\n<h1>Customer reviews</h1>\n<h2>{name}</h2>\n\
         <dl>\n<dt>Listing</dt><dd>{listing_id}</dd>\n\
         <dt>Review source</dt><dd>{source_name}</dd>\n\
         <dt>Average rating</dt><dd>{average_rating}</dd>\n\
         <dt>Total reviews</dt><dd>{total_reviews}</dd>\n</dl>\n</body>\n</html>\n",
        title = escaped(&format!("{name} — {}", source.source_name)),
        name = escaped(name),
        listing_id = escaped(&listing.listing_id),
        source_name = escaped(&source.source_name),
        average_rating = escaped(&source.average_rating),
        total_reviews = source.total_reviews,
    )
}

/// The page a fetch of an address of this shape that names no published reviews list renders.
#[must_use]
pub fn reviews_page_not_found_page() -> String {
    "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
     <title>AWS Marketplace reviews</title>\n</head>\n<body>\n\
     <h1>No such reviews page</h1>\n</body>\n</html>\n"
        .to_string()
}
