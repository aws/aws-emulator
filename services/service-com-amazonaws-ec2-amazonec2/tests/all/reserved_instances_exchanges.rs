// SPDX-License-Identifier: CC0-1.0

//! What `AcceptReservedInstancesExchangeQuote` does to the Convertible Reserved Instances it
//! names, and the values it refuses — end to end over the wire the service actually speaks:
//! `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=AcceptReservedInstancesExchangeQuote`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * An exchange REPLACES: every Convertible Reserved Instance the request names is left
//!   `retired` and one new Convertible Reserved Instance per target configuration is in
//!   effect in its place, carrying that offering's instance type, Availability Zone (and so
//!   scope), product description, offering class and type, term and prices, and its share of
//!   the reservations the exchanged purchases held. The call answers one `exchangeId`.
//! * Both halves are visible to a LATER caller: exchanging the same Reserved Instance again
//!   is refused because it is retired, and `ModifyReservedInstances` resolves the Reserved
//!   Instances the exchange minted, which is what makes them purchases of this account
//!   rather than values this one call reported.
//! * An offering id is derived from the offering's own terms, so it round-trips through the
//!   wire and a value this service never minted names no offering at all.
//! * A `reservedInstanceId` that is not a Reserved Instances id, one the account holds
//!   nothing under, one whose purchase is not in effect or is not Convertible, a target
//!   configuration naming an offering this service does not publish, one that is not
//!   Convertible, one in a zone the Region does not have, one carrying the `InstanceCount`
//!   the model reserves, a request naming no target at all, a target worth less than what is
//!   being exchanged, and more target configurations than there are reservations, are each
//!   refused with the code EC2 answers for the member at fault — and nothing is exchanged.
//! * A dry run reports the permission answer and exchanges nothing.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_PARAMETER_VALUE,
    INVALID_RESERVED_INSTANCE_ID, INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
    INVALID_RESERVED_INSTANCES_OFFERING_ID, MAX_ECHOED_RESOURCE_ID_CHARS, MISSING_PARAMETER,
    RESERVED_INSTANCE_SCOPE_REGION, RESERVED_INSTANCE_SCOPE_ZONE, RESERVED_INSTANCE_STATE_ACTIVE,
    RESERVED_INSTANCE_STATE_QUEUED, RESERVED_INSTANCE_STATE_RETIRED, ReservedInstancesOfferingTerms,
    ReservedInstancesSpec, availability_zones_in_region, decode_reserved_instances_offering_id,
    encode_reserved_instances_offering_id, reserved_instances_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The form-field prefix ec2Query flattens `TargetConfigurations` into: the member's
/// `@xmlName`
/// (`smithy_query(query="operation",operation="AcceptReservedInstancesExchangeQuote")` gives
/// `TargetConfiguration`).
const TARGETS: &str = "TargetConfiguration";

/// The form-field prefix ec2Query flattens `ReservedInstanceIds` into, from the same place:
/// `ReservedInstanceId`.
const IDS: &str = "ReservedInstanceId";

/// A one-year Reserved Instance term, in seconds — the units `ReservedInstances$Duration` is
/// reported in ("the duration of the Reserved Instance, in seconds").
const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// The offering class of a purchase that can be exchanged, and of an offering it can be
/// exchanged into: the `OfferingClassType` enum's `convertible` member.
const CONVERTIBLE: &str = "convertible";

/// The other member of that enum, which an exchange refuses on either side.
const STANDARD: &str = "standard";

/// The offering type every purchase and offering here carries: a member of
/// `OFFERING_TYPE_VALUES_VALUES`.
const ALL_UPFRONT: &str = "All Upfront";

/// The product description every purchase and offering here carries: a member of
/// `R_I_PRODUCT_DESCRIPTION_VALUES`.
const LINUX_UNIX: &str = "Linux/UNIX";

/// One Reserved Instance to seed: its instance type, its Availability Zone (`None` for a
/// Regional-scope purchase), how many reservations it holds, and the purchase price of each.
type PurchaseRow = (&'static str, Option<&'static str>, i32, f32);

/// One Convertible offering a target configuration names: its instance type, its
/// Availability Zone (`None` for an offering of Regional scope) and the purchase price of one
/// of its reservations.
type OfferingRow = (&'static str, Option<&'static str>, f32);

/// One row of the exchange table: the case, the purchases to seed, how many times each is
/// named in the one request, the target offerings to exchange them into, and the reservations
/// each of those offerings must end up holding.
type ExchangeCase = (
    &'static str,
    &'static [PurchaseRow],
    usize,
    &'static [OfferingRow],
    &'static [i32],
);

/// One row of the refusal table: the case, how to seed the account's purchase, which
/// `reservedInstanceId` the request names, the target offerings, any extra form fields, and
/// the error code and message fragment the refusal must carry.
type RefusalCase = (
    &'static str,
    Seeding,
    Named,
    &'static [OfferingRow],
    &'static [(&'static str, &'static str)],
    &'static str,
    String,
);

/// One Reserved Instance purchase to seed, in `state` and of `offering_class`.
fn purchase(state: &str, offering_class: &str, row: PurchaseRow) -> ReservedInstancesSpec {
    let (instance_type, availability_zone, instance_count, fixed_price) = row;
    ReservedInstancesSpec {
        instance_type: instance_type.to_string(),
        availability_zone: availability_zone.map(str::to_string),
        region: REGION.to_string(),
        instance_count,
        duration_seconds: ONE_YEAR_SECONDS,
        offering_class: offering_class.to_string(),
        offering_type: ALL_UPFRONT.to_string(),
        product_description: LINUX_UNIX.to_string(),
        state: state.to_string(),
        start: SystemTime::now(),
        fixed_price,
        usage_price: 0.0,
    }
}

/// Seed one Convertible Reserved Instance that is in effect, and answer the id the store
/// minted for it.
fn seed(engine: &engine::InMemoryEngine, row: PurchaseRow) -> String {
    seed_as(engine, RESERVED_INSTANCE_STATE_ACTIVE, CONVERTIBLE, row)
}

/// Seed one Reserved Instance in `state` and of `offering_class`, and answer its id.
fn seed_as(
    engine: &engine::InMemoryEngine,
    state: &str,
    offering_class: &str,
    row: PurchaseRow,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(purchase(state, offering_class, row))
        .expect("the store has room for the few purchases this suite seeds")
}

/// One Convertible offering, as the terms its id carries.
fn offering(row: OfferingRow) -> ReservedInstancesOfferingTerms {
    offering_of_class(CONVERTIBLE, row)
}

/// One offering of `offering_class`, as the terms its id carries.
fn offering_of_class(offering_class: &str, row: OfferingRow) -> ReservedInstancesOfferingTerms {
    let (instance_type, availability_zone, fixed_price) = row;
    ReservedInstancesOfferingTerms {
        instance_type: instance_type.to_string(),
        availability_zone: availability_zone.map(str::to_string),
        product_description: LINUX_UNIX.to_string(),
        offering_class: offering_class.to_string(),
        offering_type: ALL_UPFRONT.to_string(),
        duration_seconds: ONE_YEAR_SECONDS,
        fixed_price,
        usage_price: 0.0,
    }
}

/// The `ReservedInstancesOfferingId` one Convertible offering is published under.
fn offering_id(row: OfferingRow) -> String {
    encode_reserved_instances_offering_id(&offering(row))
}

/// The form fields for a whole `TargetConfigurations` list: `<prefix>.<N>.OfferingId`, each
/// offering named by the id it is published under.
fn target_params(offering_ids: &[String]) -> Vec<Param> {
    offering_ids
        .iter()
        .enumerate()
        .map(|(index, id)| param(&format!("{TARGETS}.{}.OfferingId", index + 1), id))
        .collect()
}

/// One `AcceptReservedInstancesExchangeQuote` call naming `ids` and asking for the offerings
/// `offering_ids` names, as its HTTP status and XML body.
async fn accept(
    router: &MultiServiceRouter,
    ids: &[&str],
    offering_ids: &[String],
    extra: &[Param],
) -> (u16, String) {
    let mut params = indexed(IDS, ids);
    params.extend(target_params(offering_ids));
    params.extend_from_slice(extra);
    super::ec2_query::call(router, REGION, "AcceptReservedInstancesExchangeQuote", &params).await
}

/// The `exchangeId` a successful call reported, having first pinned the status.
fn exchange_id(case: &str, status: u16, body: &str) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    super::ec2_query::member(body, "exchangeId")
        .unwrap_or_else(|| panic!("case {case:?}: no <exchangeId> in {body}"))
}

/// What the store holds for one Reserved Instance: everything an exchange decides about the
/// purchase it mints, and the state it leaves the one it replaces in.
#[derive(Debug, Clone, PartialEq)]
struct StoredPurchase {
    state: Option<String>,
    instance_type: Option<String>,
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    scope: Option<String>,
    instance_count: Option<i32>,
    duration: Option<i64>,
    offering_class: Option<String>,
    offering_type: Option<String>,
    product_description: Option<String>,
    fixed_price: Option<f32>,
    usage_price: Option<f32>,
}

/// Every Reserved Instance the store holds, by id.
fn purchases(engine: &engine::InMemoryEngine) -> BTreeMap<String, StoredPurchase> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .reserved_instances
        .iter()
        .map(|(reserved_instances_id, held)| {
            (
                reserved_instances_id.clone(),
                StoredPurchase {
                    state: held.state.clone(),
                    instance_type: held.instance_type.clone(),
                    availability_zone: held.availability_zone.clone(),
                    availability_zone_id: held.availability_zone_id.clone(),
                    scope: held.scope.clone(),
                    instance_count: held.instance_count,
                    duration: held.duration,
                    offering_class: held.offering_class.clone(),
                    offering_type: held.offering_type.clone(),
                    product_description: held.product_description.clone(),
                    fixed_price: held.fixed_price,
                    usage_price: held.usage_price,
                },
            )
        })
        .collect()
}

/// The zone id of `zone_name` in the Region these calls are signed for: a fact of the Region
/// ([`availability_zones_in_region`]), read the way the service reads it rather than written
/// down twice.
fn zone_id(zone_name: &str) -> String {
    availability_zones_in_region(REGION)
        .into_iter()
        .find(|known| known.zone_name == zone_name)
        .map(|known| known.zone_id)
        .unwrap_or_else(|| panic!("{REGION} must have the zone {zone_name}"))
}

/// The purchase one target offering must have been minted as, holding `instance_count`
/// reservations.
fn expected_purchase(row: OfferingRow, instance_count: i32) -> StoredPurchase {
    let (instance_type, availability_zone, fixed_price) = row;
    StoredPurchase {
        state: Some(RESERVED_INSTANCE_STATE_ACTIVE.to_string()),
        instance_type: Some(instance_type.to_string()),
        availability_zone: availability_zone.map(str::to_string),
        availability_zone_id: availability_zone.map(zone_id),
        scope: Some(
            if availability_zone.is_some() {
                RESERVED_INSTANCE_SCOPE_ZONE
            } else {
                RESERVED_INSTANCE_SCOPE_REGION
            }
            .to_string(),
        ),
        instance_count: Some(instance_count),
        duration: Some(ONE_YEAR_SECONDS),
        offering_class: Some(CONVERTIBLE.to_string()),
        offering_type: Some(ALL_UPFRONT.to_string()),
        product_description: Some(LINUX_UNIX.to_string()),
        fixed_price: Some(fixed_price),
        usage_price: Some(0.0),
    }
}

// ---------------------------------------------------------------------------
// What an exchange does
// ---------------------------------------------------------------------------

/// The rule: an exchange retires every Convertible Reserved Instance it names and mints one
/// in effect per target configuration, carrying that offering's terms and its share of the
/// reservations the exchanged purchases held.
///
/// The rows differ along what the rule generalises over: how many purchases one call
/// exchanges and how many reservations each holds, how many offerings they are exchanged into
/// (one, split evenly, split with a remainder), what the offering changes (instance type,
/// Availability Zone, zonal to Regional scope and back), and whether the request names a
/// purchase more than once.
#[tokio::test]
async fn an_exchange_retires_what_it_names_and_mints_the_target_offerings() {
    let cases: &[ExchangeCase] = &[
        (
            "one zonal purchase exchanged into another zone",
            &[("t3.micro", Some("us-east-1a"), 2, 500.0)],
            1,
            &[("t3.micro", Some("us-east-1b"), 600.0)],
            &[2],
        ),
        (
            "a zonal purchase exchanged into a Regional offering",
            &[("m5.large", Some("us-east-1a"), 3, 500.0)],
            1,
            &[("m5.xlarge", None, 900.0)],
            &[3],
        ),
        (
            "a Regional purchase exchanged into a zonal offering of exactly equal value",
            &[("c5.large", None, 1, 500.0)],
            1,
            &[("c5.large", Some("us-east-1c"), 500.0)],
            &[1],
        ),
        (
            "two purchases exchanged into one offering",
            &[
                ("t3.micro", Some("us-east-1a"), 1, 500.0),
                ("t3.micro", Some("us-east-1b"), 2, 500.0),
            ],
            1,
            &[("t3.large", None, 700.0)],
            &[3],
        ),
        (
            "one purchase split evenly over two offerings",
            &[("r5.2xlarge", None, 4, 500.0)],
            1,
            &[
                ("r5.xlarge", Some("us-east-1a"), 600.0),
                ("r5.xlarge", Some("us-east-1c"), 600.0),
            ],
            &[2, 2],
        ),
        (
            "one purchase split over three offerings with a remainder",
            &[("m5.large", None, 5, 500.0)],
            1,
            &[
                ("m5.large", Some("us-east-1a"), 600.0),
                ("m5.large", Some("us-east-1b"), 600.0),
                ("m5.large", None, 600.0),
            ],
            &[2, 2, 1],
        ),
        (
            "a request that names the same purchase three times",
            &[("c5.xlarge", Some("us-east-1c"), 3, 500.0)],
            3,
            &[("c5.2xlarge", Some("us-east-1a"), 800.0)],
            &[3],
        ),
    ];

    for (case, rows, repeats, targets, shares) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded: Vec<String> = rows.iter().map(|row| seed(&engine, *row)).collect();
        let named: Vec<&str> = seeded
            .iter()
            .flat_map(|id| std::iter::repeat_n(id.as_str(), *repeats))
            .collect();
        let offering_ids: Vec<String> = targets.iter().copied().map(offering_id).collect();

        let (status, body) = accept(&router, &named, &offering_ids, &[]).await;
        let reported = exchange_id(case, status, &body);
        assert!(
            reserved_instances_id_is_well_formed(&reported),
            "case {case:?}: the exchange id must read as a UUID, got {reported:?}"
        );

        let held = purchases(&engine);
        assert_eq!(
            held.len(),
            seeded.len() + targets.len(),
            "case {case:?}: the account must hold the {} exchanged purchases beside the {} \
             minted ones, got {held:?}",
            seeded.len(),
            targets.len()
        );
        for exchanged in &seeded {
            assert_eq!(
                held.get(exchanged).and_then(|purchase| purchase.state.clone()),
                Some(RESERVED_INSTANCE_STATE_RETIRED.to_string()),
                "case {case:?}: the exchanged purchase {exchanged} must be left \
                 '{RESERVED_INSTANCE_STATE_RETIRED}', got {:?}",
                held.get(exchanged)
            );
        }
        // The purchases the exchange minted, in the order the store holds them: the ids come
        // from one monotonic mint, so that order is the order the target configurations were
        // given in.
        let minted: Vec<&StoredPurchase> = held
            .iter()
            .filter(|(id, _)| !seeded.contains(id))
            .map(|(_, purchase)| purchase)
            .collect();
        let expected: Vec<StoredPurchase> = targets
            .iter()
            .zip(*shares)
            .map(|(row, count)| expected_purchase(*row, *count))
            .collect();
        let actual: Vec<StoredPurchase> = minted.into_iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the purchases {expected:?} of the target offerings, got \
             {actual:?}"
        );
    }
}

/// The rule's second half: the two sides of an exchange are visible to a LATER caller — the
/// retired purchase can no longer be exchanged, and the purchases the exchange minted are
/// resolved by another operation that reads this account's Reserved Instances.
///
/// `ModifyReservedInstances` is that reader: it refuses an id the account holds nothing
/// under, so its taking one of the minted ids and recording a modification of it is proof the
/// exchange left a real, in-effect purchase behind rather than a value it reported once.
#[tokio::test]
async fn the_exchange_is_visible_to_a_later_caller() {
    let (router, engine) = super::ec2_query::emulator();
    let exchanged = seed(&engine, ("m5.large", Some("us-east-1a"), 2, 500.0));
    let target = ("m5.xlarge", Some("us-east-1b"), 700.0);
    let before: Vec<String> = purchases(&engine).into_keys().collect();

    let (status, body) = accept(&router, &[&exchanged], &[offering_id(target)], &[]).await;
    exchange_id("an exchange a later caller reads back", status, &body);

    let after = purchases(&engine);
    let minted: Vec<String> = after
        .keys()
        .filter(|id| !before.contains(id))
        .cloned()
        .collect();
    assert_eq!(
        minted.len(),
        1,
        "the exchange must have minted exactly one purchase, got {minted:?}"
    );

    // Exchanging the retired purchase again is refused, because it is no longer in effect.
    let (status, body) = accept(&router, &[&exchanged], &[offering_id(target)], &[]).await;
    assert_eq!(
        status, 400,
        "exchanging a retired purchase must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "exchanging a retired purchase must be refused \
         '{INVALID_PARAMETER_VALUE}', got {body}"
    );
    assert!(
        error_message(&body).contains(RESERVED_INSTANCE_STATE_RETIRED),
        "the refusal must name the state the purchase is in, got {:?}",
        error_message(&body)
    );

    // The purchase the exchange minted is one `ModifyReservedInstances` resolves: it records
    // a modification of it, which it refuses for an id this account holds nothing under.
    let minted_id = &minted[0];
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "ModifyReservedInstances",
        &[
            param("ReservedInstancesId.1", minted_id),
            param(
                "ReservedInstancesConfigurationSetItemType.1.InstanceCount",
                "2",
            ),
            param(
                "ReservedInstancesConfigurationSetItemType.1.AvailabilityZone",
                "us-east-1c",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "ModifyReservedInstances must resolve the purchase {minted_id} the exchange minted, \
         got {status} with body {body}"
    );
}

/// The rule: an offering id is derived from the offering's own terms, so the terms read back
/// out of it unchanged, and a value this service never minted names no offering.
///
/// The rows differ along every member of the terms: the scope (a zone, or none), the instance
/// type, the product description, the offering class and type, the term and the prices.
#[test]
fn an_offering_id_carries_the_terms_it_was_minted_from() {
    let cases: &[(&str, ReservedInstancesOfferingTerms)] = &[
        (
            "a zonal Convertible offering",
            offering(("t3.micro", Some("us-east-1a"), 500.0)),
        ),
        (
            "a Regional Convertible offering",
            offering(("m5.24xlarge", None, 12_345.67)),
        ),
        (
            "a Standard offering",
            offering_of_class(STANDARD, ("c5.large", Some("us-east-1b"), 0.0)),
        ),
        (
            "a three-year No Upfront Windows offering",
            ReservedInstancesOfferingTerms {
                instance_type: "r5.large".to_string(),
                availability_zone: None,
                // A member of `R_I_PRODUCT_DESCRIPTION_VALUES`, and of
                // `OFFERING_TYPE_VALUES_VALUES`.
                product_description: "Windows".to_string(),
                offering_class: CONVERTIBLE.to_string(),
                offering_type: "No Upfront".to_string(),
                duration_seconds: 3 * ONE_YEAR_SECONDS,
                fixed_price: 0.0,
                usage_price: 0.125,
            },
        ),
    ];

    for (case, terms) in cases {
        let id = encode_reserved_instances_offering_id(terms);
        let read = decode_reserved_instances_offering_id(&id);
        assert_eq!(
            read.as_ref(),
            Some(terms),
            "case {case:?}: the id {id:?} must carry the terms it was minted from, got {read:?}"
        );
    }

    // A value this service never minted carries no offering: it is not read as one whatever
    // it looks like.
    let minted = offering_id(("t3.micro", Some("us-east-1a"), 500.0));
    let tampered = format!("{minted}00");
    let not_offerings: &[(&str, &str)] = &[
        ("an empty value", ""),
        ("a UUID, the form an AWS offering id reads in", "4b2293b4-5813-4cc8-9ce3-1957fc1dcfc8"),
        ("the prefix alone", "rio-"),
        ("the prefix and text that is not hex", "rio-not-hex"),
        ("an id with bytes appended", tampered.as_str()),
    ];
    for (case, value) in not_offerings {
        let read = decode_reserved_instances_offering_id(value);
        assert!(
            read.is_none(),
            "case {case:?}: {value:?} must name no offering, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What an exchange refuses
// ---------------------------------------------------------------------------

/// The rule: a request this service cannot act on is refused with the code EC2 answers for
/// the member at fault, and nothing is exchanged.
///
/// The rows span the classes of value an exchange refuses: an unreadable
/// `reservedInstanceId` (the recorded case, a value of another id form, and one far longer
/// than any id EC2 issues, which is quoted only as far as the echo is bounded), a well-formed
/// id the account holds nothing under, a purchase that is not in effect or not Convertible, a
/// target offering this service does not publish or that is not Convertible or lies outside
/// the Region, the `InstanceCount` the model reserves, no target at all, a target worth less
/// than what is exchanged, and more target configurations than reservations.
#[tokio::test]
async fn an_exchange_refuses_what_it_cannot_act_on_and_exchanges_nothing() {
    let long_id = "r".repeat(4 * MAX_ECHOED_RESOURCE_ID_CHARS);
    // The purchase every row seeds: Convertible, in effect, two reservations at 500 each.
    let seeded_row: PurchaseRow = ("m5.large", Some("us-east-1a"), 2, 500.0);
    let cases: &[RefusalCase] = &[
        (
            "a value that is not a Reserved Instances id",
            Seeding::Exchangeable,
            Named::Literal("not-a-valid-reserved-instance-id"),
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            "The reserved-instances ID 'not-a-valid-reserved-instance-id' is malformed"
                .to_string(),
        ),
        (
            "an id of another EC2 resource form",
            Seeding::Exchangeable,
            Named::Literal("i-0123456789abcdef0"),
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            "The reserved-instances ID 'i-0123456789abcdef0' is malformed".to_string(),
        ),
        (
            "a UUID with a field of the wrong length",
            Seeding::Exchangeable,
            Named::Literal("b847fa93-e282-4f55-b59a-1342f5bd7c0"),
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            "The reserved-instances ID 'b847fa93-e282-4f55-b59a-1342f5bd7c0' is malformed"
                .to_string(),
        ),
        (
            "a value far longer than any id EC2 issues",
            Seeding::Exchangeable,
            Named::Owned(long_id.clone()),
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            format!(
                "The reserved-instances ID '{}' is malformed",
                "r".repeat(MAX_ECHOED_RESOURCE_ID_CHARS)
            ),
        ),
        (
            "a well-formed id the account holds nothing under",
            Seeding::Exchangeable,
            Named::Literal("B847FA93-E282-4F55-B59A-1342F5BD7C02"),
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_RESERVED_INSTANCE_ID,
            " [B847FA93-E282-4F55-B59A-1342F5BD7C02]".to_string(),
        ),
        (
            "a purchase that is only queued",
            Seeding::Queued,
            Named::Seeded,
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_PARAMETER_VALUE,
            format!("The Reserved Instance is in the '{RESERVED_INSTANCE_STATE_QUEUED}' state"),
        ),
        (
            "a purchase whose term is over",
            Seeding::Retired,
            Named::Seeded,
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_PARAMETER_VALUE,
            format!("The Reserved Instance is in the '{RESERVED_INSTANCE_STATE_RETIRED}' state"),
        ),
        (
            "a Standard purchase",
            Seeding::Standard,
            Named::Seeded,
            &[("m5.large", None, 600.0)],
            &[],
            INVALID_PARAMETER_VALUE,
            "Only Convertible Reserved Instances can be exchanged".to_string(),
        ),
        (
            "no target configuration at all",
            Seeding::Exchangeable,
            Named::Seeded,
            &[],
            &[],
            MISSING_PARAMETER,
            "The request must contain the parameter targetConfiguration".to_string(),
        ),
        (
            "a target configuration carrying the reserved instanceCount",
            Seeding::Exchangeable,
            Named::Seeded,
            &[("m5.large", None, 600.0)],
            &[("TargetConfiguration.1.InstanceCount", "2")],
            INVALID_PARAMETER_VALUE,
            "This parameter is reserved and cannot be specified in a request".to_string(),
        ),
        (
            "a target offering worth less than what is exchanged",
            Seeding::Exchangeable,
            Named::Seeded,
            &[("m5.large", None, 100.0)],
            &[],
            INVALID_PARAMETER_VALUE,
            "are worth 200.00 USD, less than the 1000.00 USD".to_string(),
        ),
        (
            "more target configurations than there are reservations",
            Seeding::Exchangeable,
            Named::Seeded,
            &[
                ("m5.large", None, 600.0),
                ("m5.large", None, 600.0),
                ("m5.large", None, 600.0),
            ],
            &[],
            INVALID_PARAMETER_VALUE,
            "fewer than the 3 target configurations".to_string(),
        ),
        (
            "a target offering in a zone the Region does not have",
            Seeding::Exchangeable,
            Named::Seeded,
            &[("m5.large", Some("us-west-2a"), 600.0)],
            &[],
            INVALID_PARAMETER_VALUE,
            "Invalid availability zone: [us-west-2a]".to_string(),
        ),
    ];

    for (case, seeding, named, targets, extra, code, message) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seeding.seed(&engine, seeded_row);
        let before = purchases(&engine);
        let ids: Vec<&str> = vec![named.as_str(&seeded)];
        let offering_ids: Vec<String> = targets.iter().copied().map(offering_id).collect();
        let extra: Vec<Param> = extra.iter().map(|(name, value)| param(name, value)).collect();

        let (status, body) = accept(&router, &ids, &offering_ids, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code:?}, got {body}"
        );
        assert!(
            error_message(&body).contains(message.as_str()),
            "case {case:?}: expected a message carrying {message:?}, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            purchases(&engine),
            before,
            "case {case:?}: a refused exchange must leave every purchase as it was"
        );
    }
}

/// The two target offerings an exchange refuses for what the OFFERING is, rather than for the
/// id naming no offering: the ones this service never published, and a Standard one.
///
/// Separate from the table above because these rows need an offering id built by hand rather
/// than from an [`OfferingRow`]: a value the mint never produced, and one minted for an
/// offering of the other class.
#[tokio::test]
async fn an_exchange_refuses_a_target_that_is_not_a_published_convertible_offering() {
    let standard = encode_reserved_instances_offering_id(&offering_of_class(
        STANDARD,
        ("m5.large", None, 900.0),
    ));
    let cases: &[(&str, String, &str, String)] = &[
        (
            "a value that is not an offering id",
            "not-an-offering-id".to_string(),
            INVALID_RESERVED_INSTANCES_OFFERING_ID,
            "The Reserved Instances offering ID 'not-an-offering-id' does not exist".to_string(),
        ),
        (
            "a UUID, the form an AWS offering id reads in",
            "4b2293b4-5813-4cc8-9ce3-1957fc1dcfc8".to_string(),
            INVALID_RESERVED_INSTANCES_OFFERING_ID,
            "The Reserved Instances offering ID '4b2293b4-5813-4cc8-9ce3-1957fc1dcfc8' does not \
             exist"
                .to_string(),
        ),
        (
            "an offering id with bytes appended",
            format!("{}00", offering_id(("m5.large", None, 900.0))),
            INVALID_RESERVED_INSTANCES_OFFERING_ID,
            "does not exist".to_string(),
        ),
        (
            "a Standard offering",
            standard,
            INVALID_PARAMETER_VALUE,
            "Only a Convertible Reserved Instances offering can be exchanged into".to_string(),
        ),
    ];

    for (case, target, code, message) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, ("m5.large", Some("us-east-1a"), 2, 500.0));
        let before = purchases(&engine);

        let (status, body) =
            accept(&router, &[&seeded], std::slice::from_ref(target), &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code:?}, got {body}"
        );
        assert!(
            error_message(&body).contains(message.as_str()),
            "case {case:?}: expected a message carrying {message:?}, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            purchases(&engine),
            before,
            "case {case:?}: a refused exchange must leave every purchase as it was"
        );
    }
}

/// The rule: "checks whether you have the required permissions for the action, without
/// actually making the request … If you have the required permissions, the error response is
/// DryRunOperation" ($DryRun) — so a dry run reports HTTP 412 `DryRunOperation` and exchanges
/// nothing, for a request that would have gone through, for one naming a purchase this
/// account does not hold and for one naming no Reserved Instances id at all: the probe is
/// about the CALLER's permission, not about the value the request carries, so it is answered
/// ahead of every refusal a `reservedInstanceId` would otherwise draw.
#[tokio::test]
async fn a_dry_run_reports_the_permission_answer_and_exchanges_nothing() {
    // Rows differ in what the probe's `reservedInstanceId` names: the seeded purchase, a
    // well-formed id of no purchase, and a value outside the Reserved Instances id form.
    let cases: &[(&str, Named)] = &[
        ("a dry run of an exchange that would go through", Named::Seeded),
        (
            "a dry run naming a purchase this account does not hold",
            Named::Literal("B847FA93-E282-4F55-B59A-1342F5BD7C02"),
        ),
        (
            "a dry run naming a value that is no Reserved Instances id",
            Named::Literal("ri-000000000000000000"),
        ),
    ];

    for (case, named) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, ("m5.large", Some("us-east-1a"), 2, 500.0));
        let before = purchases(&engine);

        let (status, body) = accept(
            &router,
            &[named.as_str(&seeded)],
            &[offering_id(("m5.large", None, 600.0))],
            &[param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: a dry run must report HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: a dry run must report 'DryRunOperation', got {body}"
        );
        assert_eq!(
            purchases(&engine),
            before,
            "case {case:?}: a dry run must leave every purchase as it was"
        );
    }
}

/// How one refusal row seeds the account's Reserved Instance: the state and offering class it
/// is held with, which is what decides whether the exchange can act on it.
#[derive(Debug, Clone, Copy)]
enum Seeding {
    /// A Convertible purchase in effect — the one an exchange can act on.
    Exchangeable,
    /// A Convertible purchase scheduled for a future date and not paid for yet.
    Queued,
    /// A Convertible purchase whose term this account no longer holds.
    Retired,
    /// A Standard purchase in effect, which cannot be exchanged at all.
    Standard,
}

impl Seeding {
    /// Seed the purchase and answer its id.
    fn seed(self, engine: &engine::InMemoryEngine, row: PurchaseRow) -> String {
        let (state, offering_class) = match self {
            Self::Exchangeable => (RESERVED_INSTANCE_STATE_ACTIVE, CONVERTIBLE),
            Self::Queued => (RESERVED_INSTANCE_STATE_QUEUED, CONVERTIBLE),
            Self::Retired => (RESERVED_INSTANCE_STATE_RETIRED, CONVERTIBLE),
            Self::Standard => (RESERVED_INSTANCE_STATE_ACTIVE, STANDARD),
        };
        seed_as(engine, state, offering_class, row)
    }
}

/// Which `reservedInstanceId` one refusal row names: the purchase it seeded, or a value of
/// its own.
#[derive(Debug, Clone)]
enum Named {
    /// The id the store minted for the seeded purchase.
    Seeded,
    /// A literal value the row names instead.
    Literal(&'static str),
    /// A value the row had to build.
    Owned(String),
}

impl Named {
    /// The id to send, given the one the seeding minted.
    fn as_str<'a>(&'a self, seeded: &'a str) -> &'a str {
        match self {
            Self::Seeded => seeded,
            Self::Literal(value) => value,
            Self::Owned(value) => value.as_str(),
        }
    }
}
