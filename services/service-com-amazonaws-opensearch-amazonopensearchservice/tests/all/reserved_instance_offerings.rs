// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeReservedInstanceOfferings`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports the whole published catalog — one offering per instance type
//!   the model's `OpenSearchPartitionInstanceType` enum declares, times each term,
//!   times each value of `ReservedInstancePaymentOption` — each with a `GUID` of its
//!   own, each publishing the one hourly recurring charge every offering carries, and
//!   each offering's price members holding together the way its payment option bills
//!   (the whole term upfront and a zero hourly charge, nothing upfront and the whole
//!   term hourly, or half of each);
//! * the prices are ORDERED by what is reserved rather than equal to any one figure:
//!   over a table of instance types, a longer term costs less per hour than a shorter
//!   one, more upfront costs less than less upfront, a bigger size of one family costs
//!   more than a smaller one, and the instance classes rank as the catalog's rate
//!   table does;
//! * the `ReservedInstanceOfferingId` filter reports EXACTLY the offering it names,
//!   over a table of ids drawn from the listing, and refuses a well-formed id the
//!   catalog publishes nothing for, and a malformed one, with the modelled errors;
//!   a pagination token is scoped to the filter it was issued under;
//! * paging is a partition: over a table of page sizes, the pages concatenate back to
//!   the full listing in order with nothing repeated or dropped, every page is within
//!   its size, only the last page carries no token, a page size outside the modelled
//!   bound is refused at either end, and a token the operation never issued is
//!   refused;
//! * the read changes nothing a later caller sees: the same request twice answers
//!   identically, before and after paged and filtered reads.

use std::collections::BTreeSet;

use serde_json::Value;
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::RESERVED_INSTANCE_PAYMENT_OPTION_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    is_guid, published_instance_types, PAYMENT_OPTION_ALL_UPFRONT, PAYMENT_OPTION_NO_UPFRONT,
    PAYMENT_OPTION_PARTIAL_UPFRONT, RECURRING_CHARGE_HOURLY, RESERVED_INSTANCE_CURRENCY_CODE,
    RESERVED_INSTANCE_OFFERING_PAGE, RESERVED_INSTANCE_TERMS, TERM_ONE_YEAR, TERM_THREE_YEARS,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// `DescribeReservedInstanceOfferings`:
/// `GET /2021-01-01/opensearch/reservedInstanceOfferings`, with `query` as its
/// `@httpQuery` members (empty for a request that sends none).
pub(crate) async fn describe(emulator: &Emulator, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/reservedInstanceOfferings")
    } else {
        format!("{PREFIX}/opensearch/reservedInstanceOfferings?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `ReservedInstanceOfferings` list of an answer, with the case named on failure.
pub(crate) fn offerings(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["ReservedInstanceOfferings"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a ReservedInstanceOfferings array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// A numeric member of one offering, with the case named on failure.
fn number_member(offering: &Value, member: &str, case: &str) -> f64 {
    offering[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {offering:?}"))
}

/// One offering's `RecurringCharges`, with the case named on failure. Every offering
/// publishes one, whatever its payment option: an offering whose term is paid up front
/// publishes a zero amount.
fn recurring_charges(offering: &Value, case: &str) -> Vec<Value> {
    offering["RecurringCharges"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a RecurringCharges array, got {offering:?}"))
        .clone()
}

/// What one offering costs per hour of its term, however it is billed: the upfront
/// charge spread over the term, plus every recurring charge, plus the usage rate.
/// Comparing offerings by THIS is what makes "a longer term is cheaper" a statement
/// about price rather than about which member carries it.
fn hourly_cost(offering: &Value, case: &str) -> f64 {
    let term_hours = number_member(offering, "Duration", case) / 3600.0;
    assert!(
        term_hours > 0.0,
        "case {case}: expected a positive Duration, got {offering:?}"
    );
    let recurring: f64 = recurring_charges(offering, case)
        .iter()
        .map(|charge| number_member(charge, "RecurringChargeAmount", case))
        .sum();
    number_member(offering, "FixedPrice", case) / term_hours
        + recurring
        + number_member(offering, "UsagePrice", case)
}

/// The whole catalog, walked through the continuation tokens the answers hand out.
///
/// The catalog holds more offerings than one page carries, so a request that names no
/// `MaxResults` is still answered with a bounded page and a token: a reader that stops
/// at the first answer sees only part of the catalog.
pub(crate) async fn whole_catalog(emulator: &Emulator, case: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match token.as_deref() {
            None => String::new(),
            Some(token) => format!("nextToken={token}"),
        };
        let answer = describe(emulator, &query).await;
        collected.extend(offerings(&answer, case));
        assert!(
            collected.len() <= expected_catalog_size(),
            "case {case}: the walk reported more offerings than the catalog publishes, got {}",
            collected.len()
        );
        match answer.body["NextToken"].as_str() {
            // The last answer of a walk reports no token: there is nothing left.
            None => return collected,
            Some(next) => token = Some(next.to_owned()),
        }
    }
}

/// The offering in `catalog` with this instance type, term and payment option.
fn find<'a>(
    catalog: &'a [Value],
    instance_type: &str,
    duration: i32,
    payment_option: &str,
) -> &'a Value {
    catalog
        .iter()
        .find(|offering| {
            offering["InstanceType"].as_str() == Some(instance_type)
                && offering["Duration"].as_i64() == Some(i64::from(duration))
                && offering["PaymentOption"].as_str() == Some(payment_option)
        })
        .unwrap_or_else(|| {
            panic!(
                "the catalog publishes an offering for {instance_type} / {duration}s / \
                 {payment_option}"
            )
        })
}

/// How many offerings the catalog publishes: one per instance type the SERVICE
/// publishes — which is the one record of which instance types exist, and carries the
/// current generations the checked-in modelled value set does not list — times each
/// term, times each payment option.
fn expected_catalog_size() -> usize {
    published_instance_types().len()
        * RESERVED_INSTANCE_TERMS.len()
        * RESERVED_INSTANCE_PAYMENT_OPTION_VALUES.len()
}

#[tokio::test]
async fn catalog_spans_every_instance_type_term_and_payment_option() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "whole catalog").await;

    assert_eq!(
        catalog.len(),
        expected_catalog_size(),
        "case whole catalog: expected one offering per instance type, term and payment \
         option, got {} offerings",
        catalog.len()
    );

    let mut keys: BTreeSet<(String, i64, String)> = BTreeSet::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for offering in &catalog {
        let instance_type = string_member(offering, "InstanceType", "catalog entry");
        let payment_option = string_member(offering, "PaymentOption", &instance_type);
        let duration = offering["Duration"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {instance_type}: expected a Duration, got {offering:?}"));
        let case = format!("{instance_type} / {duration}s / {payment_option}");

        assert!(
            published_instance_types().contains(&instance_type.as_str()),
            "case {case}: InstanceType is an instance type the service publishes, got \
             {instance_type}"
        );
        assert!(
            RESERVED_INSTANCE_TERMS.contains(&i32::try_from(duration).unwrap_or_default()),
            "case {case}: Duration is one of the published terms {RESERVED_INSTANCE_TERMS:?}, \
             got {duration}"
        );
        assert_eq!(
            string_member(offering, "CurrencyCode", &case),
            RESERVED_INSTANCE_CURRENCY_CODE,
            "case {case}: expected every price quoted in \
             {RESERVED_INSTANCE_CURRENCY_CODE}, got {offering:?}"
        );
        // The reservation pays for the hours, so there is no separate usage rate.
        assert!(
            (number_member(offering, "UsagePrice", &case)).abs() < f64::EPSILON,
            "case {case}: expected UsagePrice 0, got {:?}",
            offering["UsagePrice"]
        );

        let id = string_member(offering, "ReservedInstanceOfferingId", &case);
        assert!(
            is_guid(&id),
            "case {case}: expected a GUID ReservedInstanceOfferingId, got {id}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected a ReservedInstanceOfferingId of its own, got the \
             already-published {id}"
        );
        assert!(
            keys.insert((instance_type.clone(), duration, payment_option.clone())),
            "case {case}: expected one offering per instance type, term and payment \
             option, got a second"
        );

        // How the three price members hold together is what the payment option MEANS.
        // Every offering publishes exactly one hourly recurring charge; what the
        // payment option decides is its AMOUNT.
        let fixed = number_member(offering, "FixedPrice", &case);
        let charges = recurring_charges(offering, &case);
        assert_eq!(
            charges.len(),
            1,
            "case {case}: expected the one recurring charge every offering publishes, got \
             {charges:?}"
        );
        let recurring = number_member(&charges[0], "RecurringChargeAmount", &case);
        match payment_option.as_str() {
            PAYMENT_OPTION_ALL_UPFRONT => {
                assert!(
                    fixed > 0.0 && recurring.abs() < f64::EPSILON,
                    "case {case}: paying the whole term upfront means a FixedPrice and a \
                     zero recurring charge, got FixedPrice {fixed} and {charges:?}"
                );
            }
            PAYMENT_OPTION_PARTIAL_UPFRONT => {
                assert!(
                    fixed > 0.0 && recurring > 0.0,
                    "case {case}: paying part of the term upfront means a FixedPrice AND a \
                     recurring charge, got FixedPrice {fixed} and {charges:?}"
                );
            }
            PAYMENT_OPTION_NO_UPFRONT => {
                assert!(
                    fixed.abs() < f64::EPSILON && recurring > 0.0,
                    "case {case}: paying nothing upfront means no FixedPrice and a \
                     recurring charge, got FixedPrice {fixed} and {charges:?}"
                );
            }
            other => panic!("case {case}: unexpected PaymentOption {other}"),
        }
        for charge in &charges {
            assert!(
                number_member(charge, "RecurringChargeAmount", &case) >= 0.0,
                "case {case}: a recurring charge bills nothing or more, got {charge:?}"
            );
            assert_eq!(
                string_member(charge, "RecurringChargeFrequency", &case),
                RECURRING_CHARGE_HOURLY,
                "case {case}: a reservation's recurring charge is billed per hour, got \
                 {charge:?}"
            );
        }
    }

    // Every triple the published catalog allows is offered, not merely as many
    // offerings as there are triples.
    for instance_type in published_instance_types() {
        for duration in RESERVED_INSTANCE_TERMS {
            for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
                assert!(
                    keys.contains(&(
                        instance_type.to_string(),
                        i64::from(duration),
                        (*payment_option).to_string()
                    )),
                    "case {instance_type} / {duration}s / {payment_option}: expected the \
                     catalog to publish this offering"
                );
            }
        }
    }
}

#[tokio::test]
async fn prices_are_ordered_by_term_payment_option_size_and_class() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "pricing").await;

    // One row per instance type to price, spanning the classes and the size range
    // (including sizes and classes no earlier case names).
    let rows = [
        "t3.nano.search",
        "t3.medium.search",
        "c5.large.search",
        "m5.xlarge.search",
        "or1.2xlarge.search",
        "r6gd.4xlarge.search",
        "i3.16xlarge.search",
        "ultrawarm1.large.search",
        "m5.24xlarge.search",
    ];

    for instance_type in rows {
        for duration in RESERVED_INSTANCE_TERMS {
            let all = hourly_cost(
                find(&catalog, instance_type, duration, PAYMENT_OPTION_ALL_UPFRONT),
                instance_type,
            );
            let partial = hourly_cost(
                find(
                    &catalog,
                    instance_type,
                    duration,
                    PAYMENT_OPTION_PARTIAL_UPFRONT,
                ),
                instance_type,
            );
            let none = hourly_cost(
                find(&catalog, instance_type, duration, PAYMENT_OPTION_NO_UPFRONT),
                instance_type,
            );
            assert!(
                all < partial && partial < none,
                "case {instance_type} / {duration}s: expected more upfront to cost less per \
                 hour, got all {all}, partial {partial}, none {none}"
            );
        }

        for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
            let one_year = hourly_cost(
                find(&catalog, instance_type, TERM_ONE_YEAR, payment_option),
                instance_type,
            );
            let three_years = hourly_cost(
                find(&catalog, instance_type, TERM_THREE_YEARS, payment_option),
                instance_type,
            );
            assert!(
                three_years < one_year,
                "case {instance_type} / {payment_option}: expected the longer term to cost \
                 less per hour, got one year {one_year}, three years {three_years}"
            );
        }
    }

    // A bigger size of one family costs more than a smaller one, over a table of
    // families and their published size ladders.
    let ladders = [
        ("t3", ["nano", "micro", "small", "medium"].to_vec()),
        ("m5", ["large", "xlarge", "2xlarge", "12xlarge"].to_vec()),
        ("or1", ["medium", "large", "xlarge"].to_vec()),
        ("ultrawarm1", ["medium", "large", "xlarge"].to_vec()),
    ];
    for (family, sizes) in ladders {
        for pair in sizes.windows(2) {
            let smaller = format!("{family}.{}.search", pair[0]);
            let bigger = format!("{family}.{}.search", pair[1]);
            let case = format!("{smaller} vs {bigger}");
            let smaller_cost = hourly_cost(
                find(
                    &catalog,
                    &smaller,
                    TERM_ONE_YEAR,
                    PAYMENT_OPTION_ALL_UPFRONT,
                ),
                &case,
            );
            let bigger_cost = hourly_cost(
                find(&catalog, &bigger, TERM_ONE_YEAR, PAYMENT_OPTION_ALL_UPFRONT),
                &case,
            );
            assert!(
                smaller_cost < bigger_cost,
                "case {case}: expected the bigger size to cost more per hour, got \
                 {smaller_cost} and {bigger_cost}"
            );
        }
    }

    // At one size, the classes rank burstable < compute optimized < general purpose <
    // OpenSearch optimized < memory optimized < storage optimized.
    let classes = [
        "t3.large.search",
        "c5.large.search",
        "m5.large.search",
        "or1.large.search",
        "r5.large.search",
        "i3.large.search",
    ];
    for pair in classes.windows(2) {
        let case = format!("{} vs {}", pair[0], pair[1]);
        let cheaper = hourly_cost(
            find(&catalog, pair[0], TERM_ONE_YEAR, PAYMENT_OPTION_NO_UPFRONT),
            &case,
        );
        let dearer = hourly_cost(
            find(&catalog, pair[1], TERM_ONE_YEAR, PAYMENT_OPTION_NO_UPFRONT),
            &case,
        );
        assert!(
            cheaper < dearer,
            "case {case}: expected the classes to rank as the catalog's rate table does, \
             got {cheaper} and {dearer}"
        );
    }
}

#[tokio::test]
async fn offering_id_filter_reports_exactly_that_offering() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "filter").await;

    // One row per way a filtered id differs: the first and last of the listing, one in
    // the middle, and one of each payment option and each term.
    let mut rows: Vec<Value> = vec![
        catalog[0].clone(),
        catalog[catalog.len() / 2].clone(),
        catalog[catalog.len() - 1].clone(),
    ];
    for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
        for duration in RESERVED_INSTANCE_TERMS {
            rows.push(find(&catalog, "r5.2xlarge.search", duration, payment_option).clone());
        }
    }

    for expected in rows {
        let id = string_member(&expected, "ReservedInstanceOfferingId", "filter row");
        let answer = describe(&emulator, &format!("offeringId={id}")).await;
        let filtered = offerings(&answer, &id);
        assert_eq!(
            filtered.len(),
            1,
            "case {id}: the filter names one offering, got {filtered:?}"
        );
        assert_eq!(
            filtered[0], expected,
            "case {id}: expected the offering the listing reports, got {:?}",
            filtered[0]
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {id}: one offering is the whole answer, so it carries no NextToken, got \
             {:?}",
            answer.body["NextToken"]
        );
    }

    // A well-formed id the catalog publishes nothing for names no resource, on the
    // status a missing resource is reported under rather than the shape's own
    // `@httpError`. The whole rule, over more ids, lives in
    // [`super::missing_offering_refusals`].
    for absent in [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "12345678-1234-1234-1234-123456789abc",
    ] {
        let answer = describe(&emulator, &format!("offeringId={absent}")).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {absent}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {absent}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }

    // A value that is not a GUID is a malformed request, whatever its length: the
    // right length with a non-hexadecimal group, the right length with the wrong
    // groups, and a value shorter than the modelled bound.
    for malformed in [
        "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz",
        "123456781-234-1234-1234-123456789abc",
        "not-a-guid",
    ] {
        let answer = describe(&emulator, &format!("offeringId={malformed}")).await;
        assert_eq!(
            answer.status, 400,
            "case {malformed}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {malformed}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // A token is scoped to the filter it was issued under: one that names a place in
    // the filtered listing resumes it, and one that does not — a token from the
    // unfiltered walk, say — is refused rather than served with the filter dropped.
    let filtered_id = string_member(&catalog[0], "ReservedInstanceOfferingId", "filtered token");
    let other_id = string_member(&catalog[1], "ReservedInstanceOfferingId", "filtered token");
    let resumed = describe(
        &emulator,
        &format!("offeringId={filtered_id}&nextToken={filtered_id}"),
    )
    .await;
    let resumed_page = offerings(&resumed, "token naming the filtered offering");
    assert_eq!(
        resumed_page,
        vec![catalog[0].clone()],
        "case token naming the filtered offering: expected that one offering, got \
         {resumed_page:?}"
    );
    let crossed = describe(
        &emulator,
        &format!("offeringId={filtered_id}&nextToken={other_id}"),
    )
    .await;
    assert_eq!(
        crossed.status, 400,
        "case token from another listing: expected HTTP 400, got {} {:?}",
        crossed.status, crossed.body
    );
    assert_eq!(
        error_code(&crossed),
        "ValidationException",
        "case token from another listing: expected the modelled validation refusal, got \
         {:?}",
        crossed.body
    );
}

#[tokio::test]
async fn pages_partition_the_catalog() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "paging").await;

    // One row per page size: the smallest, two that do not divide the catalog, and the
    // largest the modelled range allows.
    for page_size in [1_i32, 2, 7, 100] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let answer = describe(&emulator, &query).await;
            let page = offerings(&answer, &case);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case}: a page holds at most MaxResults offerings, got {}",
                page.len()
            );
            assert!(
                !page.is_empty(),
                "case {case}: a page the service issued a token for reports offerings, got \
                 none"
            );
            walked.extend(page);
            pages += 1;
            token = answer.body["NextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() < catalog.len() + 1,
                "case {case}: paging kept issuing tokens past the end of the catalog"
            );
        }
        assert_eq!(
            walked, catalog,
            "case {case}: expected the pages to concatenate back to the whole listing in \
             order, got {} offerings over {pages} pages",
            walked.len()
        );
        let expected_pages = catalog
            .len()
            .div_ceil(usize::try_from(page_size).unwrap_or(1));
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }

    // `MaxResults` is `@default(0)`, so a request that names no page size — or names
    // zero, which is that default — has asked for no particular one: the service answers
    // with its own default page and a token, not with the whole catalog.
    for unbounded in ["", "maxResults=0"] {
        let case = format!("unbounded {unbounded:?}");
        let answer = describe(&emulator, unbounded).await;
        let page = offerings(&answer, &case);
        assert_eq!(
            page,
            catalog[..RESERVED_INSTANCE_OFFERING_PAGE].to_vec(),
            "case {case}: expected the service's own default page, got {} offerings",
            page.len()
        );
        let resume_at = string_member(
            &catalog[RESERVED_INSTANCE_OFFERING_PAGE],
            "ReservedInstanceOfferingId",
            &case,
        );
        assert_eq!(
            answer.body["NextToken"].as_str(),
            Some(resume_at.as_str()),
            "case {case}: offerings remain, so the answer names where to resume, got {:?}",
            answer.body["NextToken"]
        );
    }

    // A page size outside the modelled bound is refused at either end; the bound
    // itself is not.
    let at_bound = describe(&emulator, "maxResults=100").await;
    assert_eq!(
        at_bound.status, 200,
        "case maxResults=100: the published bound is accepted, got {} {:?}",
        at_bound.status, at_bound.body
    );
    for out_of_bound in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
        let answer = describe(&emulator, out_of_bound).await;
        assert_eq!(
            answer.status, 400,
            "case {out_of_bound}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {out_of_bound}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // A token the operation never issued is a malformed request, not an empty page.
    for invented in [
        "nextToken=not-a-token",
        "nextToken=00000000-0000-0000-0000-000000000000",
        "maxResults=5&nextToken=ffffffff-ffff-ffff-ffff-ffffffffffff",
    ] {
        let answer = describe(&emulator, invented).await;
        assert_eq!(
            answer.status, 400,
            "case {invented}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {invented}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_changes_nothing() {
    let emulator = Emulator::new();
    let first = whole_catalog(&emulator, "first read").await;

    // Paged, filtered and refused reads, all of which only report.
    let id = string_member(&first[0], "ReservedInstanceOfferingId", "first offering");
    describe(&emulator, "maxResults=3").await;
    describe(&emulator, &format!("offeringId={id}")).await;
    describe(&emulator, "offeringId=00000000-0000-0000-0000-000000000000").await;

    let second = whole_catalog(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case repeat read: expected the same catalog the first read reported, got {} \
         offerings",
        second.len()
    );

    let store = emulator.store();
    let state = store.lock();
    assert_eq!(
        state.reserved_instance_offerings.len(),
        expected_catalog_size(),
        "case stored catalog: expected the seeded catalog untouched, got {} offerings",
        state.reserved_instance_offerings.len()
    );
    assert!(
        state.domains.is_empty(),
        "case stored domains: reporting offerings creates no domain, got {:?}",
        state.domains.keys().collect::<Vec<_>>()
    );
}
