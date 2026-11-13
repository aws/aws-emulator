// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Capacity Reservation cancellation quote slice:
//! `CreateCapacityReservation` makes the future-dated reservation,
//! `CreateCapacityReservationCancellationQuote` generates the quote for it, and
//! `DescribeCapacityReservationCancellationQuotes` reads the quotes back. Driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A quote exists only for a Capacity Reservation whose COMMITMENT is still in force: a
//!    reservation that committed to nothing, and one whose commitment the clock has passed,
//!    are both refused `InvalidParameterValue` with the one fixed sentence AWS words that
//!    precondition as, a value of the Capacity Reservation id SHAPE the account holds no
//!    reservation under with `InvalidCapacityReservationId.NotFound`, and a value outside
//!    that shape with `InvalidCapacityReservationId.Malformed`.
//! 2. A generated quote is COMPUTED from that reservation's stored commitment and capacity:
//!    the capacity it reserves and the state it is in as its current configuration, and, as
//!    its terms, the one cancellation charge the model carries, the state the reservation
//!    moves to while those charges are applied, the capacity that stays under commitment,
//!    the whole hours left of the commitment and the instant it ends.
//! 3. A quote is "valid for 24 hours"
//!    (`CreateCapacityReservationCancellationQuote`): its `ExpirationTime` is that long after
//!    its `CreateTime`, and its `QuoteState` follows the clock from there — generated
//!    `pending`, `active` once the generation has fallen due, and `expired` from the instant
//!    it stops being valid. A read reports that without writing it.
//! 4. The describe reports what the generates stored, narrowed by
//!    `CapacityReservationCancellationQuoteIds`, by filter, or by neither, in
//!    `CapacityReservationCancellationQuoteId` order and paged with the token this service
//!    mints.
//! 5. `ClientToken` makes the generate idempotent: a replay reports the same quote and stores
//!    nothing more, while the same token asking for a DIFFERENT quote is told so.
//! 6. `DryRun` reports the outcome and writes nothing.
//!
//! Every table row is a distinct input along what those rules generalise over — how much
//! capacity, which instance family, how far ahead the reservation starts, how long it
//! committed for (including a commitment that is not a whole number of hours), which tags,
//! which id form — so no single recorded request is the specification.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::CapacityReservationConfiguration;
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    APPLY_CANCELLATION_CHARGES_COMMITMENT_WIND_DOWN,
    CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX,
    CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE,
    CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_EXPIRED,
    CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING,
    CAPACITY_RESERVATION_CANCELLATION_QUOTE_VALIDITY, CAPACITY_RESERVATION_STATE_ACTIVE,
    CAPACITY_RESERVATION_STATE_CANCELLING, CAPACITY_RESERVATION_STATE_SCHEDULED,
    CapacityReservationCancellationQuoteSpec, CapacityReservationCommitment,
    CapacityReservationSpec, TRANSITION_DELAY, iso8601_utc,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, error_message, filters};
use super::wire::{child, child_text, document, is_long_id, set_items};

/// The Region every call here is signed for, and the zone in it the reserved capacity sits
/// in.
const REGION: &str = "us-east-1";
const AVAILABILITY_ZONE: &str = "us-east-1a";

/// The platform every reservation here reserves capacity for: a member of
/// `CapacityReservationInstancePlatform`
/// (`smithy_query(query="shape",shape="CapacityReservationInstancePlatform")`).
const INSTANCE_PLATFORM: &str = "Linux/UNIX";

/// The match criteria a future-dated reservation must carry: "If you are requesting a
/// future-dated Capacity Reservation, you must specify `targeted`"
/// (`CreateCapacityReservationRequest$InstanceMatchCriteria`).
const INSTANCE_MATCH_CRITERIA_TARGETED: &str = "targeted";

/// "The only supported value is `incremental`"
/// (`CreateCapacityReservationRequest$DeliveryPreference`).
const DELIVERY_PREFERENCE: &str = "incremental";

/// A WELL-FORMED Capacity Reservation id the account holds no reservation for, so the service
/// looks it up rather than refusing it as malformed before the lookup.
const UNKNOWN_CAPACITY_RESERVATION_ID: &str = "cr-0ffffffffffffffff";

/// A WELL-FORMED cancellation quote id the account holds no quote for, in the form this
/// service issues ([`CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX`]).
const UNKNOWN_QUOTE_ID: &str = "crcq-0ffffffffffffffff";

/// The code AWS answers a cancellation-quote request for a Capacity Reservation with no
/// commitment terms in force with: the parameter-value class, because the value of
/// `CapacityReservationId` names a reservation there is no quote to generate for.
const NO_COMMITMENT_TERMS_CODE: &str = "InvalidParameterValue";

/// The whole sentence that refusal carries. FIXED: it interpolates nothing — not the id the
/// request named and not the state the reservation is in — so every row of the table below
/// is answered the same text.
const NO_COMMITMENT_TERMS_MESSAGE: &str =
    "The Capacity Reservation contains no commitment terms. Quote is not needed for \
     cancellation.";

/// Long enough past a clock-owed move's due instant that it has fallen due, with the margin
/// every other suite of this crate gives one.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `CreateCapacityReservationCancellationQuote` call carrying `params`.
async fn generate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "CreateCapacityReservationCancellationQuote",
        params,
    )
    .await
}

/// One `DescribeCapacityReservationCancellationQuotes` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DescribeCapacityReservationCancellationQuotes",
        params,
    )
    .await
}

/// The form fields of one generate for `capacity_reservation_id`.
fn quote_params(capacity_reservation_id: &str) -> Vec<(String, String)> {
    vec![(
        "CapacityReservationId".to_string(),
        capacity_reservation_id.to_string(),
    )]
}

/// What one future-dated Capacity Reservation is created with. Each row of the table differs
/// only along what the rules generalise over: the capacity reserved, the instance family it
/// is reserved in, how far ahead it starts and how long it committed for.
struct ReservationCase {
    /// What the case is called in an assertion.
    name: &'static str,
    /// A member of the modelled `InstanceType` catalog, in one of the instance families a
    /// future-dated reservation may reserve ("the C, M, R, I, T, and G instance families
    /// only", `CreateCapacityReservationRequest$InstanceType`).
    instance_type: &'static str,
    /// "The number of instances for which to reserve capacity" (`InstanceCount`).
    instance_count: i32,
    /// How far ahead the reservation starts: "between 5 and 120 days in advance"
    /// (`StartDate`).
    lead_days: u64,
    /// "Specify a commitment duration, in seconds" (`CommitmentDuration`).
    commitment_seconds: u64,
    /// "The tags to apply to the cancellation quote"
    /// (`CreateCapacityReservationCancellationQuoteRequest$TagSpecifications`).
    quote_tags: &'static [(&'static str, &'static str)],
}

/// The rows every rule is asserted over.
fn reservation_cases() -> Vec<ReservationCase> {
    vec![
        ReservationCase {
            name: "one instance, five days out, a one-day commitment",
            instance_type: "c5.xlarge",
            instance_count: 1,
            lead_days: 5,
            commitment_seconds: 24 * 60 * 60,
            quote_tags: &[],
        },
        ReservationCase {
            name: "sixty-four instances, thirty days out, a week's commitment",
            instance_type: "m5.large",
            instance_count: 64,
            lead_days: 30,
            commitment_seconds: 7 * 24 * 60 * 60,
            quote_tags: &[("Team", "research")],
        },
        ReservationCase {
            name: "the furthest-ahead start and a year's commitment",
            instance_type: "r5.large",
            instance_count: 7,
            lead_days: 119,
            commitment_seconds: 365 * 24 * 60 * 60,
            quote_tags: &[("Team", "platform"), ("Phase", "two")],
        },
        ReservationCase {
            name: "a commitment that is not a whole number of hours",
            instance_type: "t3.micro",
            instance_count: 3,
            lead_days: 6,
            // 90 minutes and one second: the charge is published in whole HOURS, so the part
            // hour has to be charged as one.
            commitment_seconds: 90 * 60 + 1,
            quote_tags: &[],
        },
        ReservationCase {
            name: "an accelerated family and the shortest commitment",
            instance_type: "g4dn.xlarge",
            instance_count: 2,
            lead_days: 11,
            commitment_seconds: 1,
            quote_tags: &[("Owner", "ml"), ("Phase", "one"), ("Cost", "centre-9")],
        },
    ]
}

/// A whole second `lead_days` and a minute from now, so the timestamp a request sends and the
/// instant the service stores are the same to the second rather than to a fraction of one, and
/// the start date is strictly inside the window a future-dated reservation may start in ("5 and
/// 120 days in advance", `CreateCapacityReservationRequest$StartDate`) however long the call
/// takes to arrive.
fn start_date(lead_days: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(now + lead_days * 24 * 60 * 60 + 60)
}

/// The form fields that create the future-dated reservation of `case`, starting at `start`.
fn reservation_params(case: &ReservationCase, start: SystemTime) -> Vec<(String, String)> {
    vec![
        ("InstanceType".to_string(), case.instance_type.to_string()),
        ("InstancePlatform".to_string(), INSTANCE_PLATFORM.to_string()),
        ("InstanceCount".to_string(), case.instance_count.to_string()),
        (
            "AvailabilityZone".to_string(),
            AVAILABILITY_ZONE.to_string(),
        ),
        (
            "InstanceMatchCriteria".to_string(),
            INSTANCE_MATCH_CRITERIA_TARGETED.to_string(),
        ),
        ("StartDate".to_string(), iso8601_utc(start)),
        (
            "CommitmentDuration".to_string(),
            case.commitment_seconds.to_string(),
        ),
        (
            "DeliveryPreference".to_string(),
            DELIVERY_PREFERENCE.to_string(),
        ),
    ]
}

/// The form fields of a tag specification on the quote.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut fields = vec![(
        "TagSpecification.1.ResourceType".to_string(),
        "capacity-reservation-cancellation-quote".to_string(),
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        fields.push((
            format!("TagSpecification.1.Tag.{}.Key", index + 1),
            (*key).to_string(),
        ));
        fields.push((
            format!("TagSpecification.1.Tag.{}.Value", index + 1),
            (*value).to_string(),
        ));
    }
    fields
}

/// Create the future-dated Capacity Reservation of `case` OVER THE WIRE and answer its id,
/// once the clock has moved it past the assessment every future-dated request starts in.
async fn create_future_dated(
    router: &MultiServiceRouter,
    case: &ReservationCase,
    start: SystemTime,
) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateCapacityReservation",
        &reservation_params(case, start),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the reservation to be created, got {status} with body {body}",
        case.name
    );
    let id = element(&body, "capacityReservationId").unwrap_or_else(|| {
        panic!("case {:?}: expected a reservation id in {body}", case.name)
    });
    // "Amazon EC2 is assessing your request for a future-dated Capacity Reservation"
    // (`CapacityReservation$State`) until the move to `scheduled` falls due, so the quote
    // below is generated against a reservation whose state is settled rather than racing it.
    tokio::time::sleep(past_the_transition()).await;
    id
}

/// One entry of a quote's `cancellationTermSet`, as the response reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedTerms {
    cancellation_type: String,
    reservation_state: String,
    committed_instance_count: String,
    charge_commitment_duration_hours: String,
    charge_end_date: String,
}

/// One cancellation quote, as the response reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedQuote {
    id: String,
    capacity_reservation_id: String,
    create_time: String,
    expiration_time: String,
    quote_state: String,
    configuration_instance_count: String,
    configuration_reservation_state: String,
    terms: Vec<ReportedTerms>,
    tags: Vec<(String, String)>,
}

/// Every quote a response carries, in the order it reported them.
///
/// A quote is the element that has a `capacityReservationCancellationQuoteId` child, so the
/// one reading serves the generate's single quote and the describe's set alike. Read with the
/// XML parser rather than by substring, because a quote's `reservationState` and its terms'
/// are different members of the same name.
fn reported_quotes(case: &str, body: &str) -> Vec<ReportedQuote> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element() && child(*node, "capacityReservationCancellationQuoteId").is_some()
        })
        .map(|node| {
            let text = |name: &str| child_text(node, name).unwrap_or_default();
            let configuration = child(node, "currentConfiguration");
            let terms = set_items(node, "cancellationTermSet")
                .into_iter()
                .map(|item| ReportedTerms {
                    cancellation_type: child_text(item, "cancellationType").unwrap_or_default(),
                    reservation_state: child_text(item, "reservationState").unwrap_or_default(),
                    committed_instance_count: child_text(item, "committedInstanceCount")
                        .unwrap_or_default(),
                    charge_commitment_duration_hours: child_text(
                        item,
                        "chargeCommitmentDurationHours",
                    )
                    .unwrap_or_default(),
                    charge_end_date: child_text(item, "chargeEndDate").unwrap_or_default(),
                })
                .collect();
            let tags = set_items(node, "tagSet")
                .into_iter()
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect();
            ReportedQuote {
                id: text("capacityReservationCancellationQuoteId"),
                capacity_reservation_id: text("capacityReservationId"),
                create_time: text("createTime"),
                expiration_time: text("expirationTime"),
                quote_state: text("quoteState"),
                configuration_instance_count: configuration
                    .and_then(|configuration| child_text(configuration, "instanceCount"))
                    .unwrap_or_default(),
                configuration_reservation_state: configuration
                    .and_then(|configuration| child_text(configuration, "reservationState"))
                    .unwrap_or_default(),
                terms,
                tags,
            }
        })
        .collect()
}

/// The ONE quote a response carries, or a panic naming the case.
fn reported_quote(case: &str, body: &str) -> ReportedQuote {
    let mut quotes = reported_quotes(case, body);
    assert_eq!(
        quotes.len(),
        1,
        "case {case:?}: expected exactly one quote, got {} in {body}",
        quotes.len()
    );
    quotes.remove(0)
}

/// An instant a response reported, as the seconds since the epoch it names.
fn at(case: &str, member: &str, value: &str) -> f64 {
    as_timestamp(value).unwrap_or_else(|| {
        panic!("case {case:?}: expected <{member}> to be a timestamp, got {value:?}")
    })
}

/// How many quotes the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_reservation_cancellation_quotes
        .len()
}

/// The `QuoteState` WORD the store holds for `quote_id` — what a writer put there, not what a
/// reader derives.
fn stored_quote_state(engine: &engine::InMemoryEngine, quote_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_reservation_cancellation_quotes
        .get(quote_id)
        .map(|record| record.quote_state().to_string())
        .unwrap_or_default()
}

/// Write one Capacity Reservation straight into the engine's store.
///
/// Only for the commitments no implemented operation can leave behind: a commitment that has
/// already ELAPSED cannot be created over the wire, a future-dated reservation starting at
/// least five days out. Every other row goes through `CreateCapacityReservation`.
fn seed_reservation(engine: &engine::InMemoryEngine, spec: CapacityReservationSpec) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_capacity_reservation(spec)
        .expect("the store accepts a seeded Capacity Reservation")
}

/// The reservations created for IMMEDIATE use that commit to nothing, as
/// `(case, instance type, instance count, end date)`. The rows differ along what the rule
/// generalises over: how much capacity is reserved, which instance family reserves it, and
/// whether the reservation carries an end date ("limited") or none at all ("unlimited",
/// `CreateCapacityReservationRequest$EndDateType`).
fn immediate_use_cases() -> Vec<(&'static str, &'static str, i32, Option<SystemTime>)> {
    let in_two_days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 2 * 24 * 60 * 60;
    vec![
        ("one instance for immediate use", "c5.xlarge", 1, None),
        ("twenty-five instances for immediate use", "m5.large", 25, None),
        (
            "the largest capacity a request may reserve",
            "t3.micro",
            1000,
            None,
        ),
        (
            "capacity for immediate use with an end date",
            "g4dn.xlarge",
            2,
            Some(UNIX_EPOCH + Duration::from_secs(in_two_days)),
        ),
    ]
}

/// A reservation of `instance_count` instances of `instance_type` whose commitment ended
/// `ago` before now, in the `active` state it is in after delivery.
fn elapsed_commitment_spec(
    ago: Duration,
    instance_type: &str,
    instance_count: i32,
) -> CapacityReservationSpec {
    let now = SystemTime::now();
    let ended = now.checked_sub(ago).expect("a representable past instant");
    CapacityReservationSpec {
        instance_type: instance_type.to_string(),
        instance_platform: INSTANCE_PLATFORM.to_string(),
        availability_zone: AVAILABILITY_ZONE.to_string(),
        availability_zone_id: "use1-az1".to_string(),
        region: REGION.to_string(),
        tenancy: "default".to_string(),
        instance_count,
        ebs_optimized: false,
        ephemeral_storage: false,
        end_date: None,
        instance_match_criteria: INSTANCE_MATCH_CRITERIA_TARGETED.to_string(),
        created_at: now,
        start_date: Some(ended),
        commitment: Some(CapacityReservationCommitment {
            duration_seconds: 60 * 60,
            end_date: ended,
        }),
        delivery_preference: Some(DELIVERY_PREFERENCE.to_string()),
        state: CAPACITY_RESERVATION_STATE_ACTIVE,
        tags: Vec::new(),
        interruption_info: None,
        zero_size_preference: None,
        pending: None,
    }
}

// ---------------------------------------------------------------------------
// What a generated quote carries
// ---------------------------------------------------------------------------

/// Every member of a generated quote is computed from the reservation's stored commitment and
/// capacity, and the quote is readable back through the describe.
#[tokio::test]
async fn a_quote_is_computed_from_the_reservations_commitment() {
    let (router, engine) = super::ec2_query::emulator();
    let mut quotes_stored = 0usize;

    for case in reservation_cases() {
        let name = case.name;
        let start = start_date(case.lead_days);
        let reservation_id = create_future_dated(&router, &case, start).await;

        let mut params = quote_params(&reservation_id);
        params.extend(tag_params(case.quote_tags));
        let (status, body) = generate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}"
        );
        let quote = reported_quote(name, &body);

        assert!(
            is_long_id(CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX, &quote.id),
            "case {name:?}: expected a \
             {CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX}-<17 hex digits> quote id, \
             got {:?}",
            quote.id
        );
        assert_eq!(
            quote.capacity_reservation_id, reservation_id,
            "case {name:?}: expected the quote to name the reservation it quotes, got {:?}",
            quote.capacity_reservation_id
        );
        // Rule 3's first half: the quote is valid for exactly its documented validity.
        let created = at(name, "createTime", &quote.create_time);
        let expires = at(name, "expirationTime", &quote.expiration_time);
        assert!(
            (expires - created - CAPACITY_RESERVATION_CANCELLATION_QUOTE_VALIDITY.as_secs_f64())
                .abs()
                < 1.0,
            "case {name:?}: expected the quote to expire {} seconds after it was created, \
             got createTime {created} and expirationTime {expires}",
            CAPACITY_RESERVATION_CANCELLATION_QUOTE_VALIDITY.as_secs()
        );
        assert_eq!(
            quote.quote_state, CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING,
            "case {name:?}: expected a freshly generated quote to be \
             {CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING}, got {:?}",
            quote.quote_state
        );

        // "The current configuration of the Capacity Reservation": the capacity it reserves
        // and the state it is in, which for a future-dated reservation whose assessment has
        // fallen due and whose start date is still ahead is `scheduled`.
        assert_eq!(
            quote.configuration_instance_count,
            case.instance_count.to_string(),
            "case {name:?}: expected the configuration to report the reserved capacity, \
             got {:?}",
            quote.configuration_instance_count
        );
        assert_eq!(
            quote.configuration_reservation_state, CAPACITY_RESERVATION_STATE_SCHEDULED,
            "case {name:?}: expected the configuration to report the reservation's state, \
             got {:?}",
            quote.configuration_reservation_state
        );

        // The terms: one charge type, the state the reservation moves to while the charges
        // are applied, the capacity that stays under commitment, the whole hours of it that
        // are left, and the instant the charge stops — the end of the commitment.
        assert_eq!(
            quote.terms.len(),
            1,
            "case {name:?}: expected one set of cancellation terms, got {:?}",
            quote.terms
        );
        let terms = &quote.terms[0];
        assert_eq!(
            terms.cancellation_type, APPLY_CANCELLATION_CHARGES_COMMITMENT_WIND_DOWN,
            "case {name:?}: expected the charge type \
             {APPLY_CANCELLATION_CHARGES_COMMITMENT_WIND_DOWN}, got {:?}",
            terms.cancellation_type
        );
        assert_eq!(
            terms.reservation_state, CAPACITY_RESERVATION_STATE_CANCELLING,
            "case {name:?}: expected the terms to say the reservation transitions to \
             {CAPACITY_RESERVATION_STATE_CANCELLING}, got {:?}",
            terms.reservation_state
        );
        assert_eq!(
            terms.committed_instance_count,
            case.instance_count.to_string(),
            "case {name:?}: expected the committed capacity to stay under commitment, \
             got {:?}",
            terms.committed_instance_count
        );
        let charge_end = at(name, "chargeEndDate", &terms.charge_end_date);
        let commitment_end = start
            .checked_add(Duration::from_secs(case.commitment_seconds))
            .expect("a representable commitment end")
            .duration_since(UNIX_EPOCH)
            .expect("a commitment end after the epoch")
            .as_secs_f64();
        assert!(
            (charge_end - commitment_end).abs() < 1.0,
            "case {name:?}: expected the charge to end when the commitment does \
             ({commitment_end}), got {charge_end}"
        );
        // The charge runs for what is left of the commitment, in whole hours rounded UP: a
        // part hour of commitment is still a charged hour.
        let expected_hours = ((charge_end - created) / 3600.0).ceil() as i64;
        assert_eq!(
            terms.charge_commitment_duration_hours,
            expected_hours.to_string(),
            "case {name:?}: expected the charge to run for the {expected_hours} whole hours \
             left of the commitment, got {:?}",
            terms.charge_commitment_duration_hours
        );
        assert_eq!(
            terms.charge_commitment_duration_hours.parse::<i64>().ok(),
            Some(expected_hours),
            "case {name:?}: expected the charge hours to be a number, got {:?}",
            terms.charge_commitment_duration_hours
        );

        // "The tags assigned to the cancellation quote", as the request asked for them.
        let expected_tags: Vec<(String, String)> = case
            .quote_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let mut reported_tags = quote.tags.clone();
        reported_tags.sort();
        let mut expected_sorted = expected_tags.clone();
        expected_sorted.sort();
        assert_eq!(
            reported_tags, expected_sorted,
            "case {name:?}: expected the quote to carry the tags the request asked for, \
             got {:?}",
            quote.tags
        );

        // Rule 4, the round trip: the describe reports the SAME quote, member for member,
        // beside the state the clock may have moved on.
        let (status, body) = describe(
            &router,
            &[(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                quote.id.clone(),
            )],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 reading the quote back, got {status} with \
             body {body}"
        );
        let read_back = reported_quote(name, &body);
        assert!(
            [
                CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING,
                CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE,
            ]
            .contains(&read_back.quote_state.as_str()),
            "case {name:?}: expected the quote read back to be pending or active, got {:?}",
            read_back.quote_state
        );
        assert_eq!(
            ReportedQuote {
                quote_state: quote.quote_state.clone(),
                ..read_back.clone()
            },
            quote,
            "case {name:?}: expected the describe to report the quote the generate stored, \
             got {read_back:?}"
        );

        // One generate, one stored quote: the generate wrote the quote it reported and
        // nothing beside it.
        quotes_stored += 1;
        assert_eq!(
            stored_count(&engine),
            quotes_stored,
            "case {name:?}: expected the store to hold {quotes_stored} quotes, got {}",
            stored_count(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// A quote exists only while the commitment is in force
// ---------------------------------------------------------------------------

/// A reservation that committed to nothing, one whose commitment has elapsed, and an id no
/// reservation of the account answers to are each refused, and nothing is stored for any of
/// them.
///
/// The two reservations with NO commitment terms in force are refused in the PARAMETER-VALUE
/// class, with the one fixed sentence AWS words that precondition as — nothing of the request
/// or of the store interpolated into it ([`NO_COMMITMENT_TERMS_CODE`],
/// [`NO_COMMITMENT_TERMS_MESSAGE`]).
#[tokio::test]
async fn only_a_reservation_under_a_live_commitment_can_be_quoted() {
    let (router, engine) = super::ec2_query::emulator();
    // Reservations created for IMMEDIATE use: they commit to nothing, so cancelling them
    // costs nothing to quote. The rows differ in how much capacity they hold, which instance
    // family holds it, and whether the reservation carries an end date at all.
    let mut needs_no_quote: Vec<(String, String)> = Vec::new();
    for (case, instance_type, instance_count, end_date) in immediate_use_cases() {
        let mut params = vec![
            ("InstanceType".to_string(), instance_type.to_string()),
            ("InstancePlatform".to_string(), INSTANCE_PLATFORM.to_string()),
            ("InstanceCount".to_string(), instance_count.to_string()),
            (
                "AvailabilityZone".to_string(),
                AVAILABILITY_ZONE.to_string(),
            ),
        ];
        if let Some(end_date) = end_date {
            params.push(("EndDateType".to_string(), "limited".to_string()));
            params.push(("EndDate".to_string(), iso8601_utc(end_date)));
        }
        let (status, body) =
            super::ec2_query::call(&router, REGION, "CreateCapacityReservation", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the reservation to be created with HTTP 200, got \
             {status} with body {body}"
        );
        let id = element(&body, "capacityReservationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a reservation id in {body}"));
        needs_no_quote.push((case.to_string(), id));
    }

    // Commitments the clock has already passed: past the commitment duration a cancellation
    // costs nothing either, so a quote is not needed for one of these either. Seeded, because
    // a commitment that has elapsed cannot be created over the wire; the rows differ in how
    // long ago it ended, how much capacity it holds and which family holds it.
    for (case, ago, instance_type, instance_count) in [
        (
            "a commitment that ended a second ago",
            Duration::from_secs(1),
            "c5.xlarge",
            4,
        ),
        (
            "a commitment that ended an hour ago",
            Duration::from_secs(60 * 60),
            "t3.micro",
            1,
        ),
        (
            "a commitment that ended a month ago",
            Duration::from_secs(30 * 24 * 60 * 60),
            "r5.large",
            64,
        ),
    ] {
        let id = seed_reservation(
            &engine,
            elapsed_commitment_spec(ago, instance_type, instance_count),
        );
        needs_no_quote.push((case.to_string(), id));
    }

    for (case, reservation_id) in &needs_no_quote {
        let (status, body) = generate(&router, &quote_params(reservation_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NO_COMMITMENT_TERMS_CODE,
            "case {case:?}: expected {NO_COMMITMENT_TERMS_CODE}, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, NO_COMMITMENT_TERMS_MESSAGE,
            "case {case:?}: expected {NO_COMMITMENT_TERMS_MESSAGE:?}, got {message:?}"
        );
        // The sentence is FIXED: it interpolates neither the id the request named nor the
        // state the reservation is in, both of which the caller reads elsewhere.
        assert!(
            !message.contains(reservation_id),
            "case {case:?}: expected the message to interpolate no reservation id, got \
             {message:?}"
        );
    }

    // An id no reservation answers to: a value of the Capacity Reservation id SHAPE the
    // account holds no reservation under is not found — the all-zero placeholder, the short
    // form and a long form whose leading digit is not zero included — and only a value
    // outside that shape is malformed, before any lookup. The rows differ in the FORM of the
    // id: its prefix, how many digits it carries, which digits, and their case.
    let known = seed_reservation(
        &engine,
        elapsed_commitment_spec(Duration::from_secs(1), "m5.large", 2),
    );
    let cases: [(&str, &str); 10] = [
        (UNKNOWN_CAPACITY_RESERVATION_ID, "NotFound"),
        ("cr-00000000000000000", "NotFound"),
        ("cr-12345678", "NotFound"),
        ("cr-00000000", "NotFound"),
        ("cr-1234567890abcdef0", "NotFound"),
        ("crcq-00000000000000001", "Malformed"),
        ("cr-0123456", "Malformed"),
        ("cr-0fffffffffffffffg", "Malformed"),
        ("CR-0FFFFFFFFFFFFFFFF", "Malformed"),
        ("not-an-id", "Malformed"),
    ];
    for (case, half) in cases {
        let expected = format!("InvalidCapacityReservationId.{half}");
        let (status, body) = generate(&router, &quote_params(case)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(case),
            "case {case:?}: expected the message to name the id, got {message:?}"
        );
    }
    assert!(
        !known.is_empty(),
        "the seeded reservation must have an id, got {known:?}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "a refused generate must store no quote"
    );
}

// ---------------------------------------------------------------------------
// The quote state follows the clock
// ---------------------------------------------------------------------------

/// A quote is generated `pending`, is `active` once the generation has fallen due, and is
/// `expired` from the instant it stops being valid — and a read reports all three without
/// writing any of them.
#[tokio::test]
async fn the_quote_state_follows_the_clock() {
    let (router, engine) = super::ec2_query::emulator();

    let case = ReservationCase {
        name: "the quote whose state the clock moves",
        instance_type: "m5.large",
        instance_count: 5,
        lead_days: 7,
        commitment_seconds: 3 * 24 * 60 * 60,
        quote_tags: &[],
    };
    let start = start_date(case.lead_days);
    let reservation_id = create_future_dated(&router, &case, start).await;
    let (status, body) = generate(&router, &quote_params(&reservation_id)).await;
    assert_eq!(
        status, 200,
        "generating the quote: expected HTTP 200, got {status} with body {body}"
    );
    let quote_id = reported_quote("generated", &body).id;
    assert_eq!(
        reported_quote("generated", &body).quote_state,
        CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING,
        "case {:?}: a quote is generated \
         {CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING}, got {body}",
        case.name
    );

    // The generation falls due on the clock, whether or not anybody asks again.
    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "reading the quote back: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        reported_quote("generated", &body).quote_state,
        CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE,
        "case {:?}: the clock owes a generated quote \
         {CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE}, got {body}",
        case.name
    );
    // The WORD the store holds has not changed: the state is derived from the instants the
    // record carries, so reading it writes nothing a later caller could see.
    assert_eq!(
        stored_quote_state(&engine, &quote_id),
        CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING,
        "a read must not write the state it derives"
    );

    // Past the documented validity, a quote is expired. Seeded, because the only way to
    // reach it over the wire is to wait a day; the rows differ in how far past it is.
    let quoted_configuration = CapacityReservationConfiguration {
        instance_count: Some(case.instance_count),
        reservation_state: Some(CAPACITY_RESERVATION_STATE_SCHEDULED.to_string()),
    };
    for (row, ago) in [
        (
            "a quote generated exactly its validity ago",
            CAPACITY_RESERVATION_CANCELLATION_QUOTE_VALIDITY,
        ),
        (
            "a quote generated a week ago",
            Duration::from_secs(7 * 24 * 60 * 60),
        ),
    ] {
        let expired_id = engine
            .state::<Ec2State>()
            .lock()
            .insert_capacity_reservation_cancellation_quote(
                CapacityReservationCancellationQuoteSpec {
                    capacity_reservation_id: reservation_id.clone(),
                    created_at: SystemTime::now()
                        .checked_sub(ago)
                        .expect("a representable past instant"),
                    current_configuration: quoted_configuration.clone(),
                    cancellation_terms: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .expect("the store accepts a seeded quote");
        let (status, body) = describe(
            &router,
            &[(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                expired_id.clone(),
            )],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            reported_quote(row, &body).quote_state,
            CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_EXPIRED,
            "case {row:?}: a quote past its validity is \
             {CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_EXPIRED}, got {body}"
        );
        assert_eq!(
            stored_quote_state(&engine, &expired_id),
            CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING,
            "case {row:?}: a read must not write the expiry it derives"
        );
    }
}

// ---------------------------------------------------------------------------
// What the describe narrows by
// ---------------------------------------------------------------------------

/// One row of the describe's selection table: what it is called, the form fields it sends and
/// the quote ids it must report.
type Selection = (&'static str, Vec<(String, String)>, Vec<String>);

/// One row of the describe's refusal table: what it is called, the form fields it sends and the
/// error code it must be refused with.
type Refusal = (&'static str, Vec<(String, String)>, &'static str);

/// The describe reports every stored quote in id order, narrows by the ids and the filters it
/// answers, pages with the token it mints, and refuses an id, a filter name or a token it
/// cannot resolve.
#[tokio::test]
async fn the_describe_narrows_by_id_filter_and_page() {
    let (router, _engine) = super::ec2_query::emulator();

    // Three quotes over three reservations, each with tags of its own, so every selector
    // below has something to include AND something to leave out.
    let mut generated: Vec<(String, String)> = Vec::new();
    for case in reservation_cases().into_iter().take(3) {
        let start = start_date(case.lead_days);
        let reservation_id = create_future_dated(&router, &case, start).await;
        let mut params = quote_params(&reservation_id);
        params.extend(tag_params(case.quote_tags));
        let (status, body) = generate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 generating the quote, got {status} with body {body}",
            case.name
        );
        generated.push((reported_quote(case.name, &body).id, reservation_id));
    }
    // Every quote is active once the generations have fallen due, which is what the
    // `quote-state` rows below select on.
    tokio::time::sleep(past_the_transition()).await;

    let all_ids: Vec<String> = {
        let mut ids: Vec<String> = generated.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids
    };

    /// The ids a describe reported, in the order it reported them.
    fn ids(case: &str, body: &str) -> Vec<String> {
        reported_quotes(case, body)
            .into_iter()
            .map(|quote| quote.id)
            .collect()
    }

    // Every quote, in `CapacityReservationCancellationQuoteId` order.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "every quote: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        ids("every quote", &body),
        all_ids,
        "every quote: expected all three quotes in id order, got {body}"
    );

    // The ids the request names, and nothing else.
    let (first_id, first_reservation) = generated[0].clone();
    let (second_id, _) = generated[1].clone();
    let selections: Vec<Selection> = vec![
        (
            "one id",
            vec![(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                first_id.clone(),
            )],
            vec![first_id.clone()],
        ),
        (
            "two ids",
            vec![
                (
                    "CapacityReservationCancellationQuoteId.1".to_string(),
                    second_id.clone(),
                ),
                (
                    "CapacityReservationCancellationQuoteId.2".to_string(),
                    first_id.clone(),
                ),
            ],
            {
                let mut both = vec![first_id.clone(), second_id.clone()];
                both.sort();
                both
            },
        ),
        (
            "the quote id filter",
            filters(&[(
                "capacity-reservation-cancellation-quote-id",
                &[first_id.as_str()],
            )]),
            vec![first_id.clone()],
        ),
        (
            "the reservation filter",
            filters(&[("capacity-reservation-id", &[first_reservation.as_str()])]),
            vec![first_id.clone()],
        ),
        (
            "a reservation nothing was quoted for",
            filters(&[(
                "capacity-reservation-id",
                &[UNKNOWN_CAPACITY_RESERVATION_ID],
            )]),
            Vec::new(),
        ),
        (
            "the state every quote has reached",
            filters(&[(
                "quote-state",
                &[CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE],
            )]),
            all_ids.clone(),
        ),
        (
            "a state no quote is in",
            filters(&[(
                "quote-state",
                &[CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_EXPIRED],
            )]),
            Vec::new(),
        ),
        (
            "a tag key and value",
            filters(&[("tag:Team", &["research"])]),
            vec![second_id.clone()],
        ),
        (
            "a tag key",
            filters(&[("tag-key", &["Phase"])]),
            vec![generated[2].0.clone()],
        ),
        (
            "two filters, joined with an AND",
            {
                let mut both = filters(&[("tag-key", &["Team"])]);
                both.extend(filters_at(2, "capacity-reservation-id", &[
                    first_reservation.as_str()
                ]));
                both
            },
            Vec::new(),
        ),
    ];
    for (case, params, expected) in selections {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            ids(case, &body),
            expected,
            "case {case:?}: expected the quotes {expected:?}, got {body}"
        );
    }

    // One quote per page: the token walks every quote exactly once and stops.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..all_ids.len() {
        let mut params = vec![("MaxResults".to_string(), "1".to_string())];
        if let Some(token) = token.clone() {
            params.push(("NextToken".to_string(), token));
        }
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "page {page}: expected HTTP 200, got {status} with body {body}"
        );
        let page_ids = ids("one quote per page", &body);
        assert_eq!(
            page_ids.len(),
            1,
            "page {page}: expected one quote on the page, got {page_ids:?}"
        );
        walked.extend(page_ids);
        token = element(&body, "nextToken");
        let last = page + 1 == all_ids.len();
        assert_eq!(
            token.is_none(),
            last,
            "page {page}: expected a token iff a quote follows the page, got {token:?}"
        );
    }
    assert_eq!(
        walked, all_ids,
        "paging must walk every quote once in id order, got {walked:?}"
    );

    // What the describe cannot resolve, it refuses.
    let refusals: Vec<Refusal> = vec![
        (
            "a quote id the account holds no quote for",
            vec![(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                UNKNOWN_QUOTE_ID.to_string(),
            )],
            "InvalidCapacityReservationCancellationQuoteId.NotFound",
        ),
        (
            "a quote id outside the issued form",
            vec![(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                "crcq-1234567890abcdef0".to_string(),
            )],
            "InvalidCapacityReservationCancellationQuoteId.Malformed",
        ),
        (
            "the null quote id",
            vec![(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                "crcq-00000000000000000".to_string(),
            )],
            "InvalidCapacityReservationCancellationQuoteId.Malformed",
        ),
        (
            "another resource's id",
            vec![(
                "CapacityReservationCancellationQuoteId.1".to_string(),
                first_reservation.clone(),
            )],
            "InvalidCapacityReservationCancellationQuoteId.Malformed",
        ),
        (
            "a filter this operation does not answer",
            filters(&[("quote-status", &["active"])]),
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["research"])]),
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint",
            vec![("NextToken".to_string(), "not-a-token".to_string())],
            "InvalidPaginationToken",
        ),
    ];
    for (case, params, expected) in refusals {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// The form fields for one `Filters` entry at `index`, counting from 1 — for a request that
/// carries more than one filter, where [`filters`] always starts at 1.
fn filters_at(index: usize, name: &str, values: &[&str]) -> Vec<(String, String)> {
    let mut fields = vec![(format!("Filter.{index}.Name"), name.to_string())];
    for (position, value) in values.iter().enumerate() {
        fields.push((
            format!("Filter.{index}.Value.{}", position + 1),
            (*value).to_string(),
        ));
    }
    fields
}

// ---------------------------------------------------------------------------
// Idempotency, dry runs and the tags a quote may carry
// ---------------------------------------------------------------------------

/// A `ClientToken` makes the generate idempotent: a replay reports the quote the first call
/// generated and stores nothing more, while the same token asking about another reservation is
/// told so.
#[tokio::test]
async fn the_client_token_makes_the_generate_idempotent() {
    let (router, engine) = super::ec2_query::emulator();

    let cases = reservation_cases();
    let first = &cases[0];
    let second = &cases[1];
    let first_reservation = create_future_dated(&router, first, start_date(first.lead_days)).await;
    let second_reservation =
        create_future_dated(&router, second, start_date(second.lead_days)).await;

    for (case, token, tags) in [
        ("a token with no tags", "token-plain", &[][..]),
        (
            "a token with tags",
            "token-tagged",
            &[("Team", "research")][..],
        ),
    ] {
        let mut params = quote_params(&first_reservation);
        params.push(("ClientToken".to_string(), token.to_string()));
        params.extend(tag_params(tags));

        let (status, body) = generate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let first_quote = reported_quote(case, &body);
        let stored = stored_count(&engine);

        let (status, body) = generate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replay to be served, got {status} with body {body}"
        );
        let replayed = reported_quote(case, &body);
        assert_eq!(
            replayed.id, first_quote.id,
            "case {case:?}: expected the replay to report the same quote, got {:?}",
            replayed.id
        );
        assert_eq!(
            replayed.create_time, first_quote.create_time,
            "case {case:?}: expected the replay to report the stored quote unchanged, \
             got {replayed:?}"
        );
        assert_eq!(
            stored_count(&engine),
            stored,
            "case {case:?}: a replay must store no second quote"
        );

        // The same token asking about a DIFFERENT reservation is not a replay.
        let mut other = quote_params(&second_reservation);
        other.push(("ClientToken".to_string(), token.to_string()));
        other.extend(tag_params(tags));
        let (status, body) = generate(&router, &other).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a mismatched replay, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );

        // So is the same token asking for different tags.
        let mut retagged = quote_params(&first_reservation);
        retagged.push(("ClientToken".to_string(), token.to_string()));
        retagged.extend(tag_params(&[("Team", "platform")]));
        let (status, body) = generate(&router, &retagged).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a retagged replay, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            stored,
            "case {case:?}: a refused replay must store no quote"
        );
    }
}

/// A dry run reports the outcome and writes nothing, and a tag specification naming anything
/// other than the quote is refused rather than accepted and discarded.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let cases = reservation_cases();
    let case = &cases[0];
    let reservation_id = create_future_dated(&router, case, start_date(case.lead_days)).await;

    let mut params = quote_params(&reservation_id);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = generate(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry run generate: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run generate: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "a dry run must store no quote"
    );

    let (status, body) = describe(
        &router,
        &[("DryRun".to_string(), "true".to_string())],
    )
    .await;
    assert_eq!(
        status, 412,
        "a dry run describe: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run describe: expected DryRunOperation, got {body}"
    );

    // A dry run that would have been REFUSED is refused with that reason instead.
    let mut refused = quote_params(UNKNOWN_CAPACITY_RESERVATION_ID);
    refused.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = generate(&router, &refused).await;
    assert_eq!(
        status, 400,
        "a dry run of a refused generate: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityReservationId.NotFound",
        "a dry run of a refused generate: expected the refusal it would have got, got {body}"
    );

    // The quote is the only resource this call creates, so a specification naming another
    // resource type is refused. The rows differ in which type they name.
    for other_type in ["capacity-reservation", "instance"] {
        let mut mistyped = quote_params(&reservation_id);
        mistyped.push((
            "TagSpecification.1.ResourceType".to_string(),
            other_type.to_string(),
        ));
        mistyped.push(("TagSpecification.1.Tag.1.Key".to_string(), "Team".to_string()));
        mistyped.push((
            "TagSpecification.1.Tag.1.Value".to_string(),
            "research".to_string(),
        ));
        let (status, body) = generate(&router, &mistyped).await;
        assert_eq!(
            status, 400,
            "case {other_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {other_type:?}: expected InvalidParameterValue, got {body}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        0,
        "a refused generate must store no quote"
    );
}

/// The tags a quote carries are reachable through the shared tag path too: a tag `CreateTags`
/// writes onto a quote is what the describe reports and what its `tag:` filters match on.
#[tokio::test]
async fn a_quotes_tags_are_the_ones_the_generate_asked_for() {
    let (router, _engine) = super::ec2_query::emulator();

    let cases = reservation_cases();
    let case = &cases[2];
    let reservation_id = create_future_dated(&router, case, start_date(case.lead_days)).await;
    let mut params = quote_params(&reservation_id);
    params.extend(tag_params(case.quote_tags));
    let (status, body) = generate(&router, &params).await;
    assert_eq!(
        status, 200,
        "generating a tagged quote: expected HTTP 200, got {status} with body {body}"
    );
    let quote = reported_quote(case.name, &body);

    let expected: Vec<(String, String)> = {
        let mut tags: Vec<(String, String)> = case
            .quote_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        tags.sort();
        tags
    };
    let mut reported = quote.tags.clone();
    reported.sort();
    assert_eq!(
        reported, expected,
        "expected the generate's tags on the quote, got {:?}",
        quote.tags
    );

    // One more tag, written through the shared tag path rather than by the generate: the
    // quote is a taggable resource of this store like any other.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateTags",
        &[
            ("ResourceId.1".to_string(), quote.id.clone()),
            ("Tag.1.Key".to_string(), "Stage".to_string()),
            ("Tag.1.Value".to_string(), "review".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "tagging the quote: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = describe(
        &router,
        &[(
            "CapacityReservationCancellationQuoteId.1".to_string(),
            quote.id.clone(),
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the quote's tags back: expected HTTP 200, got {status} with body {body}"
    );
    let tagged = reported_quote("the tagged quote", &body);
    let mut after = tagged.tags.clone();
    after.sort();
    let mut expected_after = expected.clone();
    expected_after.push(("Stage".to_string(), "review".to_string()));
    expected_after.sort();
    assert_eq!(
        after, expected_after,
        "expected the describe to report every tag the quote carries, got {:?}",
        tagged.tags
    );

    // And the tag filters match on what was written, whichever side wrote it.
    for (case, entries) in [
        ("the tag CreateTags wrote", vec![("tag:Stage", &["review"][..])]),
        (
            "a tag the generate wrote",
            vec![("tag:Team", &["platform"][..])],
        ),
        ("a tag key", vec![("tag-key", &["Stage"][..])]),
    ] {
        let (status, body) = describe(&router, &filters(&entries)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let selected: Vec<String> = reported_quotes(case, &body)
            .into_iter()
            .map(|quote| quote.id)
            .collect();
        assert_eq!(
            selected,
            vec![quote.id.clone()],
            "case {case:?}: expected the filter to select the tagged quote, got {body}"
        );
    }
}
