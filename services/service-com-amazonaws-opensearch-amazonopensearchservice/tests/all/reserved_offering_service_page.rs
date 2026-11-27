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
//! > `MaxResults` is the bound the REQUEST names. `DescribeReservedInstanceOfferings`
//! > answers a request that names one with that page and the token to resume from, and a
//! > request that names NONE — the member is `@default(0)`,
//! > `smithy_query(query="operation",operation="DescribeReservedInstanceOfferings")` —
//! > with the whole listing it selected and no token at all, the way the recorded answer
//! > of an unfiltered call reports every offering the Region publishes and the way this
//! > service's sibling catalog reads (`ListVersions`, `ListInstanceTypeDetails`) report
//! > their own sets.
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

/// The id of one reported offering, with the case named on failure.
fn offering_id(offering: &serde_json::Value, case: &str) -> String {
    offering["ReservedInstanceOfferingId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an offering id, got {offering:?}"))
        .to_owned()
}

#[tokio::test]
async fn a_read_that_names_no_page_size_is_answered_with_the_whole_catalog() {
    let emulator = Emulator::new();
    // The catalog this Region publishes — the suite's own measure of the set, rather than
    // a count written into it.
    let catalog = whole_catalog(&emulator, "the published catalog").await;
    assert!(
        catalog.len() > NAMED_PAGE_SIZES[2],
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
            catalog.clone(),
            "case {case}: expected the whole published catalog of {} offerings, got {}",
            catalog.len(),
            page.len()
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: the answer reports the whole listing, so it names nothing to \
             resume from, got {:?}",
            answer.body["NextToken"]
        );
    }
}

#[tokio::test]
async fn the_bound_a_request_names_is_the_only_bound_on_a_page() {
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

        // Resuming from that token with no bound of its own is an unbounded read too, so
        // the rule holds in the MIDDLE of the catalog: the rest of the listing in one
        // answer, and nothing left to resume from.
        let case = format!("{case}, resumed with no bound");
        let resumed = describe(&emulator, &format!("nextToken={token}")).await;
        let rest = offerings(&resumed, &case);
        assert_eq!(
            rest,
            catalog[page_size..].to_vec(),
            "case {case}: expected the {} offerings after the first page, got {}",
            catalog.len() - page_size,
            rest.len()
        );
        assert!(
            resumed.body["NextToken"].is_null(),
            "case {case}: the answer reported the rest of the listing, so it carries no \
             token, got {:?}",
            resumed.body["NextToken"]
        );
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
