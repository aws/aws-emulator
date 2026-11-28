// SPDX-License-Identifier: CC0-1.0

//! Every address this service publishes for a client to FETCH is a capability, not a path
//! shape.
//!
//! The three addresses this emulator serves itself — a catalog record's logo thumbnail, the
//! reviews page a review source keeps for a listing, and the buyer-facing pages a SaaS
//! fulfillment option publishes — are opened with no credential of their own: that is how
//! AWS serves them, and a fetch of one is answered ahead of anything that could resolve a
//! caller. So the rule under test is that the ADDRESS stands in for the read that handed it
//! out:
//!
//! 1. the address a response published is served, and
//! 2. an address a caller could ASSEMBLE from what it already holds — the plane's layout,
//!    which is public, and the record's own id, which travels in every response — is served
//!    nothing, and the answer restates no record either way.
//!
//! What makes the difference is the authenticator the published path carries
//! (`state::media_address`): a value this process minted over the record the address names,
//! read back under the same secret before any catalog record is looked up. The suite never
//! states that value — it takes the published address from the response and alters it, which
//! is exactly what a caller can do.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the protocol,
//! the routing table and the served planes are all exercised, and the published addresses are
//! fetched the way a browser holding one does: anonymously, over the path alone.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::catalog_drilldown::{array, ok, router, text};
use crate::listing_media_addresses::fetch;

/// The page size the reads below ask for.
const WHOLE_PAGE: i64 = 100;

/// One published address, with the plane it belongs to named for the assertion messages, the
/// id of the record it is published for, and where in the path its authenticator sits —
/// counted from the END, because a plane whose document has a name of its own
/// (`…/<record>/<authenticator>/logo.png`) carries one segment after it and a plane whose
/// address IS the document (`…/<record>/<authenticator>`) carries none.
struct Published {
    plane: &'static str,
    address: String,
    record: String,
    authenticator_from_end: usize,
}

/// The addresses a client is handed to fetch, one per plane this emulator serves: a listing's
/// logo thumbnail, the reviews page of a review source that keeps one, and the registration
/// page of a SaaS fulfillment option.
async fn published_addresses(router: &MultiServiceRouter) -> Vec<Published> {
    let page = ok(
        router,
        "searchListings",
        &json!({ "maxResults": WHOLE_PAGE }),
    )
    .await;
    let summaries = array(&page, "listingSummaries");
    let mut addresses: Vec<Published> = Vec::new();

    let logo = summaries
        .iter()
        .find(|summary| !text(summary, "logoThumbnailUrl").is_empty())
        .expect("every listing publishes a logo thumbnail");
    addresses.push(Published {
        plane: "the logo of a listing",
        address: text(logo, "logoThumbnailUrl"),
        record: text(logo, "listingId"),
        authenticator_from_end: 1,
    });

    for summary in &summaries {
        let sources = summary
            .get("reviewSummary")
            .map(|reviews| array(reviews, "reviewSourceSummaries"))
            .unwrap_or_default();
        if let Some(source) = sources
            .iter()
            .find(|source| !text(source, "sourceUrl").is_empty())
        {
            addresses.push(Published {
                plane: "the reviews page of a review source",
                address: text(source, "sourceUrl"),
                record: text(summary, "listingId"),
                authenticator_from_end: 0,
            });
            break;
        }
    }

    'listings: for summary in &summaries {
        let listing = ok(
            router,
            "getListing",
            &json!({ "listingId": text(summary, "listingId") }),
        )
        .await;
        for entity in array(&listing, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            let options = ok(
                router,
                "listFulfillmentOptions",
                &json!({ "productId": product_id.clone(), "maxResults": 50 }),
            )
            .await;
            for option in array(&options, "fulfillmentOptions") {
                let Some(saas) = option.get("saasFulfillmentOption") else {
                    continue;
                };
                let url = text(saas, "fulfillmentUrl");
                if url.is_empty() {
                    continue;
                }
                addresses.push(Published {
                    plane: "the registration page of a SaaS fulfillment option",
                    address: url,
                    record: text(saas, "fulfillmentOptionId"),
                    authenticator_from_end: 1,
                });
                break 'listings;
            }
        }
    }

    assert_eq!(
        addresses.len(),
        3,
        "case setup: one address per served plane must be published, got {}",
        addresses.len()
    );
    addresses
}

/// RULE: every published address is served where it was published.
#[tokio::test]
async fn a_published_address_is_served() {
    let router = router();
    for Published {
        plane,
        address,
        record,
        authenticator_from_end: _,
    } in published_addresses(&router).await
    {
        let fetched = fetch(&router, "GET", &address).await;
        assert_eq!(
            fetched.status, 200,
            "case {plane}: the address {address:?} this service published must be served \
             here, got {}",
            fetched.status
        );
        assert!(
            !fetched.body.is_empty(),
            "case {plane}: a served address answers with the document it names, got none"
        );
        let _ = record;
    }
}

/// RULE: an address a caller assembles out of what it already holds is served nothing, and
/// the answer restates no record.
///
/// The rows are the alterations a caller can make to an address it was handed, each applied
/// to every plane: dropping the authenticator segment altogether (which is the address the
/// plane's public layout and the record's own id compose), changing it, truncating it, and
/// putting the record's own id there instead.
#[tokio::test]
async fn an_assembled_address_is_served_nothing() {
    let router = router();
    for Published {
        plane,
        address,
        record,
        authenticator_from_end,
    } in published_addresses(&router).await
    {
        // The published path is `…/<record part>/<authenticator>[/<document>]`: the
        // authenticator sits `authenticator_from_end` segments from the end, and the record
        // part is the segment before it.
        let segments: Vec<&str> = address.split('/').collect();
        let at = segments
            .len()
            .checked_sub(authenticator_from_end + 1)
            .expect("a published address carries an authenticator");
        let authenticator = segments
            .get(at)
            .copied()
            .expect("a published address carries an authenticator");
        let record_part = segments
            .get(at.wrapping_sub(1))
            .copied()
            .expect("a published address names the record it is published for");
        assert_ne!(
            authenticator, record_part,
            "case {plane}: the authenticator must not be the record part itself, got \
             {address:?}"
        );

        let mut rows: Vec<(&str, String)> = vec![
            (
                "the authenticator dropped",
                segments
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != at)
                    .map(|(_, segment)| *segment)
                    .collect::<Vec<&str>>()
                    .join("/"),
            ),
            (
                "the record part in its place",
                address.replace(authenticator, record_part),
            ),
            (
                "a truncated authenticator",
                address.replace(
                    authenticator,
                    authenticator.get(..authenticator.len() - 2).unwrap_or(""),
                ),
            ),
        ];
        // One changed character, whichever of the two the authenticator opens with.
        let changed = authenticator.replacen(['0', '1'], "2", 1);
        if changed != authenticator {
            rows.push(("a changed authenticator", address.replace(authenticator, &changed)));
        }

        for (case, assembled) in rows {
            assert_ne!(
                assembled, address,
                "case {plane} {case}: the row must alter the published address"
            );
            let fetched = fetch(&router, "GET", &assembled).await;
            assert_ne!(
                fetched.status, 200,
                "case {plane} {case}: {assembled:?} is an address this service never handed \
                 out, so nothing may be served for it"
            );
            assert!(
                !fetched.as_text().contains(&record),
                "case {plane} {case}: the answer for an address nothing published must \
                 restate no record, got {:?}",
                fetched.as_text()
            );
        }
    }
}
