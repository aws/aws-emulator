// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a PUBLISHED catalog read answers when the request names no
//! page size.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`];
//! this suite adds no second copy of it.
//!
//! The rule under test: `MaxResults` is the bound the REQUEST names, and a catalog AWS
//! answers COMPLETE is answered complete when the request names none. It is `@default(0)`,
//! so a request that names no page size has named no bound: the whole published listing is
//! reported in one answer, which therefore carries no `NextToken` at all.
//!
//! The rows differ along what the rule generalises over: the catalog read (the
//! engine-version catalog and the instance-type catalog) and how the absent bound is
//! expressed (no `maxResults` at all, and the modelled default `maxResults=0`).
//!
//! The Reserved Instance offering catalog is the counter-case and is NOT one of those
//! rows: AWS answers it page by page, so this service answers a request that names no
//! bound in its own largest page and names where to resume
//! ([`super::reserved_offering_service_page`]). What the two cases below assert about it
//! is that following that token reaches every offering, and that a bound a request does
//! name is honoured.

use std::collections::BTreeSet;

use serde_json::Value;
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::RESERVED_INSTANCE_PAYMENT_OPTION_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    published_instance_types, RESERVED_INSTANCE_TERMS,
};

use super::domain_lifecycle::{Answer, Emulator, PREFIX};

/// The engine release the instance-type catalog is read for: one the service publishes,
/// so the read is a catalog rather than a refusal.
const PUBLISHED_RELEASE: &str = "OpenSearch_2.19";

/// The ways a request expresses "no page size": the member left off, and the modelled
/// `@default(0)` sent explicitly.
const UNBOUNDED: [&str; 2] = ["", "maxResults=0"];

/// One catalog read: the path it lives at and the member its set is reported in.
struct Catalog {
    case: &'static str,
    path: String,
    member: &'static str,
}

/// Every catalog this service answers COMPLETE for a request that names no page size, as a
/// read and the member to read.
fn catalogs() -> Vec<Catalog> {
    vec![
        Catalog {
            case: "engine versions",
            path: format!("{PREFIX}/opensearch/versions"),
            member: "Versions",
        },
        Catalog {
            case: "instance types",
            path: format!("{PREFIX}/opensearch/instanceTypeDetails/{PUBLISHED_RELEASE}"),
            member: "InstanceTypeDetails",
        },
    ]
}

/// One catalog read with `query` as its `@httpQuery` members.
async fn read(emulator: &Emulator, catalog: &Catalog, query: &str) -> Answer {
    let uri = if query.is_empty() {
        catalog.path.clone()
    } else {
        format!("{}?{query}", catalog.path)
    };
    emulator.call("GET", &uri, None).await
}

/// The set one answer reports, asserted served.
fn reported(answer: &Answer, catalog: &Catalog, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body[catalog.member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a {} array, got {:?}",
                catalog.member, answer.body
            )
        })
        .clone()
}

/// Every entry of `catalog`, walked at the largest page the model allows — so the
/// expectation is the service's own set rather than a number written into this suite.
async fn walked(emulator: &Emulator, catalog: &Catalog) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match token.as_deref() {
            None => "maxResults=100".to_owned(),
            Some(cursor) => format!("maxResults=100&nextToken={cursor}"),
        };
        let answer = read(emulator, catalog, &query).await;
        walked.extend(reported(&answer, catalog, catalog.case));
        match answer.body["NextToken"].as_str() {
            None => break,
            Some(next) => token = Some(next.to_owned()),
        }
        assert!(
            walked.len() < 100_000,
            "case {}: the walk terminates, got {} entries",
            catalog.case,
            walked.len()
        );
    }
    assert!(
        !walked.is_empty(),
        "case {}: expected a published catalog, got none",
        catalog.case
    );
    walked
}

#[tokio::test]
async fn a_request_that_names_no_page_size_is_answered_with_the_whole_catalog() {
    let emulator = Emulator::new();

    for catalog in catalogs() {
        let walked = walked(&emulator, &catalog).await;

        for unbounded in UNBOUNDED {
            let case = format!("{} with {unbounded:?}", catalog.case);
            let answer = read(&emulator, &catalog, unbounded).await;
            let whole = reported(&answer, &catalog, &case);
            assert_eq!(
                whole, walked,
                "case {case}: expected the whole published catalog of {} entries, got {}",
                walked.len(),
                whole.len()
            );
            assert_eq!(
                answer.body.get("NextToken"),
                None,
                "case {case}: the answer reports the whole set, so it carries no \
                 NextToken, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn paging_the_offerings_reaches_every_instance_type_term_and_payment_option() {
    let emulator = Emulator::new();

    // What no bound may hide: the offerings a client discovers by following the token.
    // Every instance type the service publishes, each term and each payment option has to
    // be reachable from the walk, or a client cannot find an offering for the instance type
    // it would actually run.
    let catalog = Catalog {
        case: "reserved instance offerings",
        path: format!("{PREFIX}/opensearch/reservedInstanceOfferings"),
        member: "ReservedInstanceOfferings",
    };
    let case = "the walked offering catalog";
    let offerings = walked(&emulator, &catalog).await;

    let types: BTreeSet<&str> = offerings
        .iter()
        .filter_map(|offering| offering["InstanceType"].as_str())
        .collect();
    for instance_type in published_instance_types() {
        assert!(
            types.contains(instance_type),
            "case {case}: expected an offering for {instance_type}, got offerings for \
             {} instance types",
            types.len()
        );
    }

    let durations: BTreeSet<i64> = offerings
        .iter()
        .filter_map(|offering| offering["Duration"].as_i64())
        .collect();
    for term in RESERVED_INSTANCE_TERMS {
        assert!(
            durations.contains(&i64::from(term)),
            "case {case}: expected the {term}-second term among {durations:?}"
        );
    }

    let options: BTreeSet<&str> = offerings
        .iter()
        .filter_map(|offering| offering["PaymentOption"].as_str())
        .collect();
    for option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
        assert!(
            options.contains(option),
            "case {case}: expected the {option} payment option among {options:?}"
        );
    }

    // Every combination of the three, so no instance type is published for fewer terms or
    // payment options than another.
    let expected = published_instance_types().len()
        * RESERVED_INSTANCE_TERMS.len()
        * RESERVED_INSTANCE_PAYMENT_OPTION_VALUES.len();
    assert_eq!(
        offerings.len(),
        expected,
        "case {case}: expected one offering per instance type, term and payment option \
         ({expected}), got {}",
        offerings.len()
    );
}

#[tokio::test]
async fn a_request_that_names_a_page_size_is_still_bounded_by_it() {
    let emulator = Emulator::new();

    // The rule is about the bound a request names: a bound it does name bounds the answer,
    // and the answer names where to resume. One row per page size, including the largest
    // the model allows.
    let catalog = Catalog {
        case: "reserved instance offerings",
        path: format!("{PREFIX}/opensearch/reservedInstanceOfferings"),
        member: "ReservedInstanceOfferings",
    };
    let whole = walked(&emulator, &catalog).await;
    for page_size in [1_usize, 3, 100] {
        let case = format!("maxResults={page_size}");
        let answer = read(&emulator, &catalog, &format!("maxResults={page_size}")).await;
        let page = reported(&answer, &catalog, &case);
        assert_eq!(
            page,
            whole[..page_size].to_vec(),
            "case {case}: expected the first {page_size} offerings, got {}",
            page.len()
        );
        assert_eq!(
            answer.body["NextToken"].as_str(),
            whole[page_size]["ReservedInstanceOfferingId"].as_str(),
            "case {case}: offerings remain, so the answer names where to resume, got {:?}",
            answer.body["NextToken"]
        );
    }
}
