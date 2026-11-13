// SPDX-License-Identifier: CC0-1.0

//! The validation code `SendProcurementPortalValidation` issues is the thing a submission
//! has to MATCH before `VerifyProcurementPortalValidation` completes the validation of a
//! portal connection, and the real service delivers it OUT OF BAND — inside the test
//! e-invoice it posts to the configured portal — so the modelled response carries only the
//! preference's ARN (`smithy_query(query="operation",
//! operation="SendProcurementPortalValidation,VerifyProcurementPortalValidation")`).
//!
//! RULE: a code the service issues out of band and then gates a caller's verification on is
//! drawn, per issue, from a cryptographically secure random generator uniformly over its
//! whole value space — not from the position in a sequence of issues, from the clock or
//! from anything of the request — so a caller that never received the code cannot name it.
//!
//! Two suites span that rule, each over a table rather than one request:
//!
//! * the codes a run issues spread over the whole six-digit space and repeat no earlier
//!   code of the run — measured over draws taken across the portals the enum admits and
//!   across repeated sends against one preference;
//! * the codes a caller could NAME without receiving one — the position in the service's
//!   own sequence of issues, the position in a preference's sends, the bounds of the code
//!   space and a code the service issued for another preference — complete no validation:
//!   each is the modelled HTTP 400 `ValidationException`, every plane stays where it was,
//!   the code the send actually issued is what completes it, and a guess turns out to be
//!   the drawn code no more often than an independent draw lands on one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::refused_procurement_portal_writes::{
    arn_of, call, create_with_delivery_endpoint, emulator_with_engine, error_code, get_preference,
    issue_code, ok, PORTAL_NAMES,
};

/// How many decimal digits an issued validation code carries: the six-digit form the
/// portal-validation flow's `Code` is quoted in.
const CODE_DIGITS: usize = 6;

/// How many preferences the spread is measured across — one per portal the enum admits, so
/// the draw is shown not to depend on the portal.
const PREFERENCES: usize = 4;

/// How many codes are drawn per preference, so the draw is shown not to depend on the
/// position in one preference's own sequence of sends either.
const DRAWS_PER_PREFERENCE: usize = 16;

/// The codes a mint fed by a counter from zero occupies: the first thousand of the space.
/// A uniform draw lands there once in a thousand draws, so the whole batch landing there is
/// what a counter-fed mint looks like.
const LOW_BAND: u32 = 1_000;

/// How many of the batch's draws may fall inside [`LOW_BAND`] before the batch is not a
/// uniform draw: each draw lands there with probability `LOW_BAND / 10^CODE_DIGITS`, so a
/// batch of `PREFERENCES * DRAWS_PER_PREFERENCE` draws puts an expected 0.064 there and
/// this many independent ones cannot be reached in practice — while a counter-fed mint puts
/// every one of them there.
const TOLERATED_IN_LOW_BAND: usize = 8;

/// How many distinct LEADING digits the batch carries: a uniform draw spreads over all ten,
/// while a mint that reduces to a small counter spends the whole batch on one. Two would
/// already rule that out; three leaves room for an unlucky run.
const LEADING_DIGITS_AT_LEAST: usize = 3;

/// How many of the guesses below may turn out to BE the code the send drew before the
/// draws are not independent of the guess: a uniform draw over the whole space lands on a
/// named value once in `10^CODE_DIGITS` draws, so one coincidence is already generous for
/// the few dozen guesses this suite makes — while a mint that follows the issue sequence
/// coincides with that sequence's own guess on every single send.
const TOLERATED_COINCIDENCES: usize = 1;

/// The status a plane of a preference reports while nothing has verified its portal
/// connection, and the one a freshly created preference starts at: one of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The status a plane reports once its portal connection has been verified: another value
/// of that same enum.
const VALIDATED: &str = "VALIDATED";

/// The members a reader reports the verification state of each plane of a preference in.
const PLANE_STATUS_MEMBERS: [&str; 2] = [
    "EinvoiceDeliveryPreferenceStatus",
    "PurchaseOrderRetrievalPreferenceStatus",
];

/// `value`, zero-padded to the width an issued code is quoted in — the form a caller trying
/// to NAME a code would quote its guess in.
fn as_code(value: usize) -> String {
    format!("{value:0width$}", width = CODE_DIGITS)
}

/// One preference, created on `portal`, whose ARN the sends below name.
async fn preference(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    portal: &str,
    index: usize,
) -> String {
    arn_of(
        &ok(
            router,
            "CreateProcurementPortalPreference",
            &create_with_delivery_endpoint(
                portal,
                &format!("BUYER-UNPREDICTABLE-{index}"),
                &format!("SUPPLIER-UNPREDICTABLE-{index}"),
            ),
        )
        .await,
    )
}

/// Every plane of the preference `arn` names, as a reader reports its verification status.
async fn plane_statuses(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    arn: &str,
) -> Vec<Option<String>> {
    let reported = get_preference(router, arn).await;
    PLANE_STATUS_MEMBERS
        .iter()
        .map(|member| {
            reported
                .get(*member)
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// RULE: each issued code is drawn from the whole [`CODE_DIGITS`]-digit space, so a run's
/// codes spread over that space instead of walking a sequence, and no code of the run
/// repeats an earlier one.
///
/// The draws are taken across the portals the enum admits and across repeated sends against
/// one preference, so neither the portal nor the position in a preference's own sends is
/// what the draw follows.
#[tokio::test]
async fn issued_codes_are_drawn_from_the_whole_code_space() {
    let (router, engine) = emulator_with_engine();
    let mut issued: Vec<String> = Vec::new();

    for index in 0..PREFERENCES {
        let arn = preference(&router, PORTAL_NAMES[index % PORTAL_NAMES.len()], index).await;
        for _ in 0..DRAWS_PER_PREFERENCE {
            issued.push(issue_code(&router, &engine, &arn).await);
        }
    }

    // The form every code is quoted in, whatever it was drawn as.
    for (position, code) in issued.iter().enumerate() {
        assert_eq!(
            code.len(),
            CODE_DIGITS,
            "case draw {position}: an issued code is {CODE_DIGITS} decimal digits, got {code:?}"
        );
        assert!(
            code.chars().all(|digit| digit.is_ascii_digit()),
            "case draw {position}: an issued code is {CODE_DIGITS} decimal digits, got {code:?}"
        );
    }

    // Not the sequence a mint fed by its own issue counter hands out.
    let counter_sequence: Vec<String> = (1..=issued.len()).map(as_code).collect();
    assert_ne!(
        issued, counter_sequence,
        "case the whole batch: the codes a run issues must not be its issue sequence, got \
         {issued:?}"
    );

    // No code of the run repeats an earlier one, so one validation's code identifies it.
    let distinct: BTreeSet<&String> = issued.iter().collect();
    assert_eq!(
        distinct.len(),
        issued.len(),
        "case the whole batch: every issued code is one of its own, got {issued:?}"
    );

    // The batch spreads over the space rather than sitting at its bottom.
    let values: Vec<u32> = issued
        .iter()
        .map(|code| {
            code.parse().unwrap_or_else(|error| {
                panic!("case the whole batch: {code:?} is not a decimal code ({error})")
            })
        })
        .collect();
    let in_low_band = values.iter().filter(|value| **value < LOW_BAND).count();
    assert!(
        in_low_band <= TOLERATED_IN_LOW_BAND,
        "case the whole batch: at most {TOLERATED_IN_LOW_BAND} of {} draws fall below \
         {LOW_BAND}, got {in_low_band} of {issued:?}",
        issued.len()
    );
    let leading: BTreeSet<char> = issued.iter().filter_map(|code| code.chars().next()).collect();
    assert!(
        leading.len() >= LEADING_DIGITS_AT_LEAST,
        "case the whole batch: a draw over the whole space carries at least \
         {LEADING_DIGITS_AT_LEAST} distinct leading digits, got {leading:?} from {issued:?}"
    );
}

/// One row: a preference on a chosen portal, validated after a chosen number of sends, so
/// the guesses below are made at several positions of the service's own issue sequence.
struct Row {
    label: &'static str,
    portal: &'static str,
    sends: usize,
}

/// The rows: every portal the enum admits, at one send and at several.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one send",
            portal: PORTAL_NAMES[0],
            sends: 1,
        },
        Row {
            label: "two sends",
            portal: PORTAL_NAMES[1],
            sends: 2,
        },
        Row {
            label: "three sends",
            portal: PORTAL_NAMES[2],
            sends: 3,
        },
        Row {
            label: "one send, on the fourth portal",
            portal: PORTAL_NAMES[3],
            sends: 1,
        },
    ]
}

/// The codes a caller that never received one could NAME: the position of this send in the
/// service's own sequence of issues and in this preference's sends, the bounds and the first
/// value of the code space, and the codes the service issued for other preferences of the
/// same run.
fn predictable_candidates(
    issue_position: usize,
    send: usize,
    issued_elsewhere: &[String],
) -> Vec<(String, String)> {
    let mut candidates = vec![
        (
            "the position of this send in the service's issues".to_string(),
            as_code(issue_position),
        ),
        (
            "the position of this send among this preference's sends".to_string(),
            as_code(send),
        ),
        (
            "the first code a counter-fed mint issues".to_string(),
            as_code(1),
        ),
        (
            "the lowest code of the space".to_string(),
            as_code(0),
        ),
        (
            "the highest code of the space".to_string(),
            "9".repeat(CODE_DIGITS),
        ),
    ];
    candidates.extend(
        issued_elsewhere
            .iter()
            .enumerate()
            .map(|(position, code)| {
                (
                    format!("the code issue {} of this run left", position + 1),
                    code.clone(),
                )
            }),
    );
    candidates
}

/// RULE: only the code the send ISSUED completes the validation — a code a caller could
/// name without having received it is the modelled HTTP 400 `ValidationException` and moves
/// no plane out of the status it holds, whatever position in the service's issues the send
/// occupies.
#[tokio::test]
async fn a_code_a_caller_could_name_completes_no_validation() {
    // A fresh emulator, so the guesses below are made against a service that has issued
    // nothing yet: that is where a sequence-fed mint is at its most predictable.
    let (router, engine) = emulator_with_engine();
    // Where in the service's own sequence of issues the next send falls.
    let mut issue_position = 0_usize;
    // The codes earlier issues of this run left, which a caller that saw one of them could
    // submit against another preference.
    let mut issued_elsewhere: Vec<String> = Vec::new();
    // The guesses that turned out to BE the code the send drew, which is what a mint a
    // caller can follow produces on every send.
    let mut coincided: Vec<String> = Vec::new();

    for (index, row) in rows().iter().enumerate() {
        let arn = preference(&router, row.portal, index).await;
        let pending: Vec<Option<String>> = PLANE_STATUS_MEMBERS
            .iter()
            .map(|_| Some(PENDING_VERIFICATION.to_string()))
            .collect();
        assert_eq!(
            plane_statuses(&router, &arn).await,
            pending,
            "case {}: a created preference's planes are {PENDING_VERIFICATION}",
            row.label
        );

        let mut outstanding = String::new();
        for send in 1..=row.sends {
            issue_position += 1;
            let case = format!("{} / send {send}", row.label);
            let code = issue_code(&router, &engine, &arn).await;
            assert_eq!(
                code.len(),
                CODE_DIGITS,
                "case {case}: an issued code is {CODE_DIGITS} decimal digits, got {code:?}"
            );

            for (what, candidate) in predictable_candidates(issue_position, send, &issued_elsewhere)
            {
                // A guess that IS the code this send drew cannot be submitted as a wrong
                // one: the draw is uniform over the whole space, so it can land on a named
                // value. One such landing says nothing; a mint a caller can follow lands on
                // its own sequence's guess every time, which is what the count below bounds.
                if candidate == code {
                    coincided.push(format!("{case} / {what} ({candidate:?})"));
                    continue;
                }
                let (status, body) = call(
                    &router,
                    "VerifyProcurementPortalValidation",
                    &json!({"ProcurementPortalPreferenceArn": arn, "Code": candidate}),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case} / {what} ({candidate:?}): a code the service did not issue \
                     completes nothing, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case} / {what} ({candidate:?}): expected the modelled \
                     ValidationException, got {body}"
                );
                assert_eq!(
                    plane_statuses(&router, &arn).await,
                    pending,
                    "case {case} / {what} ({candidate:?}): a refused submission leaves every \
                     plane at {PENDING_VERIFICATION}"
                );
            }

            issued_elsewhere.push(code.clone());
            outstanding = code;
        }

        // The code the last send issued is what completes the validation, which the reader
        // then reports for every plane that was waiting for the portal connection.
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": outstanding}),
        )
        .await;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {}: the verify answers with the ARN it validated, got {completed}",
            row.label
        );
        let validated: Vec<Option<String>> = PLANE_STATUS_MEMBERS
            .iter()
            .map(|_| Some(VALIDATED.to_string()))
            .collect();
        assert_eq!(
            plane_statuses(&router, &arn).await,
            validated,
            "case {}: the code the send issued completes the validation, so every enabled \
             plane reports {VALIDATED}",
            row.label
        );
    }

    assert!(
        coincided.len() <= TOLERATED_COINCIDENCES,
        "case the whole table: at most {TOLERATED_COINCIDENCES} of the guesses may turn out \
         to be the code a send drew, got {} of them: {coincided:?}",
        coincided.len()
    );
}
