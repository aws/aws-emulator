// SPDX-License-Identifier: CC0-1.0

//! `ListListingAttributes` over the seeded AWS Marketplace Discovery catalog, and the two
//! summaries of it a search result carries.
//!
//! The rules under test, each asserted over a TABLE spanning what it generalises over — every
//! attribute type the model declares, and listings on both sides of the rule that decides
//! whether a publisher maintains a trust center:
//!
//! 1. a request is answered with the attributes of the types it NAMED and of no other type,
//!    each served as the union member the model binds that type to, and the whole set is the
//!    union of the per-type answers;
//! 2. `ListingSummary$caseStudiesSummary` and `ListingSummary$trustCenterSummary` restate what
//!    this read reports for the same listing — the count of case studies, the trust center's
//!    sources, and a subset of its certifications beside the total number of them — so a
//!    client cannot be told two different things about one listing;
//! 3. the read is `@paginated`: a walk with any page size reports every attribute exactly
//!    once, in one order, and a token minted for another query is refused rather than served;
//! 4. an id the catalog holds no listing under is the modelled 404 with this service's own
//!    not-found message, not an empty page;
//! 5. the mark of a certification the trust center names is the CERTIFYING BODY's own
//!    image, published at that body's own address — one per scheme, and the same address
//!    wherever the scheme is reported.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the protocol,
//! the routing table, input validation and the handlers are all exercised.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::LISTING_ATTRIBUTE_TYPE_VALUES;
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::not_found_message;

use crate::catalog_drilldown::{array, call, ok, router, text};

/// The union member the model binds each `ListingAttributeType` to
/// (`smithy_query(query="shape",shape="ListingAttributeResults")`), as
/// `(attributeType, member)`.
///
/// Checked against the model-generated `LISTING_ATTRIBUTE_TYPE_VALUES` by
/// `the_attribute_type_table_covers_the_model`, so the table cannot fall behind the enum.
const ATTRIBUTE_MEMBERS: [(&str, &str); 8] = [
    ("CASE_STUDY", "caseStudy"),
    ("SPECIALIZATION_DETAIL", "specializationDetail"),
    ("TRUST_CENTER_SOURCE", "trustCenterSource"),
    ("TRUST_CENTER_CERTIFICATION", "trustCenterCertification"),
    ("TRUST_CENTER_DOCUMENT", "trustCenterDocument"),
    ("TRUST_CENTER_CONTROL", "trustCenterControl"),
    ("TRUST_CENTER_FAQ", "trustCenterFaq"),
    ("TRUST_CENTER_UPDATE", "trustCenterUpdate"),
];

/// The required members of each attribute, by the union member that carries it: what a client
/// reading that attribute can count on finding.
///
/// Source: the shapes behind `ListingAttributeResults`
/// (`smithy_query(query="closure",operation="ListListingAttributes")`).
const REQUIRED_MEMBERS: [(&str, &[&str]); 8] = [
    ("caseStudy", &["caseStudyId", "caseStudyName", "shortDescription"]),
    ("specializationDetail", &[]),
    ("trustCenterSource", &["contentType", "value", "provider"]),
    (
        "trustCenterCertification",
        &["certificationType", "displayName", "logoUrl"],
    ),
    ("trustCenterDocument", &["contentType", "value", "accessType"]),
    ("trustCenterControl", &["name"]),
    ("trustCenterFaq", &["question", "answer"]),
    ("trustCenterUpdate", &["title", "description"]),
];

/// How many listings the per-listing round trips below are run over: enough to span the
/// catalog's tiers (its hand-written records come first in listing-id order, its derived ones
/// after) and both sides of the trust center rule, while keeping the call count bounded.
const SAMPLED_LISTINGS: usize = 6;

/// The page size a read that is not testing pagination asks for: the member's own maximum
/// (`MaxResults` is `@range(1..100)`), so one page carries every attribute of a listing.
const WHOLE_PAGE: i64 = 100;

/// Every attribute type the model declares, as a request names them.
fn every_type() -> Vec<String> {
    LISTING_ATTRIBUTE_TYPE_VALUES
        .iter()
        .map(|attribute_type| (*attribute_type).to_string())
        .collect()
}

/// A sample of the catalog's listings, by id.
async fn sampled_listings(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(
        router,
        "searchListings",
        &json!({ "maxResults": WHOLE_PAGE }),
    )
    .await;
    let summaries = array(&page, "listingSummaries");
    assert!(
        summaries.len() > SAMPLED_LISTINGS,
        "case setup: the catalog must hold more listings than the sample, got {}",
        summaries.len()
    );
    let step = summaries.len() / SAMPLED_LISTINGS;
    summaries
        .iter()
        .step_by(step.max(1))
        .take(SAMPLED_LISTINGS)
        .map(|summary| text(summary, "listingId"))
        .collect()
}

/// Every attribute one listing reports for `attribute_types`, read in one page.
async fn attributes(
    router: &MultiServiceRouter,
    listing_id: &str,
    attribute_types: &[String],
) -> Vec<Value> {
    let body = ok(
        router,
        "listListingAttributes",
        &json!({
            "listingId": listing_id,
            "attributeTypes": attribute_types,
            "maxResults": WHOLE_PAGE,
        }),
    )
    .await;
    assert!(
        body.get("nextToken").is_none(),
        "case {listing_id}: one page of {WHOLE_PAGE} must carry every attribute, got {body}"
    );
    assert_eq!(
        text(&body, "locale"),
        "en-US",
        "case {listing_id}: the default locale is the one the content comes back in, got {body}"
    );
    array(&body, "listingAttributes")
}

/// The union member one result carries, and the value under it.
fn served(result: &Value) -> (String, Value) {
    let object = result
        .as_object()
        .unwrap_or_else(|| panic!("a union result is an object, got {result}"));
    assert_eq!(
        object.len(),
        1,
        "a union result carries exactly one member, got {result}"
    );
    let (member, value) = object.iter().next().expect("a union carries a member");
    (member.clone(), value.clone())
}

/// The table above is exactly the model's `ListingAttributeType` enum.
#[test]
fn the_attribute_type_table_covers_the_model() {
    let tabled: BTreeSet<&str> = ATTRIBUTE_MEMBERS
        .iter()
        .map(|(attribute_type, _)| *attribute_type)
        .collect();
    let modelled: BTreeSet<&str> = LISTING_ATTRIBUTE_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        tabled, modelled,
        "case ListingAttributeType: the suite must cover every modelled attribute type, got \
         {tabled:?}"
    );
    let members: BTreeSet<&str> = REQUIRED_MEMBERS.iter().map(|(member, _)| *member).collect();
    let bound: BTreeSet<&str> = ATTRIBUTE_MEMBERS.iter().map(|(_, member)| *member).collect();
    assert_eq!(
        members, bound,
        "case ListingAttributeResults: every union member must state its required members, \
         got {members:?}"
    );
}

/// RULE 1: a request is answered with the attributes of the types it named and of no other
/// type, each as its own union member, and the whole set is the union of the per-type answers.
#[tokio::test]
async fn a_request_is_answered_with_the_types_it_named_and_no_other() {
    let router = router();
    let mut served_types: BTreeSet<String> = BTreeSet::new();
    for listing_id in sampled_listings(&router).await {
        let mut per_type = 0_usize;
        for (attribute_type, member) in ATTRIBUTE_MEMBERS {
            let results = attributes(
                &router,
                &listing_id,
                &[attribute_type.to_string()],
            )
            .await;
            per_type += results.len();
            if !results.is_empty() {
                served_types.insert(attribute_type.to_string());
            }
            for result in &results {
                let (served_member, value) = served(result);
                assert_eq!(
                    served_member, member,
                    "case {listing_id} {attribute_type}: a result must be served as the member \
                     the model binds that type to, got {result}"
                );
                let required = REQUIRED_MEMBERS
                    .iter()
                    .find(|(known, _)| *known == member)
                    .map(|(_, required)| *required)
                    .unwrap_or_default();
                for name in required {
                    assert!(
                        !text(&value, name).trim().is_empty(),
                        "case {listing_id} {attribute_type}: the required member {name} must \
                         carry a value, got {value}"
                    );
                }
            }
        }
        // The whole set is the union of the per-type answers: asking for every type at once
        // reports each type's attributes once.
        let whole = attributes(&router, &listing_id, &every_type()).await;
        assert_eq!(
            whole.len(),
            per_type,
            "case {listing_id}: asking for every type must report each type's attributes \
             exactly once, got {} against {per_type}",
            whole.len()
        );
    }
    assert!(
        served_types.len() >= ATTRIBUTE_MEMBERS.len(),
        "case setup: the sample must cover every attribute type the catalog holds, got \
         {served_types:?}"
    );
}

/// RULE 1's other half: the ORDER the results come back in is the order the request named the
/// types in, which is what makes a page cut from them stable.
#[tokio::test]
async fn the_results_follow_the_order_the_request_named_the_types_in() {
    let router = router();
    let listing_id = listing_with_a_trust_center(&router).await;
    for (case, requested) in [
        ("case studies first", vec!["CASE_STUDY", "TRUST_CENTER_FAQ"]),
        ("the FAQ first", vec!["TRUST_CENTER_FAQ", "CASE_STUDY"]),
    ] {
        let requested: Vec<String> = requested.iter().map(|one| (*one).to_string()).collect();
        let results = attributes(&router, &listing_id, &requested).await;
        let leading = ATTRIBUTE_MEMBERS
            .iter()
            .find(|(attribute_type, _)| *attribute_type == requested[0])
            .map(|(_, member)| *member)
            .expect("the table covers every requested type");
        assert_eq!(
            served(&results[0]).0,
            leading,
            "case {case}: the type named first must lead the results, got {results:?}"
        );
    }
}

/// RULE 2: the summaries a search result carries restate what this read reports.
#[tokio::test]
async fn a_search_summary_restates_the_attributes_this_read_reports() {
    let router = router();
    let page = ok(
        &router,
        "searchListings",
        &json!({ "maxResults": WHOLE_PAGE }),
    )
    .await;
    let summaries = array(&page, "listingSummaries");
    let mut with_trust_center = 0_usize;
    let mut without = 0_usize;
    let mut with_case_studies = 0_usize;
    let step = (summaries.len() / SAMPLED_LISTINGS).max(1);
    for summary in summaries.iter().step_by(step).take(SAMPLED_LISTINGS) {
        let listing_id = text(summary, "listingId");

        let case_studies = attributes(&router, &listing_id, &["CASE_STUDY".to_string()]).await;
        let summarised = summary
            .get("caseStudiesSummary")
            .and_then(|summary| summary.get("caseStudyCount"))
            .and_then(Value::as_u64)
            .map(|count| usize::try_from(count).unwrap_or(usize::MAX));
        assert_eq!(
            summarised,
            (!case_studies.is_empty()).then_some(case_studies.len()),
            "case {listing_id}: caseStudiesSummary must count the case studies this read \
             reports, got {summary}"
        );
        with_case_studies += usize::from(!case_studies.is_empty());

        let sources = attributes(&router, &listing_id, &["TRUST_CENTER_SOURCE".to_string()]).await;
        let certifications = attributes(
            &router,
            &listing_id,
            &["TRUST_CENTER_CERTIFICATION".to_string()],
        )
        .await;
        match summary.get("trustCenterSummary") {
            Some(trust_center) => {
                with_trust_center += 1;
                let summarised_sources = array(trust_center, "sourceSummaries");
                assert_eq!(
                    summarised_sources.len(),
                    sources.len(),
                    "case {listing_id}: the summary must carry the trust center's own sources, \
                     got {trust_center}"
                );
                for (summarised, reported) in summarised_sources.iter().zip(&sources) {
                    let (_, source) = served(reported);
                    assert_eq!(
                        (text(summarised, "value"), text(summarised, "provider")),
                        (text(&source, "value"), text(&source, "provider")),
                        "case {listing_id}: a summarised source must be the one this read \
                         reports, got {summarised} against {source}"
                    );
                }
                assert_eq!(
                    trust_center
                        .get("totalCertificationCount")
                        .and_then(Value::as_u64),
                    Some(certifications.len() as u64),
                    "case {listing_id}: totalCertificationCount must count every certification \
                     this read reports, got {trust_center}"
                );
                let summarised_certifications = array(trust_center, "certificationSummaries");
                assert!(
                    !summarised_certifications.is_empty()
                        && summarised_certifications.len() <= certifications.len(),
                    "case {listing_id}: certificationSummaries must be a non-empty subset of \
                     the certifications it counts, got {trust_center}"
                );
                let reported: BTreeSet<String> = certifications
                    .iter()
                    .map(|result| text(&served(result).1, "certificationType"))
                    .collect();
                for summarised in &summarised_certifications {
                    assert!(
                        reported.contains(&text(summarised, "certificationType")),
                        "case {listing_id}: a summarised certification must be one this read \
                         reports, got {summarised} against {reported:?}"
                    );
                }
            }
            None => {
                without += 1;
                assert!(
                    sources.is_empty() && certifications.is_empty(),
                    "case {listing_id}: a listing with no trust center summary must report no \
                     trust center attribute, got {sources:?} and {certifications:?}"
                );
            }
        }
    }
    assert!(
        with_trust_center > 0 && without > 0 && with_case_studies > 0,
        "case setup: the sample must span listings with and without a trust center and some \
         with case studies, got {with_trust_center}, {without} and {with_case_studies}"
    );
}

/// RULE 2 again, through the facet that counts the same fact: the `TRUST_CENTER` facet counts
/// the listings this read reports a trust center source for, and filtering on its value
/// selects exactly them.
#[tokio::test]
async fn the_trust_center_facet_counts_the_listings_this_read_reports_one_for() {
    let router = router();
    let facets = ok(
        &router,
        "searchFacets",
        &json!({ "facetTypes": ["TRUST_CENTER"] }),
    )
    .await;
    let values = facets
        .get("listingFacets")
        .and_then(|map| map.get("TRUST_CENTER"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        values.len(),
        1,
        "case whole catalog: the TRUST_CENTER facet reports the one value the filter takes, \
         got {values:?}"
    );
    let value = text(&values[0], "value");
    assert_eq!(
        value, "HAS_TRUST_CENTER",
        "case whole catalog: the facet value is the one documented for the filter, got {value}"
    );
    let counted = values[0].get("count").and_then(Value::as_u64).unwrap_or(0);

    let filtered = ok(
        &router,
        "searchListings",
        &json!({
            "maxResults": WHOLE_PAGE,
            "filters": [{ "filterType": "TRUST_CENTER", "filterValues": [value] }],
        }),
    )
    .await;
    let selected = array(&filtered, "listingSummaries");
    assert_eq!(
        selected.len() as u64,
        counted,
        "case HAS_TRUST_CENTER: filtering on the facet value must select its count, got {}",
        selected.len()
    );
    assert!(
        counted > 0,
        "case setup: the catalog must hold listings with a trust center, got {counted}"
    );
    for summary in &selected {
        let listing_id = text(summary, "listingId");
        let sources = attributes(&router, &listing_id, &["TRUST_CENTER_SOURCE".to_string()]).await;
        assert!(
            !sources.is_empty(),
            "case {listing_id}: a listing the filter selected must report a trust center \
             source, got {sources:?}"
        );
    }
}

/// RULE 3: a walk with any page size reports every attribute exactly once, in one order.
#[tokio::test]
async fn a_page_walk_reports_every_attribute_exactly_once() {
    let router = router();
    let listing_id = listing_with_a_trust_center(&router).await;
    let whole = attributes(&router, &listing_id, &every_type()).await;
    assert!(
        whole.len() > 3,
        "case setup: the listing must report several attributes to page over, got {}",
        whole.len()
    );
    for page_size in [1_i64, 2, 3, 7, whole.len() as i64, WHOLE_PAGE] {
        let mut walked: Vec<Value> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut input = json!({
                "listingId": listing_id.clone(),
                "attributeTypes": every_type(),
                "maxResults": page_size,
            });
            if let Some(token) = &next {
                input["nextToken"] = json!(token);
            }
            let body = ok(&router, "listListingAttributes", &input).await;
            let page = array(&body, "listingAttributes");
            assert!(
                !page.is_empty() && page.len() <= usize::try_from(page_size).unwrap_or(1),
                "case maxResults={page_size}: a page carries between one and maxResults \
                 attributes, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            next = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next.is_none() {
                break;
            }
            assert!(
                pages <= whole.len(),
                "case maxResults={page_size}: the walk must end, got {pages} pages"
            );
        }
        assert_eq!(
            walked, whole,
            "case maxResults={page_size}: the walk must report every attribute exactly once, \
             in the one order the whole page reports them"
        );
    }
}

/// RULE 3's other half: a token minted for ANOTHER query is refused rather than served as a
/// page of this one.
#[tokio::test]
async fn a_token_from_another_query_is_refused() {
    let router = router();
    let listing_id = listing_with_a_trust_center(&router).await;
    let other = listing_with_a_trust_center_other_than(&router, &listing_id).await;
    let first = ok(
        &router,
        "listListingAttributes",
        &json!({
            "listingId": listing_id.clone(),
            "attributeTypes": every_type(),
            "maxResults": 1,
        }),
    )
    .await;
    let token = text(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case setup: a page of one must hand back a token, got {first}"
    );

    // Rows: the same token on another listing, on another attribute-type set, and a token
    // this service never minted.
    let cases: Vec<(&str, Value)> = vec![
        (
            "another listing",
            json!({
                "listingId": other,
                "attributeTypes": every_type(),
                "maxResults": 1,
                "nextToken": token.clone(),
            }),
        ),
        (
            "another attribute-type set",
            json!({
                "listingId": listing_id.clone(),
                "attributeTypes": ["CASE_STUDY"],
                "maxResults": 1,
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token this service did not mint",
            json!({
                "listingId": listing_id.clone(),
                "attributeTypes": every_type(),
                "maxResults": 1,
                "nextToken": "bm90LWEtdG9rZW4=",
            }),
        ),
    ];
    for (case, input) in cases {
        let (status, body) = call(&router, "listListingAttributes", &input).await;
        assert_eq!(
            status, 400,
            "case {case}: a token that names no position in this query must be refused, got \
             {status} with {body}"
        );
        assert_eq!(
            text(&body, "__type")
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "ValidationException",
            "case {case}: expected the modelled 400, got {body}"
        );
    }
}

/// RULE 4: an id the catalog holds no listing under is the modelled 404 with this service's
/// own message, over a table of well-formed ids the catalog does not hold.
#[tokio::test]
async fn an_unknown_listing_is_refused_as_not_found() {
    let router = router();
    for listing_id in [
        "l-no-such-listing",
        "l-0000-nothing",
        "LISTING",
        "9999",
        &"l".repeat(255),
    ] {
        let (status, body) = call(
            &router,
            "listListingAttributes",
            &json!({
                "listingId": listing_id,
                "attributeTypes": ["CASE_STUDY"],
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {listing_id}: an id the catalog does not hold is the modelled 404, got \
             {status} with {body}"
        );
        assert_eq!(
            text(&body, "message"),
            not_found_message("listingId"),
            "case {listing_id}: expected this service's not-found message, got {body}"
        );
    }
}

/// RULE 5: the mark of a certification is the CERTIFYING BODY's own image, so the address is
/// the body's and not this emulator's, one per scheme, and the same scheme is published at
/// the same address wherever it is reported.
#[tokio::test]
async fn a_certification_mark_is_the_certifying_bodys_own_address() {
    let router = router();
    let listing_id = listing_with_a_trust_center(&router).await;
    let certifications = attributes(
        &router,
        &listing_id,
        &["TRUST_CENTER_CERTIFICATION".to_string()],
    )
    .await;
    assert!(
        certifications.len() > 1,
        "case {listing_id}: the listing must report several certifications, got \
         {certifications:?}"
    );

    // One address per scheme, and one scheme per address.
    let mut marks: BTreeMap<String, String> = BTreeMap::new();
    for result in &certifications {
        let (_, certification) = served(result);
        let logo = text(&certification, "logoUrl");
        let certification_type = text(&certification, "certificationType");
        assert!(
            (1..=2048).contains(&logo.chars().count())
                && (logo.starts_with("http://") || logo.starts_with("https://")),
            "case {certification_type}: logoUrl must satisfy URL's @length(1..2048) and \
             @pattern ^https?://.*$, got {logo:?}"
        );
        if let Some(held) = marks.insert(logo.clone(), certification_type.clone()) {
            panic!("case {logo}: {certification_type} and {held} publish one mark");
        }
    }

    // The mark a search result's trust center summary carries is the same address the
    // attribute read reports for that scheme: one statement about the scheme, not two.
    let summaries = array(
        &ok(
            &router,
            "searchListings",
            &json!({
                "maxResults": WHOLE_PAGE,
                "filters": [{
                    "filterType": "TRUST_CENTER",
                    "filterValues": ["HAS_TRUST_CENTER"],
                }],
            }),
        )
        .await,
        "listingSummaries",
    );
    let mut summarised = 0_usize;
    for summary in &summaries {
        let trust_center = summary
            .get("trustCenterSummary")
            .expect("a listing the TRUST_CENTER filter selected summarises its trust center");
        for certification in array(trust_center, "certificationSummaries") {
            let logo = text(&certification, "logoUrl");
            let certification_type = text(&certification, "certificationType");
            assert_eq!(
                marks.get(&logo).map(String::as_str),
                Some(certification_type.as_str()),
                "case {}: the summarised mark of {certification_type} must be the address \
                 the attribute read reports, got {logo}",
                text(summary, "listingId")
            );
            summarised += 1;
        }
    }
    assert!(
        summarised > 1,
        "case setup: the search page must summarise certifications, got {summarised}"
    );
}

/// The id of a listing whose publisher maintains a trust center.
async fn listing_with_a_trust_center(router: &MultiServiceRouter) -> String {
    listing_with_a_trust_center_other_than(router, "").await
}

/// The id of a listing with a trust center other than `excluded`.
async fn listing_with_a_trust_center_other_than(
    router: &MultiServiceRouter,
    excluded: &str,
) -> String {
    let filtered = ok(
        router,
        "searchListings",
        &json!({
            "maxResults": WHOLE_PAGE,
            "filters": [{
                "filterType": "TRUST_CENTER",
                "filterValues": ["HAS_TRUST_CENTER"],
            }],
        }),
    )
    .await;
    array(&filtered, "listingSummaries")
        .iter()
        .map(|summary| text(summary, "listingId"))
        .find(|listing_id| listing_id != excluded)
        .expect("the catalog holds listings whose publisher maintains a trust center")
}
