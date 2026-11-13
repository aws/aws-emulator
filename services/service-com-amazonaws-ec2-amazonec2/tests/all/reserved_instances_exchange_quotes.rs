// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetReservedInstancesExchangeQuote`, driven over the wire the service
//! actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. THE VALUES ARE COMPUTED from the purchases and the offerings the request names: each
//!    side's `hourlyPrice` is its usage price for every reservation it holds, its
//!    `remainingUpfrontValue` is the part of its purchase price the term still to run has not
//!    used up, and its `remainingTotalValue` is "the sum of remainingUpfrontValue +
//!    hourlyPrice * number of hours remaining" (`ReservationValue$RemainingTotalValue`); each
//!    rollup is the sum of the items it rolls up, and `paymentDue` is "the total true upfront
//!    charge for the exchange" — what the targets cost upfront, less what the purchases being
//!    given up have not used up, never below zero;
//! 2. THE DIVISION a quote reports is the one an exchange makes: the reservations are divided
//!    equally over the target configurations with the remainder going to the earlier ones, so
//!    `AcceptReservedInstancesExchangeQuote` mints exactly the counts the quote reported;
//! 3. AN EXCHANGE THAT CANNOT BE PERFORMED IS REPORTED, NOT REFUSED: "if the exchange cannot
//!    be performed, the reason is returned in the response" — `isValidExchange` is `false`
//!    with a `validationFailureReason`, and the call still answers HTTP 200;
//! 4. ONLY A VALUE THAT NAMES NOTHING IS AN ERROR, and the refusal is the one EC2 words for
//!    the value at fault: a `reservedInstanceId` outside the Reserved Instances id form is
//!    `InvalidReservedInstancesID.NotFound` with "The reserved-instances ID '<id>' is
//!    malformed", a well-formed one this account holds no purchase under is
//!    `InvalidReservedInstanceId`, and an `offeringId` this service publishes no offering
//!    under is `InvalidReservedInstancesOfferingId`;
//! 5. A QUOTE IS A READ: it writes nothing — every purchase is where it was, twice over —
//!    and `DryRun` answers the permission probe ahead of every refusal a `reservedInstanceId`
//!    would otherwise draw;
//!
//! Each table row is a distinct input along what its rule generalises over — how many
//! reservations a purchase holds, what they cost, how many target configurations they are
//! divided over, which condition makes an exchange impossible, and every way an id can fail
//! to name a purchase — so no row is the specification.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_RESERVED_INSTANCE_ID,
    INVALID_RESERVED_INSTANCES_ID_NOT_FOUND, INVALID_RESERVED_INSTANCES_OFFERING_ID,
    MAX_ECHOED_RESOURCE_ID_CHARS, RESERVED_INSTANCE_STATE_ACTIVE, RESERVED_INSTANCE_STATE_RETIRED,
    ReservedInstancesOfferingTerms, ReservedInstancesSpec, encode_reserved_instances_offering_id,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, indexed, member};
use super::wire::{Param, param};

/// The Region every call here is signed for: a Reserved Instance is reported in a zone of the
/// Region the request was signed for.
const REGION: &str = "us-east-1";

/// The form-field prefix ec2Query flattens `TargetConfigurations` into: the member's
/// `@xmlName`
/// (`smithy_query(query="operation",operation="GetReservedInstancesExchangeQuote")` gives
/// `TargetConfiguration`).
const TARGETS: &str = "TargetConfiguration";

/// The form-field prefix `ReservedInstanceIds` is flattened into, from the same place.
const IDS: &str = "ReservedInstanceId";

/// A one-year term, in seconds — the units `ReservedInstances$Duration` is reported in.
const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// The offering class a purchase and an offering must carry to be exchanged: the
/// `OfferingClassType` enum's `convertible` member.
const CONVERTIBLE: &str = "convertible";

/// The other member of that enum, which an exchange cannot act on.
const STANDARD: &str = "standard";

/// The offering type every purchase and offering here carries.
const ALL_UPFRONT: &str = "All Upfront";

/// The product description every purchase and offering here carries.
const LINUX_UNIX: &str = "Linux/UNIX";

/// How close a quoted amount has to be to the amount this suite computes: the quote measures
/// the term that is still to RUN, which is a fraction of a second shorter than the term the
/// seeding gave the purchase, so a cent-exact comparison would fail on the elapsed time
/// rather than on the rule.
const AMOUNT_TOLERANCE: f64 = 1.0;

/// One Reserved Instance to seed: how many reservations it holds, the purchase price of each
/// and the hourly usage price of each.
type PurchaseRow = (i32, f32, f32);

/// One target offering: the purchase price of one of its reservations and its usage price.
type OfferingRow = (f32, f32);

/// Seed one Reserved Instance in `state` and of `offering_class`, answering its id.
fn seed_as(
    engine: &engine::InMemoryEngine,
    state: &str,
    offering_class: &str,
    row: PurchaseRow,
) -> String {
    let (instance_count, fixed_price, usage_price) = row;
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(ReservedInstancesSpec {
            instance_type: "m5.large".to_string(),
            availability_zone: Some("us-east-1a".to_string()),
            region: REGION.to_string(),
            instance_count,
            duration_seconds: ONE_YEAR_SECONDS,
            offering_class: offering_class.to_string(),
            offering_type: ALL_UPFRONT.to_string(),
            product_description: LINUX_UNIX.to_string(),
            state: state.to_string(),
            start: SystemTime::now(),
            fixed_price,
            usage_price,
        })
        .expect("the store has room for the few purchases this suite seeds")
}

/// Seed one Convertible Reserved Instance that is in effect — the one an exchange can act on.
fn seed(engine: &engine::InMemoryEngine, row: PurchaseRow) -> String {
    seed_as(engine, RESERVED_INSTANCE_STATE_ACTIVE, CONVERTIBLE, row)
}

/// One offering of `offering_class`, as the terms its id carries.
fn offering_of_class(offering_class: &str, row: OfferingRow) -> ReservedInstancesOfferingTerms {
    let (fixed_price, usage_price) = row;
    ReservedInstancesOfferingTerms {
        instance_type: "m5.large".to_string(),
        availability_zone: Some("us-east-1a".to_string()),
        product_description: LINUX_UNIX.to_string(),
        offering_class: offering_class.to_string(),
        offering_type: ALL_UPFRONT.to_string(),
        duration_seconds: ONE_YEAR_SECONDS,
        fixed_price,
        usage_price,
    }
}

/// The `ReservedInstancesOfferingId` one Convertible offering is published under.
fn offering_id(row: OfferingRow) -> String {
    encode_reserved_instances_offering_id(&offering_of_class(CONVERTIBLE, row))
}

/// The form fields for a whole `TargetConfigurations` list.
fn target_params(offering_ids: &[String]) -> Vec<Param> {
    offering_ids
        .iter()
        .enumerate()
        .map(|(index, id)| param(&format!("{TARGETS}.{}.OfferingId", index + 1), id))
        .collect()
}

/// One `GetReservedInstancesExchangeQuote` call naming `ids` and the offerings
/// `offering_ids` names.
async fn quote(
    router: &MultiServiceRouter,
    ids: &[&str],
    offering_ids: &[String],
    extra: &[Param],
) -> (u16, String) {
    let mut params = indexed(IDS, ids);
    params.extend(target_params(offering_ids));
    params.extend_from_slice(extra);
    call(
        router,
        REGION,
        "GetReservedInstancesExchangeQuote",
        &params,
    )
    .await
}

/// One quote that must be served.
async fn quoted(
    router: &MultiServiceRouter,
    case: &str,
    ids: &[&str],
    offering_ids: &[String],
) -> String {
    let (status, body) = quote(router, ids, offering_ids, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetReservedInstancesExchangeQuote must succeed, got {status}: {body}"
    );
    body
}

/// One `ReservationValue` as the three amounts it carries: `(hourlyPrice,
/// remainingUpfrontValue, remainingTotalValue)`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Value {
    hourly_price: f64,
    remaining_upfront_value: f64,
    remaining_total_value: f64,
}

/// The amount the `name` child of `node` carries.
fn amount_child(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> f64 {
    let text = super::wire::child_text(node, name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} under {:?}", node.tag_name()));
    text.parse::<f64>()
        .unwrap_or_else(|error| panic!("case {case:?}: {name} must be an amount ({error})"))
}

/// The `ReservationValue` `node` carries, read off its own three children.
fn value_of(case: &str, node: roxmltree::Node<'_, '_>) -> Value {
    Value {
        hourly_price: amount_child(case, node, "hourlyPrice"),
        remaining_upfront_value: amount_child(case, node, "remainingUpfrontValue"),
        remaining_total_value: amount_child(case, node, "remainingTotalValue"),
    }
}

/// The rollup the envelope's `name` member carries — read by the PARENT element, so a rollup
/// is never confused with one of the items it rolls up.
fn rollup(case: &str, body: &str, name: &str) -> Value {
    let document = super::wire::document(case, body);
    let node = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"));
    value_of(case, node)
}

/// Every item of the envelope's `name` set, in document order: each item's own
/// `reservationValue`, beside the `instanceCount` a target configuration carries.
fn value_set(case: &str, body: &str, name: &str) -> Vec<(Value, Option<i32>)> {
    let document = super::wire::document(case, body);
    let set = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"));
    set.children()
        .filter(|node| node.is_element())
        .map(|item| {
            let value = super::wire::child(item, "reservationValue").unwrap_or_else(|| {
                panic!("case {case:?}: expected a reservationValue in each {name} item")
            });
            let instance_count = super::wire::child(item, "targetConfiguration")
                .and_then(|target| super::wire::child_text(target, "instanceCount"))
                .and_then(|text| text.parse::<i32>().ok());
            (value_of(case, value), instance_count)
        })
        .collect()
}

/// Assert `quoted` is `expected` to within [`AMOUNT_TOLERANCE`].
fn assert_amount(case: &str, what: &str, quoted: f64, expected: f64) {
    assert!(
        (quoted - expected).abs() <= AMOUNT_TOLERANCE,
        "case {case:?}: expected {what} of {expected:.2}, got {quoted:.2}"
    );
}

/// Every Reserved Instance the store holds, as `(id, state, instance_count)`.
fn purchases(engine: &engine::InMemoryEngine) -> BTreeMap<String, (Option<String>, Option<i32>)> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .reserved_instances
        .iter()
        .map(|(id, held)| (id.clone(), (held.state.clone(), held.instance_count)))
        .collect()
}

/// RULE 1: every amount the quote reports is computed from the purchases and the offerings,
/// and each rollup is the sum of its items.
#[tokio::test]
async fn the_quote_values_are_computed_from_the_purchases_and_the_offerings() {
    // Rows differ in how many reservations the purchase holds, what they cost, whether they
    // carry an hourly usage price, and how many target configurations they are exchanged
    // into.
    let rows: [(&str, PurchaseRow, &[OfferingRow]); 4] = [
        ("one reservation into one offering", (1, 500.0, 0.0), &[(600.0, 0.0)]),
        ("two reservations into one offering", (2, 500.0, 0.0), &[(1100.0, 0.0)]),
        (
            "two reservations with an hourly price",
            (2, 400.0, 0.05),
            &[(900.0, 0.02)],
        ),
        (
            "four reservations into two offerings",
            (4, 250.0, 0.0),
            &[(600.0, 0.0), (600.0, 0.0)],
        ),
    ];

    for (case, purchase, offerings) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let held = seed(&engine, purchase);
        let offering_ids: Vec<String> = offerings.iter().copied().map(offering_id).collect();
        let body = quoted(&router, case, &[&held], &offering_ids).await;

        let (count, fixed_price, usage_price) = purchase;
        let held_hourly = f64::from(usage_price) * f64::from(count);
        let held_upfront = f64::from(fixed_price) * f64::from(count);
        let term_hours = ONE_YEAR_SECONDS as f64 / 3_600.0;

        assert_eq!(
            member(&body, "currencyCode").as_deref(),
            Some("USD"),
            "case {case:?}: expected the currency the service prices in, got {body}"
        );
        assert_eq!(
            member(&body, "isValidExchange").as_deref(),
            Some("true"),
            "case {case:?}: this exchange can be performed, got {body}"
        );
        assert_eq!(
            member(&body, "validationFailureReason"),
            None,
            "case {case:?}: a valid exchange carries no failure reason, got {body}"
        );
        assert_eq!(
            super::ec2_query::elements(&body, "reservedInstanceId"),
            vec![held.clone()],
            "case {case:?}: the quote must report the purchase the request named, got {body}"
        );

        // The HELD side: one item, carrying this purchase's own prices over the term it has
        // left — which is all of it, since the purchase was seeded just now. Read by the
        // PARENT element, so an item is never confused with a rollup.
        let held_items = value_set(case, &body, "reservedInstanceValueSet");
        assert_eq!(
            held_items.len(),
            1,
            "case {case:?}: expected one value per purchase named, got {body}"
        );
        let held_value = held_items[0].0;
        assert_amount(
            case,
            "the purchase's hourly price",
            held_value.hourly_price,
            held_hourly,
        );
        assert_amount(
            case,
            "the purchase's remaining upfront value",
            held_value.remaining_upfront_value,
            held_upfront,
        );
        assert_amount(
            case,
            "the purchase's remaining total value",
            held_value.remaining_total_value,
            held_upfront + held_hourly * term_hours,
        );

        // The TARGET side: one item per configuration, each over its own full term, and the
        // reservations divided over them.
        let target_items = value_set(case, &body, "targetConfigurationValueSet");
        assert_eq!(
            target_items.len(),
            offerings.len(),
            "case {case:?}: expected one target configuration per offering, got {body}"
        );
        let counts: Vec<i32> = target_items
            .iter()
            .map(|(_value, instance_count)| {
                instance_count.unwrap_or_else(|| {
                    panic!("case {case:?}: each target configuration reports its count: {body}")
                })
            })
            .collect();
        assert_eq!(
            counts.iter().sum::<i32>(),
            count,
            "case {case:?}: the targets must hold every reservation being exchanged, got {body}"
        );
        let mut target_upfront_total = 0.0;
        for (index, ((target_fixed, target_usage), (value, share))) in
            offerings.iter().copied().zip(&target_items).enumerate()
        {
            let share = f64::from(share.unwrap_or_default());
            let expected_hourly = f64::from(target_usage) * share;
            let expected_upfront = f64::from(target_fixed) * share;
            target_upfront_total += expected_upfront;
            assert_amount(
                case,
                &format!("target {index}'s hourly price"),
                value.hourly_price,
                expected_hourly,
            );
            assert_amount(
                case,
                &format!("target {index}'s remaining upfront value"),
                value.remaining_upfront_value,
                expected_upfront,
            );
            assert_amount(
                case,
                &format!("target {index}'s remaining total value"),
                value.remaining_total_value,
                expected_upfront + expected_hourly * term_hours,
            );
        }

        // The ROLLUPS: the sum of each side's items.
        let held_rollup = rollup(case, &body, "reservedInstanceValueRollup");
        assert_amount(
            case,
            "the held rollup's remaining upfront value",
            held_rollup.remaining_upfront_value,
            held_upfront,
        );
        assert_amount(
            case,
            "the held rollup's hourly price",
            held_rollup.hourly_price,
            held_hourly,
        );
        let target_rollup = rollup(case, &body, "targetConfigurationValueRollup");
        assert_amount(
            case,
            "the target rollup's remaining upfront value",
            target_rollup.remaining_upfront_value,
            target_upfront_total,
        );

        // "The total true upfront charge for the exchange": what the targets cost upfront,
        // less what the purchase has not used up.
        let payment_due = member(&body, "paymentDue")
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or_else(|| panic!("case {case:?}: expected a paymentDue in {body}"));
        assert_amount(
            case,
            "the payment due",
            payment_due,
            (target_upfront_total - held_upfront).max(0.0),
        );

        // "The new end date of the reservation term": the targets' term, from now.
        let expires_at = member(&body, "outputReservedInstancesWillExpireAt")
            .as_deref()
            .and_then(super::ec2_query::as_timestamp)
            .unwrap_or_else(|| panic!("case {case:?}: expected a term end in {body}"));
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs_f64();
        assert!(
            (expires_at - (now + ONE_YEAR_SECONDS as f64)).abs() < 60.0,
            "case {case:?}: expected the term to end one term from now, got {expires_at}"
        );

        assert_eq!(
            purchases(&engine).len(),
            1,
            "case {case:?}: a quote must mint no purchase"
        );
    }
}

/// RULE 2: the division a quote reports is the one the exchange makes — equal shares with
/// the remainder to the earlier configurations — and an accept of that quote mints exactly
/// those counts.
#[tokio::test]
async fn the_quote_divides_the_reservations_the_way_the_exchange_does() {
    // Rows differ in how the reservations divide: exactly, with a remainder, and one
    // configuration taking all of them.
    let rows: [(&str, i32, usize, &[i32]); 4] = [
        ("four over two", 4, 2, &[2, 2]),
        ("three over two", 3, 2, &[2, 1]),
        ("five over three", 5, 3, &[2, 2, 1]),
        ("three over one", 3, 1, &[3]),
    ];

    for (case, count, configurations, expected) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let held = seed(&engine, (count, 100.0, 0.0));
        // Each configuration must be worth at least its share of what is given up, so the
        // offerings are priced above the purchase.
        let offering_ids: Vec<String> = (0..configurations)
            .map(|index| offering_id((200.0 + index as f32, 0.0)))
            .collect();

        let body = quoted(&router, case, &[&held], &offering_ids).await;
        let counts: Vec<i32> = value_set(case, &body, "targetConfigurationValueSet")
            .into_iter()
            .map(|(_value, instance_count)| instance_count.unwrap_or_default())
            .collect();
        assert_eq!(
            counts, expected,
            "case {case:?}: expected the quote to divide the reservations {expected:?}, \
             got {counts:?} in {body}"
        );

        // The accept of that quote mints exactly those counts, so the quote described the
        // exchange rather than a division of its own.
        let mut params = indexed(IDS, &[held.as_str()]);
        params.extend(target_params(&offering_ids));
        let (status, body) = call(
            &router,
            REGION,
            "AcceptReservedInstancesExchangeQuote",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accept must succeed, got {status} with body {body}"
        );
        let mut minted: Vec<i32> = purchases(&engine)
            .into_iter()
            .filter(|(id, (state, _count))| {
                id != &held && state.as_deref() == Some(RESERVED_INSTANCE_STATE_ACTIVE)
            })
            .filter_map(|(_id, (_state, count))| count)
            .collect();
        minted.sort_unstable();
        let mut expected_counts = expected.to_vec();
        expected_counts.sort_unstable();
        assert_eq!(
            minted, expected_counts,
            "case {case:?}: expected the exchange to mint the counts the quote reported"
        );
    }
}

/// RULE 3: an exchange that cannot be performed is REPORTED — HTTP 200, `isValidExchange`
/// false, and the reason — rather than refused.
#[tokio::test]
async fn an_exchange_that_cannot_be_performed_is_reported_with_its_reason() {
    /// What one row seeds and asks for, and the word its reason must carry.
    struct Row {
        case: &'static str,
        state: &'static str,
        offering_class: &'static str,
        purchase: PurchaseRow,
        /// The offering class of each target, with its price.
        targets: &'static [(&'static str, f32)],
        expected: &'static str,
    }

    // Rows differ in WHICH condition makes the exchange impossible: the purchase is not in
    // effect, it is not Convertible, the targets are worth less than it, a target is not
    // Convertible, and there are more targets than reservations.
    let rows: [Row; 5] = [
        Row {
            case: "a retired purchase",
            state: RESERVED_INSTANCE_STATE_RETIRED,
            offering_class: CONVERTIBLE,
            purchase: (2, 500.0, 0.0),
            targets: &[(CONVERTIBLE, 600.0)],
            expected: "state",
        },
        Row {
            case: "a Standard purchase",
            state: RESERVED_INSTANCE_STATE_ACTIVE,
            offering_class: STANDARD,
            purchase: (2, 500.0, 0.0),
            targets: &[(CONVERTIBLE, 600.0)],
            expected: "Convertible",
        },
        Row {
            case: "targets worth less than the purchase",
            state: RESERVED_INSTANCE_STATE_ACTIVE,
            offering_class: CONVERTIBLE,
            purchase: (2, 500.0, 0.0),
            targets: &[(CONVERTIBLE, 400.0)],
            expected: "worth less",
        },
        Row {
            case: "a Standard target offering",
            state: RESERVED_INSTANCE_STATE_ACTIVE,
            offering_class: CONVERTIBLE,
            purchase: (2, 500.0, 0.0),
            targets: &[(STANDARD, 2000.0)],
            expected: "Convertible",
        },
        Row {
            case: "more targets than reservations",
            state: RESERVED_INSTANCE_STATE_ACTIVE,
            offering_class: CONVERTIBLE,
            purchase: (1, 100.0, 0.0),
            targets: &[(CONVERTIBLE, 500.0), (CONVERTIBLE, 500.0)],
            expected: "reservations",
        },
    ];

    for row in rows {
        let (router, engine) = super::ec2_query::emulator();
        let held = seed_as(&engine, row.state, row.offering_class, row.purchase);
        let offering_ids: Vec<String> = row
            .targets
            .iter()
            .map(|(class, price)| {
                encode_reserved_instances_offering_id(&offering_of_class(class, (*price, 0.0)))
            })
            .collect();
        let before = purchases(&engine);

        let body = quoted(&router, row.case, &[&held], &offering_ids).await;
        assert_eq!(
            member(&body, "isValidExchange").as_deref(),
            Some("false"),
            "case {case:?}: this exchange cannot be performed, got {body}",
            case = row.case
        );
        let reason = member(&body, "validationFailureReason").unwrap_or_default();
        assert!(
            reason.contains(row.expected),
            "case {case:?}: expected a reason naming {expected:?}, got {reason:?}",
            case = row.case,
            expected = row.expected
        );
        assert_eq!(
            purchases(&engine),
            before,
            "case {case:?}: a quote must leave every purchase as it was",
            case = row.case
        );
    }
}

/// One row of the refusal table below: what the request names, and the refusal it must draw.
struct Refusal {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The `reservedInstanceId` values the request carries.
    ids: Vec<String>,
    /// The `offeringId` of each target configuration the request carries.
    offering_ids: Vec<String>,
    /// The EC2 error code the refusal must carry.
    code: &'static str,
    /// The whole message the refusal must carry.
    message: String,
}

/// RULE 4: a value that names no purchase and one that names no offering are errors, each in
/// the refusal EC2 words for the value at fault.
///
/// The rows differ in which member names nothing and HOW: a `reservedInstanceId` outside the
/// Reserved Instances id form — spanning a value of no id form at all, an id of another EC2
/// resource, a UUID whose fields are the wrong length, one carrying a digit that is not
/// hexadecimal, and a value far longer than any id EC2 issues, which is echoed only as far as
/// the bound — one INSIDE that form the account holds nothing under, and an offering id this
/// service never published. The id form is one refusal and the lookup another, so the two
/// carry different codes.
#[tokio::test]
async fn a_value_that_names_nothing_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, (2, 500.0, 0.0));
    let published = offering_id((600.0, 0.0));
    let far_too_long = "r".repeat(4 * MAX_ECHOED_RESOURCE_ID_CHARS);

    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "a purchase id of no id form at all",
            ids: vec!["not-a-uuid".to_string()],
            offering_ids: vec![published.clone()],
            code: INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            message: "The reserved-instances ID 'not-a-uuid' is malformed".to_string(),
        },
        Refusal {
            case: "a purchase id of another EC2 resource form",
            ids: vec!["i-0123456789abcdef0".to_string()],
            offering_ids: vec![published.clone()],
            code: INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            message: "The reserved-instances ID 'i-0123456789abcdef0' is malformed".to_string(),
        },
        Refusal {
            case: "a UUID with a field of the wrong length",
            ids: vec!["b847fa93-e282-4f55-b59a-1342f5bd7c0".to_string()],
            offering_ids: vec![published.clone()],
            code: INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            message: "The reserved-instances ID 'b847fa93-e282-4f55-b59a-1342f5bd7c0' is \
                      malformed"
                .to_string(),
        },
        Refusal {
            case: "a UUID carrying a digit that is not hexadecimal",
            ids: vec!["b847fa93-e282-4f55-b59a-1342f5bd7c0z".to_string()],
            offering_ids: vec![published.clone()],
            code: INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            message: "The reserved-instances ID 'b847fa93-e282-4f55-b59a-1342f5bd7c0z' is \
                      malformed"
                .to_string(),
        },
        Refusal {
            case: "a purchase id far longer than any id EC2 issues",
            ids: vec![far_too_long],
            offering_ids: vec![published.clone()],
            code: INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            message: format!(
                "The reserved-instances ID '{}' is malformed",
                "r".repeat(MAX_ECHOED_RESOURCE_ID_CHARS)
            ),
        },
        Refusal {
            case: "a purchase id the account holds nothing under",
            ids: vec!["b847fa93-e282-4f55-b59a-1342f5bd7c02".to_string()],
            offering_ids: vec![published.clone()],
            code: INVALID_RESERVED_INSTANCE_ID,
            message: " [b847fa93-e282-4f55-b59a-1342f5bd7c02]".to_string(),
        },
        Refusal {
            case: "an offering id this service never published",
            ids: vec![held.clone()],
            offering_ids: vec!["rio-0123456789abcdef".to_string()],
            code: INVALID_RESERVED_INSTANCES_OFFERING_ID,
            message: "The Reserved Instances offering ID 'rio-0123456789abcdef' does not exist"
                .to_string(),
        },
        Refusal {
            case: "an offering id that is not an id at all",
            ids: vec![held.clone()],
            offering_ids: vec!["not-an-offering".to_string()],
            code: INVALID_RESERVED_INSTANCES_OFFERING_ID,
            message: "The Reserved Instances offering ID 'not-an-offering' does not exist"
                .to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let ids: Vec<&str> = row.ids.iter().map(String::as_str).collect();
        let (status, body) = quote(&router, &ids, &row.offering_ids, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {code}, got {body}",
            code = row.code
        );
        assert_eq!(
            error_message(&body),
            row.message,
            "case {case:?}: expected the message {message:?}, got {actual:?}",
            message = row.message,
            actual = error_message(&body)
        );
    }

    // A request that names no target at all has nothing to be quoted an exchange into.
    let (status, body) = quote(&router, &[held.as_str()], &[], &[]).await;
    assert_eq!(
        status, 400,
        "no target configuration: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MissingParameter",
        "no target configuration: expected MissingParameter, got {body}"
    );
}

/// RULE 5: a quote writes nothing, and `DryRun` answers the permission probe.
#[tokio::test]
async fn a_quote_writes_nothing_and_a_dry_run_answers_the_probe() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, (2, 500.0, 0.0));
    let offering_ids = vec![offering_id((600.0, 0.0))];
    let before = purchases(&engine);

    let first = quoted(&router, "the first quote", &[&held], &offering_ids).await;
    let again = quoted(&router, "the second quote", &[&held], &offering_ids).await;
    assert_eq!(
        member(&first, "paymentDue"),
        member(&again, "paymentDue"),
        "two quotes of one request must answer the same charge"
    );
    assert_eq!(
        purchases(&engine),
        before,
        "a quote must leave every purchase as it was"
    );

    // Rows differ in what the probe names: a purchase the account holds, one it does not, and
    // a value that is no Reserved Instances id at all. "Checks whether you have the required
    // permissions … without actually making the request", so all three answer the same thing
    // — the probe is answered ahead of every refusal the value would otherwise draw.
    for (case, id) in [
        ("a quote that would have been served", held.clone()),
        (
            "a quote naming a purchase the account does not hold",
            "b847fa93-e282-4f55-b59a-1342f5bd7c02".to_string(),
        ),
        (
            "a quote naming a value that is no Reserved Instances id",
            "ri-000000000000000000".to_string(),
        ),
    ] {
        let (status, body) = quote(
            &router,
            &[id.as_str()],
            &offering_ids,
            &[param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: expected EC2's dry-run sentence, got {:?}",
            error_message(&body)
        );
    }
    assert_eq!(
        purchases(&engine),
        before,
        "a probe must leave every purchase as it was"
    );
}
