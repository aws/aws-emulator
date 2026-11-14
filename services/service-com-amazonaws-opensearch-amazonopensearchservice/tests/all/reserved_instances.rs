// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service Reserved Instance slice:
//! `DescribeReservedInstanceOfferings`, `PurchaseReservedInstanceOffering` and
//! `DescribeReservedInstances`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the published catalog carries one offering per instance type, term and payment
//!   option — the instance types being `OpenSearchPartitionInstanceType`'s own value
//!   list and the payment options `ReservedInstancePaymentOption`'s — and every
//!   offering's prices follow from its payment option and its term;
//! * a purchase is reported back by `DescribeReservedInstances` with the terms of the
//!   offering it was bought from, over a table of purchases differing in instance
//!   type, term, payment option, instance count and reservation-name length;
//! * paging the reservations with every page size returns each record exactly once,
//!   in the one order, and a token this service did not mint is refused;
//! * the id filter names ONE reservation: a stored id reports exactly it, while a
//!   value that is not a `GUID` is refused as the malformed request it is and a
//!   well-formed id nobody holds a reservation under is the modelled not-found;
//! * a purchase is judged on its own input BEFORE anything it names is resolved:
//!   `InstanceCount` is `@range(min: 1)` and carries no default, so a count below
//!   that bound — including the count a request that names none asks for — is the
//!   modelled constraint refusal, whatever offering the request names;
//! * a reservation name is an identifier, so a purchase re-using one is refused and
//!   leaves the stored record exactly as it was;
//! * the store is bounded: instances are reserved up to the quota and the purchase
//!   past it is refused with the modelled `LimitExceededException`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::RESERVED_INSTANCE_PAYMENT_OPTION_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    published_instance_types, PAYMENT_OPTION_ALL_UPFRONT, PAYMENT_OPTION_NO_UPFRONT,
    PAYMENT_OPTION_PARTIAL_UPFRONT, RECURRING_CHARGE_HOURLY, RESERVATION_STATE_ACTIVE,
    RESERVATION_STATE_PAYMENT_PENDING, RESERVED_INSTANCE_CURRENCY_CODE, RESERVED_INSTANCE_QUOTA,
    RESERVED_INSTANCE_TERMS,
};

use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// How close two money amounts have to be to count as equal: a JSON number is an
/// `f64`, and a price is rounded to cents or to a hundredth of a cent.
const EPSILON: f64 = 1e-9;

/// `DescribeReservedInstanceOfferings`: `GET /opensearch/reservedInstanceOfferings`.
async fn offerings(emulator: &Emulator, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/reservedInstanceOfferings{query}"),
            None,
        )
        .await
}

/// `PurchaseReservedInstanceOffering`:
/// `POST /opensearch/purchaseReservedInstanceOffering`.
pub(crate) async fn purchase(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/purchaseReservedInstanceOffering"),
            Some(body),
        )
        .await
}

/// `DescribeReservedInstances`: `GET /opensearch/reservedInstances`.
pub(crate) async fn reservations(emulator: &Emulator, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/reservedInstances{query}"),
            None,
        )
        .await
}

/// The `member` array of an answer, with the case named on failure.
fn array(answer: &Answer, member: &str, case: &str) -> Vec<Value> {
    answer.body[member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a {member} array, got {:?} (status {})",
                answer.body, answer.status
            )
        })
        .clone()
}

/// The `member` string of a structure, with the case named on failure.
fn text(value: &Value, member: &str, case: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a {member} string, got {value:?}"))
        .to_string()
}

/// The `member` number of a structure, with the case named on failure.
fn number(value: &Value, member: &str, case: &str) -> f64 {
    value[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a {member} number, got {value:?}"))
}

/// The recurring charge of an offering or a reservation: its hourly amount. Every
/// offering publishes one, so this is `None` only where the member carries no charge
/// at all.
fn recurring_charge(value: &Value, case: &str) -> Option<f64> {
    let charges = value["RecurringCharges"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a RecurringCharges array, got {value:?}"));
    assert!(
        charges.len() <= 1,
        "case {case}: expected at most one recurring charge, got {charges:?}"
    );
    let charge = charges.first()?;
    assert_eq!(
        text(charge, "RecurringChargeFrequency", case),
        RECURRING_CHARGE_HOURLY,
        "case {case}: expected an hourly recurring charge, got {charge:?}"
    );
    Some(number(charge, "RecurringChargeAmount", case))
}

/// Every published offering, keyed by instance type, term in seconds and payment
/// option — the three attributes that make an offering.
///
/// The catalog holds more offerings than one page carries, so this walks the
/// continuation tokens the answers hand out: a reader that stops at the first answer
/// sees only part of the catalog.
async fn fetch_catalog(emulator: &Emulator) -> BTreeMap<(String, i64, String), Value> {
    let mut catalog: BTreeMap<(String, i64, String), Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let query = match token.as_deref() {
            None => String::new(),
            Some(token) => format!("?nextToken={token}"),
        };
        let answer = offerings(emulator, &query).await;
        assert_eq!(
            answer.status, 200,
            "the whole catalog: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let next = answer.body["NextToken"].as_str().map(str::to_owned);
        for offering in array(&answer, "ReservedInstanceOfferings", "the whole catalog") {
            let key = (
                text(&offering, "InstanceType", "the whole catalog"),
                offering["Duration"]
                    .as_i64()
                    .expect("every offering publishes a Duration"),
                text(&offering, "PaymentOption", "the whole catalog"),
            );
            catalog.insert(key, offering);
        }
        match next {
            // The last answer of a walk reports no token: there is nothing left.
            None => return catalog,
            Some(next) => token = Some(next),
        }
    }
}

/// The offering with these attributes, from the published catalog.
fn offering<'a>(
    catalog: &'a BTreeMap<(String, i64, String), Value>,
    instance_type: &str,
    duration: i32,
    payment_option: &str,
) -> &'a Value {
    catalog
        .get(&(
            instance_type.to_string(),
            i64::from(duration),
            payment_option.to_string(),
        ))
        .unwrap_or_else(|| {
            panic!("the catalog publishes {instance_type} / {duration}s / {payment_option}")
        })
}

/// One purchase to make: the offering's attributes, the `ReservationName` to send
/// and the `InstanceCount`.
struct Row {
    instance_type: &'static str,
    duration: i32,
    payment_option: &'static str,
    reservation_name: String,
    instance_count: i32,
}

/// The purchase table: one row per way an accepted purchase differs from another —
/// the instance type (a burstable, a general-purpose, a memory-optimised, an
/// UltraWarm and an OR1 type, spanning the families
/// `OpenSearchPartitionInstanceType` names), both published terms, all three payment
/// options, a count at `InstanceCount`'s published minimum and several above it, and
/// `ReservationName` at each of its published length bounds
/// (`@length(min: 5, max: 64)`).
fn purchase_rows() -> Vec<Row> {
    vec![
        Row {
            instance_type: "t3.small.search",
            duration: RESERVED_INSTANCE_TERMS[0],
            payment_option: PAYMENT_OPTION_NO_UPFRONT,
            // The shortest name the constraint allows.
            reservation_name: "ri-01".to_string(),
            // The smallest count `InstanceCount`'s `@range(min: 1)` allows.
            instance_count: 1,
        },
        Row {
            instance_type: "m5.xlarge.search",
            duration: RESERVED_INSTANCE_TERMS[1],
            payment_option: PAYMENT_OPTION_ALL_UPFRONT,
            reservation_name: "search-tier-prod".to_string(),
            instance_count: 1,
        },
        Row {
            instance_type: "r6g.large.search",
            duration: RESERVED_INSTANCE_TERMS[0],
            payment_option: PAYMENT_OPTION_PARTIAL_UPFRONT,
            instance_count: 6,
            // The longest name the constraint allows.
            reservation_name: "n".repeat(64),
        },
        Row {
            instance_type: "ultrawarm1.medium.search",
            duration: RESERVED_INSTANCE_TERMS[1],
            payment_option: PAYMENT_OPTION_NO_UPFRONT,
            reservation_name: "warm tier 2026".to_string(),
            instance_count: 3,
        },
        Row {
            instance_type: "or1.2xlarge.search",
            duration: RESERVED_INSTANCE_TERMS[0],
            payment_option: PAYMENT_OPTION_ALL_UPFRONT,
            reservation_name: "OR1_indexing".to_string(),
            instance_count: 2,
        },
    ]
}

/// The body a row purchases with.
fn purchase_body(row: &Row, offering_id: &str) -> Value {
    json!({
        "ReservedInstanceOfferingId": offering_id,
        "ReservationName": row.reservation_name,
        "InstanceCount": row.instance_count,
    })
}

#[tokio::test]
async fn the_catalog_carries_one_offering_per_type_term_and_payment_option() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;

    // The instance types the catalog prices are the ones the SERVICE publishes
    // (`published_instance_types`, the one record of which instance types exist — it
    // carries the current generations the checked-in
    // `OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES` snapshot does not list), so the
    // catalog is checked against that catalog rather than against a list written
    // beside it.
    for instance_type in published_instance_types() {
        let case = format!("instance type {instance_type}");
        for duration in RESERVED_INSTANCE_TERMS {
            for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
                let case = format!("{case} / {duration}s / {payment_option}");
                let published = offering(&catalog, instance_type, duration, payment_option);
                assert_eq!(
                    text(published, "CurrencyCode", &case),
                    RESERVED_INSTANCE_CURRENCY_CODE,
                    "case {case}: expected the catalog's currency, got {published:?}"
                );
                // `UsagePrice` is the hourly rate charged for USING the reserved
                // capacity, which a reservation has already paid for: its hourly
                // charge is its recurring charge.
                assert!(
                    number(published, "UsagePrice", &case).abs() < EPSILON,
                    "case {case}: expected a zero UsagePrice, got {published:?}"
                );
                let fixed_price = number(published, "FixedPrice", &case);
                let recurring = recurring_charge(published, &case);
                // The rule the three payment options differ by: an all-upfront
                // offering charges the whole term up front and nothing hourly, a
                // no-upfront one charges nothing up front and everything hourly,
                // and a partial-upfront one charges both.
                match *payment_option {
                    PAYMENT_OPTION_ALL_UPFRONT => {
                        assert!(
                            fixed_price > 0.0
                                && recurring.is_some_and(|amount| amount.abs() < EPSILON),
                            "case {case}: expected a fixed price and a zero recurring charge, \
                             got {fixed_price} and {recurring:?}"
                        );
                    }
                    PAYMENT_OPTION_NO_UPFRONT => {
                        assert!(
                            fixed_price.abs() < EPSILON
                                && recurring.is_some_and(|amount| amount > 0.0),
                            "case {case}: expected no fixed price and a recurring charge, got \
                             {fixed_price} and {recurring:?}"
                        );
                    }
                    _ => {
                        assert!(
                            fixed_price > 0.0 && recurring.is_some_and(|amount| amount > 0.0),
                            "case {case}: expected both a fixed price and a recurring charge, got \
                             {fixed_price} and {recurring:?}"
                        );
                    }
                }
            }
        }
    }

    assert_eq!(
        catalog.len(),
        published_instance_types().len()
            * RESERVED_INSTANCE_TERMS.len()
            * RESERVED_INSTANCE_PAYMENT_OPTION_VALUES.len(),
        "the catalog holds exactly one offering per type, term and payment option, got {}",
        catalog.len()
    );

    // Every offering id is distinct, and an id is derived from the attributes that
    // make the offering, so a second read of the catalog reports the same ids.
    let ids: BTreeSet<String> = catalog
        .values()
        .map(|offering| text(offering, "ReservedInstanceOfferingId", "offering ids"))
        .collect();
    assert_eq!(
        ids.len(),
        catalog.len(),
        "every offering carries its own id, got {} ids for {} offerings",
        ids.len(),
        catalog.len()
    );
    let again = fetch_catalog(&emulator).await;
    assert_eq!(
        again, catalog,
        "the published catalog is the same on every read"
    );
}

#[tokio::test]
async fn a_longer_term_and_more_money_up_front_discount_more() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;

    // The ordering a reservation is bought for, over a sample of types spanning the
    // families: the effective hourly cost of a term falls as the term lengthens and
    // as more of it is paid up front.
    for instance_type in [
        "t3.medium.search",
        "m5.large.search",
        "r5.2xlarge.search",
        "c6g.xlarge.search",
        "or1.medium.search",
    ] {
        let hourly = |duration: i32, payment_option: &str| -> f64 {
            let case = format!("{instance_type} / {duration}s / {payment_option}");
            let published = offering(&catalog, instance_type, duration, payment_option);
            let hours = f64::from(duration) / 3600.0;
            let recurring = recurring_charge(published, &case).unwrap_or(0.0);
            number(published, "FixedPrice", &case) / hours + recurring
        };
        for duration in RESERVED_INSTANCE_TERMS {
            let case = format!("{instance_type} / {duration}s");
            let all = hourly(duration, PAYMENT_OPTION_ALL_UPFRONT);
            let partial = hourly(duration, PAYMENT_OPTION_PARTIAL_UPFRONT);
            let none = hourly(duration, PAYMENT_OPTION_NO_UPFRONT);
            assert!(
                all < partial && partial < none,
                "case {case}: expected all-upfront < partial-upfront < no-upfront, got {all}, \
                 {partial} and {none}"
            );
        }
        for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
            let case = format!("{instance_type} / {payment_option}");
            let short = hourly(RESERVED_INSTANCE_TERMS[0], payment_option);
            let long = hourly(RESERVED_INSTANCE_TERMS[1], payment_option);
            assert!(
                long < short,
                "case {case}: expected the three-year term to cost less per hour than the \
                 one-year term, got {long} and {short}"
            );
        }
    }
}

#[tokio::test]
async fn a_purchase_is_reported_back_with_its_offering_terms() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;

    let mut billing_ids: BTreeSet<i64> = BTreeSet::new();
    let mut purchased: Vec<(String, Row)> = Vec::new();

    for row in purchase_rows() {
        let case = format!(
            "{} / {}s / {} x{}",
            row.instance_type, row.duration, row.payment_option, row.instance_count
        );
        let published = offering(&catalog, row.instance_type, row.duration, row.payment_option);
        let offering_id = text(published, "ReservedInstanceOfferingId", &case);

        let bought = purchase(&emulator, purchase_body(&row, &offering_id)).await;
        assert_eq!(
            bought.status, 200,
            "case {case}: expected HTTP 200, got {} {:?}",
            bought.status, bought.body
        );
        assert_eq!(
            text(&bought.body, "ReservationName", &case),
            row.reservation_name,
            "case {case}: the purchase answers with the name it was given"
        );
        let reserved_instance_id = text(&bought.body, "ReservedInstanceId", &case);

        // Read the record back through the describe, by the id the purchase minted.
        let answer = reservations(
            &emulator,
            &format!("?reservationId={reserved_instance_id}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let reported = array(&answer, "ReservedInstances", &case);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: the id filter names one reservation, got {reported:?}"
        );
        let reservation = &reported[0];

        assert_eq!(
            text(reservation, "ReservedInstanceId", &case),
            reserved_instance_id,
            "case {case}: the reported id is the one the purchase minted"
        );
        assert_eq!(
            text(reservation, "ReservationName", &case),
            row.reservation_name,
            "case {case}: the reported name is the one the purchase was given"
        );
        assert_eq!(
            text(reservation, "ReservedInstanceOfferingId", &case),
            offering_id,
            "case {case}: the reservation names the offering it was bought from"
        );
        assert_eq!(
            text(reservation, "InstanceType", &case),
            row.instance_type,
            "case {case}: the reservation carries the offering's instance type"
        );
        assert_eq!(
            reservation["Duration"].as_i64(),
            Some(i64::from(row.duration)),
            "case {case}: the reservation carries the offering's term, got {reservation:?}"
        );
        assert_eq!(
            text(reservation, "PaymentOption", &case),
            row.payment_option,
            "case {case}: the reservation carries the offering's payment option"
        );
        assert_eq!(
            text(reservation, "CurrencyCode", &case),
            text(published, "CurrencyCode", &case),
            "case {case}: the reservation carries the offering's currency"
        );
        for member in ["FixedPrice", "UsagePrice"] {
            assert!(
                (number(reservation, member, &case) - number(published, member, &case)).abs()
                    < EPSILON,
                "case {case}: expected the offering's {member}, got {reservation:?}"
            );
        }
        assert_eq!(
            recurring_charge(reservation, &case),
            recurring_charge(published, &case),
            "case {case}: the reservation carries the offering's recurring charge"
        );
        assert_eq!(
            reservation["InstanceCount"].as_i64(),
            Some(i64::from(row.instance_count)),
            "case {case}: the reservation counts the instances the purchase asked for, got \
             {reservation:?}"
        );
        assert!(
            reservation["StartTime"].is_number(),
            "case {case}: a purchased reservation is dated, got {:?}",
            reservation["StartTime"]
        );
        // A purchase is `payment-pending` until the clock makes its payment due, so
        // a caller reading straight after the purchase still observes it.
        assert_eq!(
            text(reservation, "State", &case),
            RESERVATION_STATE_PAYMENT_PENDING,
            "case {case}: a fresh purchase is payment-pending"
        );
        let billing_id = reservation["BillingSubscriptionId"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: expected a BillingSubscriptionId"));
        assert!(
            billing_ids.insert(billing_id),
            "case {case}: no two reservations share a billing subscription id, {billing_id} \
             repeated"
        );

        purchased.push((reserved_instance_id, row));
    }

    // Once the settlement span has passed, every reservation is reported `active` —
    // the transition the clock owed it, materialised by the read rather than by a
    // sleep inside the service.
    tokio::time::sleep(SETTLE_WAIT).await;
    let answer = reservations(&emulator, "").await;
    let reported = array(&answer, "ReservedInstances", "after settling");
    assert_eq!(
        reported.len(),
        purchased.len(),
        "after settling: the describe reports every purchased reservation, got {reported:?}"
    );
    for reservation in &reported {
        let case = text(reservation, "ReservedInstanceId", "after settling");
        assert_eq!(
            text(reservation, "State", &case),
            RESERVATION_STATE_ACTIVE,
            "case {case}: a settled reservation is active"
        );
    }
    // Reading does not change what a later caller sees: the same ids, in the same
    // order, on a second read.
    let again = reservations(&emulator, "").await;
    assert_eq!(
        array(&again, "ReservedInstances", "second read"),
        reported,
        "a read leaves the reported set exactly as it was"
    );
}

#[tokio::test]
async fn every_page_size_returns_each_reservation_exactly_once() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;
    let rows = purchase_rows();

    let mut expected: Vec<String> = Vec::new();
    for row in &rows {
        let case = format!("purchase {}", row.reservation_name);
        let offering_id = text(
            offering(&catalog, row.instance_type, row.duration, row.payment_option),
            "ReservedInstanceOfferingId",
            &case,
        );
        let bought = purchase(&emulator, purchase_body(row, &offering_id)).await;
        assert_eq!(
            bought.status, 200,
            "case {case}: expected HTTP 200, got {} {:?}",
            bought.status, bought.body
        );
        expected.push(text(&bought.body, "ReservedInstanceId", &case));
    }
    // The describe's order is the store's own, which is its ids sorted; the test
    // asserts the SET and the order, not an order it chose.
    expected.sort();

    // Every page size from one record per page up to more than the store holds, plus
    // the uncapped default: the pages concatenated are the whole set, once each.
    for max_results in [None, Some(1), Some(2), Some(3), Some(5), Some(100)] {
        let case = format!("maxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = String::from("?");
            if let Some(size) = max_results {
                query.push_str(&format!("maxResults={size}&"));
            }
            if let Some(next) = token.as_deref() {
                query.push_str(&format!("nextToken={next}&"));
            }
            let answer = reservations(&emulator, query.trim_end_matches(['?', '&'])).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = array(&answer, "ReservedInstances", &case);
            if let Some(size) = max_results {
                assert!(
                    page.len() <= usize::try_from(size).unwrap_or(0),
                    "case {case}: a page holds no more than the maximum asked for, got {}",
                    page.len()
                );
            }
            for reservation in &page {
                seen.push(text(reservation, "ReservedInstanceId", &case));
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging terminates, got {pages} pages"
            );
            match answer.body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: the pages concatenated are every reservation, once each and in order"
        );
    }

    // A `NextToken` is only ever a value this operation handed out, so one that
    // names no reservation of the listing — whatever shape it has — is refused
    // rather than served as an empty page. A negative `MaxResults` cannot be a
    // number of results, and one past the modelled maximum is refused too.
    for (query, case) in [
        ("?nextToken=not-a-token", "a token nobody minted"),
        (
            "?nextToken=00000000-0000-0000-0000-000000000000",
            "a GUID-shaped token naming no reservation",
        ),
        ("?maxResults=-1", "a negative maximum"),
        ("?maxResults=101", "a maximum past the modelled bound"),
    ] {
        let answer = reservations(&emulator, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // The same paging rule over the published catalog, which is the other
    // `MaxResults`/`NextToken` answer of this slice.
    let first = offerings(&emulator, "?maxResults=4").await;
    let page = array(&first, "ReservedInstanceOfferings", "catalog page one");
    assert_eq!(
        page.len(),
        4,
        "catalog page one: expected four offerings, got {page:?}"
    );
    let token = first.body["NextToken"]
        .as_str()
        .expect("a capped catalog page carries the token the next one resumes from");
    let second = offerings(&emulator, &format!("?maxResults=4&nextToken={token}")).await;
    let next_page = array(&second, "ReservedInstanceOfferings", "catalog page two");
    assert!(
        next_page
            .iter()
            .all(|offering| !page.contains(offering)),
        "catalog page two: a later page repeats no offering of an earlier one"
    );
    let refused = offerings(&emulator, "?nextToken=zzz").await;
    assert_eq!(
        refused.status, 400,
        "a catalog token nobody minted: expected HTTP 400, got {} {:?}",
        refused.status, refused.body
    );
}

#[tokio::test]
async fn an_id_filter_names_one_resource_and_an_unknown_one_is_refused() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;
    let row = &purchase_rows()[0];
    let offering_id = text(
        offering(&catalog, row.instance_type, row.duration, row.payment_option),
        "ReservedInstanceOfferingId",
        "the filter case",
    );
    let bought = purchase(&emulator, purchase_body(row, &offering_id)).await;
    let reserved_instance_id = text(&bought.body, "ReservedInstanceId", "the filter case");

    // The offering id filter narrows the catalog to exactly the offering it names.
    let answer = offerings(&emulator, &format!("?offeringId={offering_id}")).await;
    let page = array(&answer, "ReservedInstanceOfferings", "the offering filter");
    assert_eq!(
        page.len(),
        1,
        "the offering filter: expected exactly the offering named, got {page:?}"
    );
    assert_eq!(
        text(&page[0], "ReservedInstanceOfferingId", "the offering filter"),
        offering_id,
        "the offering filter: expected the offering named"
    );

    // A well-formed `GUID` that names nothing addresses no resource, so both reads
    // answer the modelled not-found: the reservations listing for an id this account
    // holds no reservation under, the catalog listing for an offering id it
    // publishes nothing for. Over several such ids.
    for unknown in [
        "00000000-0000-0000-0000-000000000000",
        "deadbeef-dead-beef-dead-beefdeadbeef",
        "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9",
    ] {
        let case = format!("DescribeReservedInstances with {unknown}");
        let answer = reservations(&emulator, &format!("?reservationId={unknown}")).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected the missing resource's HTTP {RECORDED_NOT_FOUND_STATUS}, got \
             {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some("Reservation id not found"),
            "case {case}: expected the service's wording for a reservation id that resolves to \
             nothing, got {:?}",
            answer.body
        );

        let case = format!("DescribeReservedInstanceOfferings with {unknown}");
        let answer = offerings(&emulator, &format!("?offeringId={unknown}")).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected HTTP {RECORDED_NOT_FOUND_STATUS} for an offering the catalog \
             does not publish, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found, got {:?}",
            answer.body
        );

        // A purchase from an offering the catalog does not publish is the same miss:
        // the id is well formed and names no offering, so it is the not-found rather
        // than a malformed request.
        let case = format!("PurchaseReservedInstanceOffering from {unknown}");
        let answer = purchase(
            &emulator,
            json!({
                "ReservedInstanceOfferingId": unknown,
                "ReservationName": "no-such-offering",
                "InstanceCount": 1,
            }),
        )
        .await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected HTTP {RECORDED_NOT_FOUND_STATUS} for an offering the catalog \
             does not publish, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found, got {:?}",
            answer.body
        );
    }

    // A value of the right LENGTH that is not a `GUID` is refused: the request
    // boundary enforces `@length(36, 36)` but not the pattern, so the handler does.
    for malformed in [
        "not-a-guid-not-a-guid-not-a-guid-not",
        "000000000000000000000000000000000000",
        "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz",
    ] {
        for (query, operation) in [
            (
                format!("?reservationId={malformed}"),
                "DescribeReservedInstances",
            ),
            (
                format!("?offeringId={malformed}"),
                "DescribeReservedInstanceOfferings",
            ),
        ] {
            let case = format!("{operation} with {malformed}");
            let answer = if operation == "DescribeReservedInstances" {
                reservations(&emulator, &query).await
            } else {
                offerings(&emulator, &query).await
            };
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected the modelled validation refusal, got {:?}",
                answer.body
            );
        }
    }

    // The stored reservation is untouched by every refusal above.
    let answer = reservations(&emulator, "").await;
    let reported = array(&answer, "ReservedInstances", "after the refusals");
    assert_eq!(
        reported.len(),
        1,
        "after the refusals: the store still holds exactly the one purchase, got {reported:?}"
    );
    assert_eq!(
        text(&reported[0], "ReservedInstanceId", "after the refusals"),
        reserved_instance_id,
        "after the refusals: the stored reservation is the one that was bought"
    );
}

#[tokio::test]
async fn a_reused_reservation_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;
    let rows = purchase_rows();
    let first = &rows[0];
    let offering_id = text(
        offering(
            &catalog,
            first.instance_type,
            first.duration,
            first.payment_option,
        ),
        "ReservedInstanceOfferingId",
        "the first purchase",
    );
    let bought = purchase(&emulator, purchase_body(first, &offering_id)).await;
    let stored_id = text(&bought.body, "ReservedInstanceId", "the first purchase");
    let before = array(
        &reservations(&emulator, "").await,
        "ReservedInstances",
        "before the reuse",
    );

    // Re-using the name is refused whatever ELSE the second purchase differs in:
    // the same offering, another offering, another count.
    for (row, case) in [
        (
            Row {
                instance_type: first.instance_type,
                duration: first.duration,
                payment_option: first.payment_option,
                reservation_name: first.reservation_name.clone(),
                instance_count: first.instance_count,
            },
            "the same offering again",
        ),
        (
            Row {
                instance_type: rows[1].instance_type,
                duration: rows[1].duration,
                payment_option: rows[1].payment_option,
                reservation_name: first.reservation_name.clone(),
                instance_count: 4,
            },
            "another offering under the same name",
        ),
    ] {
        let offering_id = text(
            offering(&catalog, row.instance_type, row.duration, row.payment_option),
            "ReservedInstanceOfferingId",
            case,
        );
        let answer = purchase(&emulator, purchase_body(&row, &offering_id)).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected the modelled already-exists' HTTP 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceAlreadyExistsException",
            "case {case}: expected the modelled already-exists, got {:?}",
            answer.body
        );
    }

    let after = array(
        &reservations(&emulator, "").await,
        "ReservedInstances",
        "after the reuse",
    );
    assert_eq!(
        after.len(),
        1,
        "after the reuse: the refused purchases stored nothing, got {after:?}"
    );
    assert_eq!(
        text(&after[0], "ReservedInstanceId", "after the reuse"),
        stored_id,
        "after the reuse: the stored reservation is still the first one"
    );
    assert_eq!(
        after, before,
        "after the reuse: the stored record is exactly as it was"
    );

    // A DIFFERENT name on the same offering is accepted, so it is the name that was
    // refused rather than the offering.
    let another = Row {
        instance_type: first.instance_type,
        duration: first.duration,
        payment_option: first.payment_option,
        reservation_name: format!("{}-two", first.reservation_name),
        instance_count: first.instance_count,
    };
    let accepted = purchase(&emulator, purchase_body(&another, &offering_id)).await;
    assert_eq!(
        accepted.status, 200,
        "a second name on the same offering: expected HTTP 200, got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn the_reserved_instance_store_is_bounded_by_the_quota() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;
    let offering_id = text(
        offering(
            &catalog,
            "m5.large.search",
            RESERVED_INSTANCE_TERMS[0],
            PAYMENT_OPTION_NO_UPFRONT,
        ),
        "ReservedInstanceOfferingId",
        "the quota case",
    );
    let quota = i32::try_from(RESERVED_INSTANCE_QUOTA).expect("the quota fits an instance count");

    // One purchase may not reserve more instances than the quota, however large it
    // asks for.
    let answer = purchase(
        &emulator,
        json!({
            "ReservedInstanceOfferingId": offering_id,
            "ReservationName": "too-many-at-once",
            "InstanceCount": quota + 1,
        }),
    )
    .await;
    assert_eq!(
        error_code(&answer),
        "LimitExceededException",
        "one purchase past the quota: expected the modelled limit refusal, got {:?}",
        answer.body
    );

    // Nor may several purchases together: the instances already reserved count
    // towards the bound.
    let half = quota / 2;
    for index in 0..2 {
        let answer = purchase(
            &emulator,
            json!({
                "ReservedInstanceOfferingId": offering_id,
                "ReservationName": format!("half-{index}"),
                "InstanceCount": half,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "purchase {index} up to the quota: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    let answer = purchase(
        &emulator,
        json!({
            "ReservedInstanceOfferingId": offering_id,
            "ReservationName": "one-past-the-quota",
            "InstanceCount": 1,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 409,
        "one instance past the quota: expected the modelled limit refusal's HTTP 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "LimitExceededException",
        "one instance past the quota: expected the modelled limit refusal, got {:?}",
        answer.body
    );

    let reported = array(
        &reservations(&emulator, "?maxResults=100").await,
        "ReservedInstances",
        "at the quota",
    );
    let reserved: i64 = reported
        .iter()
        .map(|reservation| {
            reservation["InstanceCount"]
                .as_i64()
                .expect("every reservation counts its instances")
        })
        .sum();
    assert_eq!(
        reserved,
        i64::from(half) * 2,
        "at the quota: the store holds exactly the instances the accepted purchases reserved, \
         got {reserved}"
    );
}

/// A `GUID` the published catalog holds no offering for: the catalog is keyed by the
/// attributes that make an offering, so an id derived from none of them is not one
/// of its keys.
const UNPUBLISHED_OFFERING: &str = "11111111-2222-3333-4444-555555555555";

#[tokio::test]
async fn a_count_below_the_modelled_range_is_refused_before_the_offering_is_resolved() {
    let emulator = Emulator::new();
    let catalog = fetch_catalog(&emulator).await;
    let published = text(
        offering(
            &catalog,
            "m5.large.search",
            RESERVED_INSTANCE_TERMS[0],
            PAYMENT_OPTION_NO_UPFRONT,
        ),
        "ReservedInstanceOfferingId",
        "the range case",
    );

    // `InstanceCount` is `@range(min: 1)` and carries no `@default`, so the counts
    // below the bound span: the count a request that names none asks for, the bound
    // minus one, a negative count and the smallest integer the member can carry.
    // Each is sent BOTH against an offering the catalog publishes and against one it
    // does not, because the input is judged before anything it names is resolved:
    // every row must be answered about the member, never about the offering.
    for offering_id in [published.as_str(), UNPUBLISHED_OFFERING] {
        for (count, shown) in [
            (None, "0"),
            (Some(0), "0"),
            (Some(-1), "-1"),
            (Some(i32::MIN), "-2147483648"),
        ] {
            let case = format!("InstanceCount {count:?} on offering {offering_id}");
            let mut body = json!({
                "ReservedInstanceOfferingId": offering_id,
                "ReservationName": "range-rule",
            });
            if let Some(count) = count {
                body["InstanceCount"] = json!(count);
            }
            let answer = purchase(&emulator, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected the modelled constraint refusal's HTTP 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected the modelled constraint refusal, got {:?}",
                answer.body
            );
            let message = answer.body["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            assert_eq!(
                message,
                format!(
                    "1 validation error detected: Value '{shown}' at 'instanceCount' failed to \
                     satisfy constraint: Member must have value greater than or equal to 1"
                ),
                "case {case}: expected the constraint refusal to name the member and its bound, \
                 got {message:?}"
            );
        }
    }

    // The bound itself is accepted, so it is the value that was refused above and
    // not the member: one instance on a published offering is a purchase.
    let accepted = purchase(
        &emulator,
        json!({
            "ReservedInstanceOfferingId": published,
            "ReservationName": "range-rule-accepted",
            "InstanceCount": 1,
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the published minimum: expected HTTP 200, got {} {:?}",
        accepted.status, accepted.body
    );

    // Nothing but that one purchase was stored: a refused request reserves nothing.
    let reported = array(
        &reservations(&emulator, "").await,
        "ReservedInstances",
        "after the range refusals",
    );
    assert_eq!(
        reported.len(),
        1,
        "after the range refusals: the store holds only the accepted purchase, got {reported:?}"
    );
}
