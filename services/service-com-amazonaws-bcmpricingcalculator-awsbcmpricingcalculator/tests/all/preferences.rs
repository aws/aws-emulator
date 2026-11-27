// SPDX-License-Identifier: CC0-1.0

//! The rate-type preferences plane (`UpdatePreferences` / `GetPreferences`), driven
//! end-to-end over this service's awsJson1_0 wire through the same router the
//! production front door uses.
//!
//! The RULE every suite here asserts: `GetPreferences` reports exactly the rate-type
//! record `UpdatePreferences` last stored, member by member — a supplied selection
//! replaces the stored one, an omitted one keeps its stored value, and all three
//! selections are reported whether or not anybody ever wrote them (an account that never
//! wrote one has the effective selection `state::DEFAULT_RATE_TYPE_SELECTION`). The rows
//! of each table differ along what the rule generalises over: which members a request
//! carries, which `RateType` values, how many of them, and in which order.
//!
//! Writing the record is gated on the caller's account class; these suites therefore
//! drive an emulator that may write (`Emulator::new`). The gate is pinned in
//! `account_class_gate`.

use serde_json::{json, Map, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::state;

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// Every value of the `RateType` enum, from the model (smithy_query:
/// `shape="RateType"` → `[BEFORE_DISCOUNTS, AFTER_DISCOUNTS,
/// AFTER_DISCOUNTS_AND_COMMITMENTS]`).
///
/// `pub(crate)`: every suite of this target that names a rate type reads it here, so one
/// catalog answers for all of them.
pub(crate) const RATE_TYPES: [&str; 3] = [
    "BEFORE_DISCOUNTS",
    "AFTER_DISCOUNTS",
    "AFTER_DISCOUNTS_AND_COMMITMENTS",
];

/// The three selection members `GetPreferencesResponse` and `UpdatePreferencesRequest`
/// each declare (smithy_query: `shape="GetPreferencesResponse"`), every one of them a
/// `RateTypes` list of length 1..3.
const SELECTION_MEMBERS: [&str; 3] = [
    "managementAccountRateTypeSelections",
    "memberAccountRateTypeSelections",
    "standaloneAccountRateTypeSelections",
];

/// The published bounds of `RateTypes` (smithy_query: `shape="RateTypes"` → `length
/// {min: 1, max: 3}`).
const RATE_TYPES_MIN: usize = 1;
const RATE_TYPES_MAX: usize = 3;

/// A preferences record as a JSON object, from one selection per member.
fn record(management: &[&str], member: &[&str], standalone: &[&str]) -> Value {
    json!({
        SELECTION_MEMBERS[0]: management,
        SELECTION_MEMBERS[1]: member,
        SELECTION_MEMBERS[2]: standalone,
    })
}

/// Assert `body` reports exactly `expected` for the three selection members: the same
/// values, in the same order, and nothing where `expected` has nothing.
fn assert_reports(case: &str, body: &Value, expected: &Value) {
    for member in SELECTION_MEMBERS {
        assert_eq!(
            body.get(member),
            expected.get(member),
            "case {case:?} member {member}: expected {:?}, got {:?} (whole body {body})",
            expected.get(member),
            body.get(member)
        );
    }
}

/// `GetPreferences` on `emulator`, asserting it succeeded, and its body.
async fn get_preferences(emulator: &Emulator, case: &str) -> Value {
    let (status, body) = emulator.call("GetPreferences", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPreferences expected 200, got {status} with {body}"
    );
    assert!(
        body.is_object(),
        "case {case:?}: GetPreferences expected a JSON object body, got {body}"
    );
    body
}

#[tokio::test]
async fn a_store_nobody_wrote_reports_the_effective_selection_and_a_read_changes_nothing() {
    let emulator = Emulator::new();

    // Two consecutive reads of an untouched store: a reporting operation must not
    // change what the next caller sees, and an account that never stated a preference
    // still has the rate types its bills are quoted with — `RateTypes` is `@length 1..3`,
    // so there is no legitimate empty or absent selection to report.
    let effective = json!([state::DEFAULT_RATE_TYPE_SELECTION]);
    for read in ["first read", "second read"] {
        let body = get_preferences(&emulator, read).await;
        for member in SELECTION_MEMBERS {
            assert_eq!(
                body.get(member),
                Some(&effective),
                "case {read:?} member {member}: expected {effective}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_stored_record_reads_back_exactly() {
    let emulator = Emulator::new();

    // Rows differ along value (each `RateType` of the catalog appears, including
    // `AFTER_DISCOUNTS_AND_COMMITMENTS`), count (1, 2 and the upper bound 3) and
    // ordering (a selection is a list, so its order is reported as stored, never
    // sorted). Each row carries all three members, so the record it stores is the row
    // itself — which also exercises overwriting the row before it.
    let rows: [(&str, Value); 5] = [
        (
            "one rate type per member, a different one each",
            record(
                &[RATE_TYPES[0]],
                &[RATE_TYPES[1]],
                &[RATE_TYPES[2]],
            ),
        ),
        (
            "two rate types per member",
            record(
                &[RATE_TYPES[1], RATE_TYPES[0]],
                &[RATE_TYPES[2], RATE_TYPES[1]],
                &[RATE_TYPES[0], RATE_TYPES[2]],
            ),
        ),
        (
            "the whole catalog, in model order",
            record(&RATE_TYPES, &RATE_TYPES, &RATE_TYPES),
        ),
        (
            "the whole catalog, reversed — order is reported as stored",
            record(
                &[RATE_TYPES[2], RATE_TYPES[1], RATE_TYPES[0]],
                &[RATE_TYPES[2], RATE_TYPES[1], RATE_TYPES[0]],
                &[RATE_TYPES[2], RATE_TYPES[1], RATE_TYPES[0]],
            ),
        ),
        (
            "back to a single rate type per member",
            record(
                &[RATE_TYPES[2]],
                &[RATE_TYPES[0]],
                &[RATE_TYPES[1]],
            ),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = emulator
            .call("UpdatePreferences", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdatePreferences expected 200, got {status} with {body}"
        );
        // The update answers with the record it stored...
        assert_reports(case, &body, &request);
        // ...and that is the record the reader reports.
        let read = get_preferences(&emulator, case).await;
        assert_reports(case, &read, &request);
    }
}

#[tokio::test]
async fn a_member_a_request_omits_keeps_its_stored_value() {
    let emulator = Emulator::new();
    let baseline = record(&[RATE_TYPES[0]], &[RATE_TYPES[1]], &[RATE_TYPES[2]]);

    // One row per member updated alone, plus the degenerate request that carries no
    // member at all: in every case the members the request omits must keep exactly
    // what the baseline stored.
    let mut cases: Vec<(String, Value)> = vec![("no member at all".to_string(), json!({}))];
    for member in SELECTION_MEMBERS {
        cases.push((
            format!("{member} alone, to the whole catalog"),
            json!({ member: RATE_TYPES }),
        ));
        cases.push((
            format!("{member} alone, to one rate type"),
            json!({ member: [RATE_TYPES[1]] }),
        ));
    }

    for (case, request) in cases {
        // Re-establish the baseline so each row starts from the same store.
        let (status, body) = emulator
            .call("UpdatePreferences", baseline.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: baseline UpdatePreferences expected 200, got {status} with {body}"
        );

        let mut expected = baseline.clone();
        let supplied: &Map<String, Value> = request
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the request is a JSON object"));
        for (member, selection) in supplied {
            expected[member.as_str()] = selection.clone();
        }

        let (status, body) = emulator.call("UpdatePreferences", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdatePreferences expected 200, got {status} with {body}"
        );
        assert_reports(&case, &body, &expected);
        let read = get_preferences(&emulator, &case).await;
        assert_reports(&case, &read, &expected);
    }
}

#[tokio::test]
async fn the_same_request_sent_twice_stores_the_same_record() {
    let emulator = Emulator::new();
    let request = record(
        &[RATE_TYPES[1], RATE_TYPES[2]],
        &[RATE_TYPES[0]],
        &RATE_TYPES,
    );

    // `UpdatePreferences` is `@idempotent`: replaying it leaves the same record, and
    // the reader keeps agreeing with it.
    for attempt in ["first send", "second send"] {
        let (status, body) = emulator
            .call("UpdatePreferences", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {attempt:?}: UpdatePreferences expected 200, got {status} with {body}"
        );
        assert_reports(attempt, &body, &request);
        let read = get_preferences(&emulator, attempt).await;
        assert_reports(attempt, &read, &request);
    }
}

#[tokio::test]
async fn a_selection_outside_the_modelled_constraints_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let baseline = record(&[RATE_TYPES[0]], &[RATE_TYPES[1]], &[RATE_TYPES[2]]);
    let (status, body) = emulator
        .call("UpdatePreferences", baseline.clone())
        .await;
    assert_eq!(
        status, 200,
        "baseline UpdatePreferences: expected 200, got {status} with {body}"
    );

    // Each published bound of `RateTypes` is accepted; one entry past each bound, and a
    // value outside the `RateType` catalog, is refused — for EVERY selection member,
    // not just the one a recording happened to carry.
    let too_many: Vec<&str> = (0..=RATE_TYPES_MAX)
        .map(|index| RATE_TYPES[index % RATE_TYPES.len()])
        .collect();
    assert_eq!(
        too_many.len(),
        RATE_TYPES_MAX + 1,
        "the over-long selection must hold one entry past the maximum, got {too_many:?}"
    );
    let at_min: Vec<&str> = RATE_TYPES.iter().copied().take(RATE_TYPES_MIN).collect();
    let at_max: Vec<&str> = RATE_TYPES.iter().copied().take(RATE_TYPES_MAX).collect();

    // What the store holds, carried across the rows: a refused row must leave it
    // exactly as it was, and an accepted row must replace just that one member.
    let mut stored = baseline.clone();

    for member in SELECTION_MEMBERS {
        for (case, selection, expected_status) in [
            ("empty, one below the minimum", Vec::new(), 400),
            ("at the minimum length", at_min.clone(), 200),
            ("at the maximum length", at_max.clone(), 200),
            ("one past the maximum length", too_many.clone(), 400),
            (
                "a rate type outside the catalog",
                vec!["AFTER_DISCOUNTS_AND_SOMETHING"],
                400,
            ),
            ("the catalog in the wrong case", vec!["after_discounts"], 400),
        ] {
            let case = format!("{member}: {case}");
            let (status, body) = emulator
                .call("UpdatePreferences", json!({ member: selection }))
                .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status} with {body}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body}"
                );
            } else {
                stored[member] = json!(selection);
                assert_reports(&case, &body, &stored);
            }
            // Either way, the reader reports the record the accepted calls left: a
            // refused request stores nothing.
            let read = get_preferences(&emulator, &case).await;
            assert_reports(&case, &read, &stored);
        }
    }
}
