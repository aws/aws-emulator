// SPDX-License-Identifier: CC0-1.0

//! The values this service MINTS for a zonal shift — its identifier and the two instants
//! it reports — driven end to end over the wire.
//!
//! Every case builds the production `MultiServiceRouter`, registers this service on one
//! shared engine and dispatches real `restJson1` requests, so what is pinned is the
//! document a client parses rather than a handler's return value.
//!
//! Two rules, each asserted over a SET of inputs:
//!
//! * `zonalShiftId` is ISSUED BY THE SERVICE (no operation takes one as an input member),
//!   and it is issued in the identifier form ARC fills the modelled 36-character maximum
//!   of `ZonalShiftId` with: a version-4 UUID, lowercase hexadecimal in the hyphenated
//!   `8-4-4-4-12` groups. No two shifts share one, whichever store or process minted
//!   them, and the one shift reports the same id at every site that reports it.
//! * every `timestamp` member of this model carries no `@timestampFormat`, so each is
//!   serialized in the default document format of the service's protocol (`restJson1`): a
//!   plain JSON number of seconds since the epoch, written — as AWS writes one — to at
//!   most millisecond resolution. The length of time between the two instants of a
//!   customer-initiated shift is therefore exactly the whole number of seconds
//!   `expiresIn` names.
//!
//! The sites asserted at are every one this service reports a shift from:
//! `StartZonalShift`, `UpdateZonalShift`, `CancelZonalShift`, `ListZonalShifts`,
//! `GetManagedResource`, `ListManagedResources`, `StartPracticeRun` and
//! `CancelPracticeRun`.
//!
//! The rows differ along what the rules generalise over: the resource shifted (seeded into
//! this store or registered by the service that owns it) and how many zones it spans, the
//! zone traffic moves away from, the UNIT and MAGNITUDE of `expiresIn` — each end of the
//! documented range, a whole hour, and a value whose minutes are not a whole hour — and
//! the TYPE of shift (customer-initiated and practice run).

use std::collections::BTreeSet;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::practice_run_cancel::cancel_practice_run;
use super::practice_run_configuration::get_managed_resource;
use super::practice_run_start::{given_configuration, start_practice_run};
use super::registered_managed_resources::register;
use super::zonal_shift_lifecycle::{call, emulator, encoded, seeded, served, start_body};

/// The alphabet and the length the model gives `ZonalShiftId`: `^[A-Za-z0-9-]+$`, length
/// 6..36 (`smithy_query(query="shape", shape="ZonalShiftId")`).
const ID_ALPHABET_EXTRA: char = '-';
const ID_MIN_LENGTH: usize = 6;
const ID_MAX_LENGTH: usize = 36;

/// The group widths of UUID text: `8-4-4-4-12` hexadecimal digits.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// The most fractional digits an AWS epoch-seconds timestamp is written with: whole
/// milliseconds.
const MAX_FRACTION_DIGITS: usize = 3;

/// The two instants every zonal shift ARC reports carries.
const INSTANT_MEMBERS: [&str; 2] = ["startTime", "expiryTime"];

/// Assert that `id` is an identifier of the form ARC issues for a zonal shift, and return
/// it.
fn assert_issued_id(case: &str, site: &str, body: &Value) -> String {
    let id = body["zonalShiftId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {site} must report a zonalShiftId, got {body}"))
        .to_string();

    // The modelled constraints first: whatever form the id takes, it has to be a value the
    // shape admits.
    assert!(
        (ID_MIN_LENGTH..=ID_MAX_LENGTH).contains(&id.chars().count()),
        "case {case:?}: {site} must report an id of {ID_MIN_LENGTH}..{ID_MAX_LENGTH} \
         characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ID_ALPHABET_EXTRA),
        "case {case:?}: {site} must report an id over the modelled alphabet, got {id:?}"
    );

    // Then the form ARC issues: UUID text, in the version-4 layout.
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        UUID_GROUPS.to_vec(),
        "case {case:?}: {site} must report a uuid-shaped id, got {id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())),
        "case {case:?}: {site} must report a lowercase-hexadecimal id, got {id:?}"
    );
    assert_eq!(
        groups[2].chars().next(),
        Some('4'),
        "case {case:?}: {site} must report an id in the version-4 layout, got {id:?}"
    );
    assert!(
        matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
        "case {case:?}: {site} must report an id with the uuid variant bits, got {id:?}"
    );
    id
}

/// The decimal fraction of a number AS SERVED: the digits after the point of the literal
/// the document carries, with trailing zeros dropped.
///
/// Taken from the number's own rendering rather than from a reparse to a fixed precision,
/// so a surplus digit is visible here instead of being rounded away by the assertion.
fn fraction_digits(case: &str, site: &str, member: &str, number: &serde_json::Number) -> usize {
    let rendered = number.to_string();
    assert!(
        !rendered.contains(['e', 'E']),
        "case {case:?}: {site} must write {member} in plain decimal seconds, got {rendered}"
    );
    rendered
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.trim_end_matches('0').len())
}

/// Assert that `body`'s `member` is an epoch-seconds number of the shape AWS serializes,
/// and return the instant it names in seconds.
fn assert_instant(case: &str, site: &str, body: &Value, member: &str) -> f64 {
    let Value::Number(number) = &body[member] else {
        panic!("case {case:?}: {site} must report {member} as a JSON number, got {body}")
    };
    let digits = fraction_digits(case, site, member, number);
    assert!(
        digits <= MAX_FRACTION_DIGITS,
        "case {case:?}: {site} must write {member} to at most {MAX_FRACTION_DIGITS} fractional \
         digits, got {number} with {digits}"
    );
    let seconds = number.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {site} must report {member} as a finite number, got {number}")
    });
    assert!(
        seconds > 0.0,
        "case {case:?}: {site} must report {member} as an instant after the epoch, got {seconds}"
    );
    seconds
}

/// Assert both instants of one reported shift, and return them as `(startTime, expiryTime)`.
fn assert_instants(case: &str, site: &str, body: &Value) -> (f64, f64) {
    let start = assert_instant(case, site, body, INSTANT_MEMBERS[0]);
    let expiry = assert_instant(case, site, body, INSTANT_MEMBERS[1]);
    assert!(
        expiry > start,
        "case {case:?}: {site} must report an expiry after the start, got {start} and {expiry}"
    );
    (start, expiry)
}

/// The one entry of `items` that reports the shift `id`, which is how a shift is picked
/// out of a listing that also carries the shifts started before it.
fn shift_with<'a>(case: &str, site: &str, items: &'a [Value], id: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["zonalShiftId"] == id)
        .unwrap_or_else(|| panic!("case {case:?}: {site} must report the shift {id}, got {items:?}"))
}

/// `ListZonalShifts` items for one resource.
async fn shifts_of(router: &MultiServiceRouter, arn: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/zonalshifts?resourceIdentifier={}", encoded(arn)),
        "",
    )
    .await;
    assert_eq!(status, 200, "ListZonalShifts for {arn}: got {status} {body}");
    body["items"].as_array().cloned().unwrap_or_default()
}

/// The `ListManagedResources` entry for one resource, walking the pages it hands out.
async fn listed_resource(router: &MultiServiceRouter, arn: &str) -> Value {
    let mut query = String::new();
    loop {
        let (status, body) = call(router, "GET", &format!("/managedresources{query}"), "").await;
        assert_eq!(status, 200, "ListManagedResources{query}: got {status} {body}");
        let items = body["items"].as_array().cloned().unwrap_or_default();
        if let Some(found) = items.into_iter().find(|item| item["arn"] == arn) {
            return found;
        }
        let Some(token) = body["nextToken"].as_str() else {
            panic!("ListManagedResources never reported {arn}");
        };
        query = format!("?nextToken={}", encoded(token));
    }
}

/// One resource a shift is started for: the ARN naming it and the zones it is deployed in.
struct Row {
    case: &'static str,
    arn: String,
    away_from: String,
}

/// The resources every case below shifts: the ones this store seeds, plus resources the
/// services that own them registered, spanning one, two and three Availability Zones.
fn rows(engine: &engine::InMemoryEngine) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (index, (arn, zones)) in seeded(engine).into_iter().enumerate().take(2) {
        // A different zone per row, as far as the resource's own zones go.
        let away_from = zones
            .get(index.min(zones.len() - 1))
            .expect("a seeded resource is deployed in at least one zone")
            .clone();
        rows.push(Row {
            case: match index {
                0 => "the first resource this store seeds",
                _ => "the second resource this store seeds",
            },
            arn,
            away_from,
        });
    }
    for (case, arn, zones) in [
        (
            "a Network Load Balancer another service registered, one zone",
            "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/net/mint-one\
             /1a2b3c4d5e6f7890",
            vec!["usw2-az1"],
        ),
        (
            "an Auto Scaling group another service registered, three zones",
            "arn:aws:autoscaling:eu-central-1:000000000000:autoScalingGroup:\
             4e2b6e5a-9c1d-4f3a-8b7e-0d6c5a4b3e21:autoScalingGroupName/mint-three",
            vec!["euc1-az1", "euc1-az2", "euc1-az3"],
        ),
    ] {
        register(engine, arn, "mint-resource", &zones);
        rows.push(Row {
            case,
            arn: arn.to_string(),
            away_from: zones[zones.len() - 1].to_string(),
        });
    }
    rows
}

/// The lengths of time a shift is started for: each end of the documented range ("You can
/// set a zonal shift to be active initially for up to three days (72 hours)", the
/// `expiresIn` documentation), both units, and a value whose minutes are not a whole hour.
const EXPIRES_IN: [&str; 5] = ["1m", "25m", "90m", "1h", "72h"];

/// `expires_in` as the whole number of seconds it names, written out here rather than
/// parsed, so a row is an expectation and not a restatement of the implementation.
fn expires_in_seconds(expires_in: &str) -> f64 {
    match expires_in {
        "1m" => 60.0,
        "25m" => 25.0 * 60.0,
        "90m" => 90.0 * 60.0,
        "1h" => 60.0 * 60.0,
        "72h" => 72.0 * 60.0 * 60.0,
        other => panic!("no expected length of time is written down for {other:?}"),
    }
}

#[tokio::test]
async fn every_site_reports_a_shifts_instants_as_epoch_seconds_at_millisecond_resolution() {
    let (router, engine) = emulator();
    for row in rows(&engine) {
        for expires_in in EXPIRES_IN {
            let case = format!("{} for {expires_in}", row.case);
            let case = case.as_str();

            let (status, started) = call(
                &router,
                "POST",
                "/zonalshifts",
                &start_body(&row.arn, &row.away_from, expires_in, "minted instants"),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: StartZonalShift answers 201, got {status} {started}"
            );
            let (start, expiry) = assert_instants(case, "the StartZonalShift body", &started);

            // The expiry ARC converts `expiresIn` into stands exactly that length of time
            // after the start: the resolution the instants are written at cannot blur it.
            let expected = expires_in_seconds(expires_in);
            // A millisecond, the resolution both instants are written at: the difference
            // cannot be off by more than the digits the wire carries.
            assert!(
                (expiry - start - expected).abs() < 0.001,
                "case {case:?}: the expiry stands {expected} seconds after the start, got \
                 {} ({start} to {expiry})",
                expiry - start
            );

            let shift_id = assert_issued_id(case, "the StartZonalShift body", &started);

            // Every read of the shift reports the same two instants, in the same shape.
            let (status, view) = get_managed_resource(&router, &row.arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetManagedResource answers 200, got {status} {view}"
            );
            let in_resource = view["zonalShifts"].as_array().cloned().unwrap_or_default();
            let nested = shift_with(
                case,
                "the GetManagedResource zonalShifts entry",
                &in_resource,
                &shift_id,
            );
            assert_eq!(
                assert_instants(case, "the GetManagedResource zonalShifts entry", nested),
                (start, expiry),
                "case {case:?}: GetManagedResource reports the instants the shift was started \
                 with, got {nested}"
            );

            let item = listed_resource(&router, &row.arn).await;
            let in_item = item["zonalShifts"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                assert_instants(
                    case,
                    "the ListManagedResources zonalShifts entry",
                    shift_with(
                        case,
                        "the ListManagedResources zonalShifts entry",
                        &in_item,
                        &shift_id
                    )
                ),
                (start, expiry),
                "case {case:?}: ListManagedResources reports the same instants, got {item}"
            );

            let listed = shifts_of(&router, &row.arn).await;
            assert_eq!(
                assert_instants(
                    case,
                    "the ListZonalShifts items entry",
                    shift_with(case, "the ListZonalShifts items entry", &listed, &shift_id)
                ),
                (start, expiry),
                "case {case:?}: ListZonalShifts reports the same instants, got {listed:?}"
            );

            // The update: it moves the expiry, and the instant it moves it to is written in
            // the same shape.
            let (status, updated) = call(
                &router,
                "PATCH",
                &format!("/zonalshifts/{shift_id}"),
                &serde_json::json!({ "expiresIn": "2h" }).to_string(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateZonalShift answers 200, got {status} {updated}"
            );
            let (updated_start, _) = assert_instants(case, "the UpdateZonalShift body", &updated);
            assert_eq!(
                updated_start, start,
                "case {case:?}: an update does not move the start, got {updated}"
            );

            let (status, canceled) =
                call(&router, "DELETE", &format!("/zonalshifts/{shift_id}"), "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: CancelZonalShift answers 200, got {status} {canceled}"
            );
            let (canceled_start, _) = assert_instants(case, "the CancelZonalShift body", &canceled);
            assert_eq!(
                canceled_start, start,
                "case {case:?}: the cancel reports the start the shift was started at, got \
                 {canceled}"
            );
        }
    }
}

#[tokio::test]
async fn a_practice_runs_instants_take_the_same_shape() {
    // A practice run IS a zonal shift, so the instants of its own response, of the reads
    // that report it and of its cancel are written exactly as a customer-initiated
    // shift's are.
    let (router, engine) = emulator();
    for row in rows(&engine) {
        let case = row.case;
        given_configuration(&router, &row.arn, &[]).await;
        let (status, started) =
            start_practice_run(&router, &row.arn, &row.away_from, "minted instants").await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartPracticeRun answers 200, got {status} {started}"
        );
        let (start, _) = assert_instants(case, "the StartPracticeRun body", &started);
        let shift_id = assert_issued_id(case, "the StartPracticeRun body", &started);

        let listed = shifts_of(&router, &row.arn).await;
        let site = "the ListZonalShifts entry of the practice run";
        let (listed_start, _) =
            assert_instants(case, site, shift_with(case, site, &listed, &shift_id));
        assert_eq!(
            listed_start, start,
            "case {case:?}: the listing reports the start of the run, got {listed:?}"
        );

        let (status, canceled) = cancel_practice_run(&router, &shift_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CancelPracticeRun answers 200, got {status} {canceled}"
        );
        let (canceled_start, _) = assert_instants(case, "the CancelPracticeRun body", &canceled);
        assert_eq!(
            canceled_start, start,
            "case {case:?}: the cancel reports the start of the run, got {canceled}"
        );
    }
}

#[tokio::test]
async fn the_served_document_carries_each_instant_as_a_plain_decimal_literal() {
    // The assertions above read the instants out of a parse of the document; this one reads
    // the BYTES, so the literal a client's own parser sees is pinned too.
    let (router, engine) = emulator();
    for row in rows(&engine) {
        let case = row.case;
        let (status, _class, document) = served(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&row.arn, &row.away_from, "30m", "served literals"),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: StartZonalShift answers 201, got {status} {document}"
        );
        for member in INSTANT_MEMBERS {
            let needle = format!("\"{member}\":");
            let at = document.find(&needle).unwrap_or_else(|| {
                panic!("case {case:?}: the document must carry {member}, got {document}")
            });
            let literal: String = document[at + needle.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            assert!(
                !literal.is_empty(),
                "case {case:?}: {member} must be written as a number, got {document}"
            );
            let fraction = literal
                .split_once('.')
                .map_or("", |(_, fraction)| fraction)
                .trim_end_matches('0');
            assert!(
                fraction.len() <= MAX_FRACTION_DIGITS,
                "case {case:?}: {member} must be written to at most {MAX_FRACTION_DIGITS} \
                 fractional digits, got {literal}"
            );
        }
    }
}

#[tokio::test]
async fn one_shift_is_reported_under_one_issued_id_wherever_it_is_read() {
    let (router, engine) = emulator();
    for row in rows(&engine) {
        let case = row.case;
        let (status, started) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(&row.arn, &row.away_from, "1h", "one id per shift"),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: StartZonalShift answers 201, got {status} {started}"
        );
        let shift_id = assert_issued_id(case, "the StartZonalShift body", &started);

        let (_, view) = get_managed_resource(&router, &row.arn).await;
        let listed = shifts_of(&router, &row.arn).await;
        let item = listed_resource(&router, &row.arn).await;
        let (_, updated) = call(
            &router,
            "PATCH",
            &format!("/zonalshifts/{shift_id}"),
            &serde_json::json!({ "comment": "still the same shift" }).to_string(),
        )
        .await;
        let (_, canceled) =
            call(&router, "DELETE", &format!("/zonalshifts/{shift_id}"), "").await;

        let in_resource = view["zonalShifts"].as_array().cloned().unwrap_or_default();
        let in_item = item["zonalShifts"].as_array().cloned().unwrap_or_default();
        for (site, body) in [
            (
                "the GetManagedResource zonalShifts entry",
                shift_with(case, "GetManagedResource", &in_resource, &shift_id),
            ),
            (
                "the ListManagedResources zonalShifts entry",
                shift_with(case, "ListManagedResources", &in_item, &shift_id),
            ),
            (
                "the ListZonalShifts items entry",
                shift_with(case, "ListZonalShifts", &listed, &shift_id),
            ),
            ("the UpdateZonalShift body", &updated),
            ("the CancelZonalShift body", &canceled),
        ] {
            let reported = assert_issued_id(case, site, body);
            assert_eq!(
                reported, shift_id,
                "case {case:?}: {site} must report the id the shift was issued, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn no_two_shifts_are_issued_one_id() {
    // Two emulators, each with its own store: an id is not drawn from a sequence a fresh
    // store restarts, so the shifts of one store collide with neither each other's nor
    // another store's ids.
    let mut issued: BTreeSet<String> = BTreeSet::new();
    let mut minted = 0_usize;
    for store in 0..2 {
        let (router, engine) = emulator();
        for row in rows(&engine) {
            let case = format!("store {store}: {}", row.case);
            let case = case.as_str();
            // Two shifts per resource: the second is started after the first is canceled,
            // since one resource carries one customer-initiated shift at a time.
            for attempt in 0..2 {
                let (status, started) = call(
                    &router,
                    "POST",
                    "/zonalshifts",
                    &start_body(&row.arn, &row.away_from, "30m", "unique ids"),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?} attempt {attempt}: StartZonalShift answers 201, got {status} \
                     {started}"
                );
                let id = assert_issued_id(case, "the StartZonalShift body", &started);
                assert!(
                    issued.insert(id.clone()),
                    "case {case:?} attempt {attempt}: {id} was already issued to another shift"
                );
                minted += 1;
                let (status, canceled) =
                    call(&router, "DELETE", &format!("/zonalshifts/{id}"), "").await;
                assert_eq!(
                    status, 200,
                    "case {case:?} attempt {attempt}: CancelZonalShift answers 200, got {status} \
                     {canceled}"
                );
            }
        }
    }
    assert_eq!(
        issued.len(),
        minted,
        "every one of the {minted} shifts started must carry an id of its own, got \
         {} distinct ids",
        issued.len()
    );
}
