// SPDX-License-Identifier: CC0-1.0

//! WHICH page size each listing of this service refuses, end to end over the wire.
//!
//! RULE under test: **a page size is held to the bounds the LISTING publishes, and a
//! listing that never pages publishes none — the listings that page refuse a `maxResults`
//! outside their published range, at either end, while the add-on version CATALOG, which
//! the recorded service answers WHOLE whatever the request asks for, serves every page
//! size and ignores it: a listing with no page after the first cuts nothing at a page
//! size, so the member bounds nothing. Every one of the paging listings' refusals is HTTP
//! 400 `InvalidParameterException` in this API's own page-size sentence, and it is answered
//! before the request's resource is resolved, so the complaint is about the page the caller
//! asked for and not about any cluster.**
//!
//! The recorded corpus is what says which half a listing is in: `DescribeAddonVersions`
//! answers a `maxResults` outside the range its member's documentation prints ("This value
//! can be between 1 and 100") with the whole catalog, above the maximum and below the
//! minimum alike, so that range is not one this listing enforces; the paging listings
//! answer one outside theirs with the sentence. The rows below span both halves and, inside
//! each, what the rule generalises over: which listing (the catalog, two listings of a
//! cluster's children addressed by a cluster NAME, and two whose whole address is a query
//! string), and how far outside the published bound the asked-for page is — one past the
//! maximum, far past it, the largest value a page size can be spelled as, one below the
//! minimum and a negative page size.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    self, MAX_LIST_MAX_RESULTS, MIN_LIST_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::wire::{call, emulator, error_code, Answer};

/// The code every page-size refusal of this API carries: the `InvalidParameterException`
/// each of these listings models with `@httpError 400`
/// (`smithy_query(query="operation",operation="ListClusters,ListAddons,ListNodegroups,
/// DescribeClusterVersions,DescribeAddonVersions")`).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// A cluster name no create in this suite ever made, so a refusal that names the page
/// size proves the page is judged BEFORE the cluster is resolved — the order the recorded
/// service answers these two complaints in.
const ABSENT_CLUSTER: &str = "page-size-no-such-cluster";

/// One listing: the case it is named by, the path it is addressed at, and whether the
/// range its model shape declares is one the listing ENFORCES.
struct Listing {
    case: &'static str,
    path: &'static str,
    /// `false` for a listing the recorded service answers whole whatever the page size
    /// says — the add-on version catalog, which has no page after the first.
    enforces_range: bool,
}

/// Every listing of this service that takes a `maxResults`, with the half of the rule it
/// is in.
const LISTINGS: &[Listing] = &[
    Listing {
        case: "the add-on version catalog",
        path: "/addons/supported-versions",
        enforces_range: false,
    },
    Listing {
        case: "the cluster listing",
        path: "/clusters",
        enforces_range: true,
    },
    Listing {
        case: "the published cluster-version calendar",
        path: "/cluster-versions",
        enforces_range: true,
    },
    Listing {
        case: "a cluster's add-on listing",
        path: concat!("/clusters/", "page-size-no-such-cluster", "/addons"),
        enforces_range: true,
    },
    Listing {
        case: "a cluster's node-group listing",
        path: concat!("/clusters/", "page-size-no-such-cluster", "/node-groups"),
        enforces_range: true,
    },
];

/// Ask `listing` for a page of `page_size` entries.
async fn listed(router: &MultiServiceRouter, listing: &Listing, page_size: i64) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("{}?maxResults={page_size}", listing.path),
        "",
    )
    .await
}

/// Hold one answer to this API's page-size refusal: the status, the code and the sentence
/// whose two numbers are the published bounds themselves.
fn assert_page_size_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a page size outside the published bounds, got \
         {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, INVALID_PARAMETER,
        "case {case:?}: expected {INVALID_PARAMETER}, got {code:?} in {answer:?}"
    );
    assert_eq!(
        answer.message(),
        state::max_results_range_message(),
        "case {case:?}: expected this API's page-size sentence, got {:?}",
        answer.message()
    );
}

#[tokio::test]
async fn a_page_size_past_the_maximum_is_refused_only_where_the_listing_pages() {
    let router = emulator();

    // Rows: every listing, at every size past the maximum its model shape declares —
    // one past it, far past it, and the largest value a page size can be spelled as.
    for listing in LISTINGS {
        for page_size in [
            MAX_LIST_MAX_RESULTS + 1,
            10 * MAX_LIST_MAX_RESULTS,
            i64::from(i32::MAX),
        ] {
            let case = format!("{} at maxResults={page_size}", listing.case);
            let answer = listed(&router, listing, page_size).await;
            if listing.enforces_range {
                assert_page_size_refusal(&case, &answer);
                continue;
            }
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a listing that never pages serves every page size, got \
                 {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_below_the_minimum_is_refused_only_where_the_listing_pages() {
    let router = emulator();

    // Rows: every listing, at every size below the published minimum — the value one
    // below it, a negative page size and a far negative one, none of which names a page.
    for listing in LISTINGS {
        for page_size in [MIN_LIST_MAX_RESULTS - 1, -MIN_LIST_MAX_RESULTS, -100] {
            let case = format!("{} at maxResults={page_size}", listing.case);
            let answer = listed(&router, listing, page_size).await;
            if !listing.enforces_range {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a listing that never pages serves every page size, got \
                     {answer:?}"
                );
                continue;
            }
            assert_page_size_refusal(&case, &answer);
            // The complaint is about the page, not about the resource: the two listings
            // addressed by a cluster NAME name a cluster this suite never created, and the
            // answer is still the page-size sentence rather than a not-found.
            assert!(
                !answer.message().contains(ABSENT_CLUSTER),
                "case {case:?}: expected the page-size sentence alone, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn the_catalog_serves_the_same_listing_at_every_page_size() {
    let router = emulator();

    /// The add-on names one answer reports, in the order it reports them.
    fn reported(answer: &Answer) -> Vec<String> {
        answer.body["addons"]
            .as_array()
            .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
            .iter()
            .map(|addon| {
                addon["addonName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected an addonName, got {addon:?}"))
                    .to_string()
            })
            .collect()
    }

    let unbounded = call(&router, REGION, "GET", "/addons/supported-versions", "").await;
    assert_eq!(
        unbounded.status, 200,
        "setup DescribeAddonVersions with no page size: expected HTTP 200, got {unbounded:?}"
    );
    let whole = reported(&unbounded);
    assert!(
        !whole.is_empty(),
        "setup: the catalog must publish at least one add-on, got {:?}",
        unbounded.body
    );

    // Rows: below the published minimum, the minimum itself, a size between the bounds,
    // the documented maximum and sizes past it — the catalog is answered WHOLE at every
    // one of them, and no page token is minted, because there is no page after the first.
    for page_size in [
        -1,
        MIN_LIST_MAX_RESULTS - 1,
        MIN_LIST_MAX_RESULTS,
        2,
        MAX_LIST_MAX_RESULTS,
        MAX_LIST_MAX_RESULTS + 1,
        i64::from(i32::MAX),
    ] {
        let case = format!("maxResults={page_size}");
        let answer = call(
            &router,
            REGION,
            "GET",
            &format!("/addons/supported-versions?maxResults={page_size}"),
            "",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            reported(&answer),
            whole,
            "case {case:?}: expected the whole catalog, got {:?}",
            answer.body["addons"]
        );
        assert!(
            answer
                .body
                .get("nextToken")
                .is_none_or(serde_json::Value::is_null),
            "case {case:?}: expected no nextToken, got {:?}",
            answer.body.get("nextToken")
        );
    }
}
