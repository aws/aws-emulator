// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a PUBLISHED offering catalog read answers when the
//! request names no page size of its own.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness and the two readers come from
//! [`super::reserved_instance_offerings`]; this suite adds no second copy of them.
//!
//! The rule under test:
//!
//! > `MaxResults` is the bound a request MAY name, and `@default(0)`
//! > (`smithy_query(query="operation",operation="DescribeReservedInstanceOfferings")`)
//! > means it named none — so the listing is answered in the LARGEST page the member
//! > admits (`@range(max: 100)`) and, while offerings remain, with the token to resume
//! > from, the way the recorded answer of an unfiltered call carries a page of the
//! > catalog and a continuation token. A listing that FITS in that page — the one
//! > offering the `ReservedInstanceOfferingId` filter selects — is reported whole and
//! > carries no token, because there is nothing left to resume from.
//!
//! The rows differ along what the rule generalises over: how the absent bound is expressed
//! (the member left off, and the modelled default `maxResults=0` sent explicitly), the
//! listing the absent bound is asked of (the whole catalog, and the one-offering listing
//! the `ReservedInstanceOfferingId` filter selects), and — for the bound a request DOES
//! name — a page size at the modelled maximum and well under it, resumed from the token
//! the previous answer handed out.

use super::domain_lifecycle::Emulator;
use super::reserved_instance_offerings::{describe, offerings, whole_catalog};

/// The ways a request expresses "no page size": the member left off, and the modelled
/// `@default(0)` sent explicitly.
const UNBOUNDED: [&str; 2] = ["", "maxResults=0"];

/// The page sizes a request names, the largest the member admits (`@range(max: 100)`)
/// among them.
const NAMED_PAGE_SIZES: [usize; 3] = [1, 7, 100];

/// The largest page the member admits, which is also the page a request that names no
/// size is answered in (`MaxResults` is `@range(max: 100)`).
const SERVICE_PAGE: usize = 100;

/// The id of one reported offering, with the case named on failure.
fn offering_id(offering: &serde_json::Value, case: &str) -> String {
    offering["ReservedInstanceOfferingId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an offering id, got {offering:?}"))
        .to_owned()
}

#[tokio::test]
async fn a_read_that_names_no_page_size_is_answered_with_the_service_page() {
    let emulator = Emulator::new();
    // The catalog this Region publishes — the suite's own measure of the set, rather than
    // a count written into it.
    let catalog = whole_catalog(&emulator, "the published catalog").await;
    assert!(
        catalog.len() > SERVICE_PAGE,
        "the published catalog must be larger than the largest page a caller can ask for \
         for these rows to mean anything, got {} offerings",
        catalog.len()
    );

    for unbounded in UNBOUNDED {
        let case = format!("no page size, as {unbounded:?}");
        let answer = describe(&emulator, unbounded).await;
        let page = offerings(&answer, &case);
        assert_eq!(
            page,
            catalog[..SERVICE_PAGE].to_vec(),
            "case {case}: expected the first {SERVICE_PAGE} offerings of the published \
             catalog of {}, got {}",
            catalog.len(),
            page.len()
        );
        assert_eq!(
            answer.body["NextToken"].as_str().unwrap_or_default(),
            offering_id(&catalog[SERVICE_PAGE], &case),
            "case {case}: offerings remain, so the answer names the first one it did not \
             report, got {:?}",
            answer.body["NextToken"]
        );
    }
}

#[tokio::test]
async fn a_page_is_bounded_by_the_size_the_request_names_and_by_the_service_page() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "the published catalog").await;

    for page_size in NAMED_PAGE_SIZES {
        let case = format!("maxResults={page_size}");
        let answer = describe(&emulator, &format!("maxResults={page_size}")).await;
        let page = offerings(&answer, &case);
        assert_eq!(
            page,
            catalog[..page_size].to_vec(),
            "case {case}: expected the first {page_size} offerings of the published \
             catalog, got {}",
            page.len()
        );
        let token = answer.body["NextToken"].as_str().unwrap_or_default();
        assert_eq!(
            token,
            offering_id(&catalog[page_size], &case),
            "case {case}: offerings remain, so the answer names the first one it did not \
             report, got {:?}",
            answer.body["NextToken"]
        );

        // Resuming from that token with no bound of its own is answered in the service's
        // own page too, so the rule holds in the MIDDLE of the catalog: the offerings
        // after the token, at most a page of them, and a token again while any remain.
        let case = format!("{case}, resumed with no bound");
        let resumed = describe(&emulator, &format!("nextToken={token}")).await;
        let rest = offerings(&resumed, &case);
        let end = (page_size + SERVICE_PAGE).min(catalog.len());
        assert_eq!(
            rest,
            catalog[page_size..end].to_vec(),
            "case {case}: expected the {} offerings after the first page, got {}",
            end - page_size,
            rest.len()
        );
        match catalog.get(end) {
            Some(next) => assert_eq!(
                resumed.body["NextToken"].as_str().unwrap_or_default(),
                offering_id(next, &case),
                "case {case}: offerings remain, so the answer names the first one it did \
                 not report, got {:?}",
                resumed.body["NextToken"]
            ),
            None => assert!(
                resumed.body["NextToken"].is_null(),
                "case {case}: the answer reported the rest of the listing, so it carries no \
                 token, got {:?}",
                resumed.body["NextToken"]
            ),
        }
    }
}

#[tokio::test]
async fn a_listing_of_one_offering_names_nothing_to_resume_from() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "the published catalog").await;

    // The filter selects ONE offering, so the listing an unbounded read reports is that
    // one offering and there is nothing left to resume from. Rows from the catalog's
    // start, middle and end, so the row is not a property of one position in it.
    for index in [0, catalog.len() / 2, catalog.len() - 1] {
        let case = format!("the offering filter at {index} of {}", catalog.len());
        let id = offering_id(&catalog[index], &case);
        let answer = describe(&emulator, &format!("offeringId={id}")).await;
        let page = offerings(&answer, &case);
        assert_eq!(
            page.len(),
            1,
            "case {case}: the filter selects one offering, got {page:?}"
        );
        assert_eq!(
            offering_id(&page[0], &case),
            id,
            "case {case}: expected the offering the filter names, got {page:?}"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: the answer reports the whole listing, so it names nothing to \
             resume from, got {:?}",
            answer.body["NextToken"]
        );
    }
}
