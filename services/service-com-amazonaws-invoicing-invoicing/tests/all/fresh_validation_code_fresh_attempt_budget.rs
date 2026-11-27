// SPDX-License-Identifier: CC0-1.0

//! The ATTEMPT BUDGET a refused validation code is reported against, driven end to end
//! over this service's wire protocol through the same `MultiServiceRouter` the production
//! router uses, with the shared harness of `refused_procurement_portal_writes`.
//!
//! The rule under test: the attempt count a refused code submission reports counts the
//! submissions refused against the code CURRENTLY OUTSTANDING, so the call that issues a
//! code starts that count again — `SendProcurementPortalValidation` "initiates the
//! validation process by issuing a validation code" (`smithy_query(query="doc",
//! shape="SendProcurementPortalValidation")`) and is the remedy the spent-budget refusal
//! names, so a client that follows that instruction reads attempt 1 of the budget for the
//! fresh code instead of the previous code's spent one, and the number it drives its
//! "retry or ask again" decision off stays inside the budget it is stated against.
//!
//! Its counterpart, which pins the count to the ISSUING rather than to any call that
//! writes: a configuration update issues no code, so it starts no second budget either —
//! the attempts already made against the outstanding code still count.
//!
//! Each test is a table: the rows differ along how many submissions were refused before a
//! fresh code is asked for (none, one, exactly the budget, past it), whether the earlier
//! round completed the validation first, how many fresh codes follow one another, the
//! portals a preference can route to, and the form the submitted wrong code takes.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, call, create_with_delivery_endpoint, emulator_with_engine,
    error_code, error_text, get_preference, invalid_code_sentence, issue_code, ok,
    outstanding_code, portal_at, shifted, version, without_write_markers, PORTAL_NAMES,
    VERIFICATION_ATTEMPT_BUDGET,
};

/// How many submissions one round refuses: one past the budget, so each round also shows
/// the budget is REPORTING-only — the service keeps accepting submissions — and that the
/// reported attempt is the true count against the outstanding code rather than a clamp.
const SUBMISSIONS_PER_ROUND: u32 = VERIFICATION_ATTEMPT_BUDGET + 1;

/// One refused submission, asserting the modelled answer and WHICH attempt it names.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    code: &str,
    expected_attempt: u32,
) {
    let (status, body) = call(
        router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": arn, "Code": code}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        error_text(&body),
        invalid_code_sentence(expected_attempt),
        "case {case}: the refusal must name attempt {expected_attempt} of \
         {VERIFICATION_ATTEMPT_BUDGET}, got {body}"
    );
}

/// Each row's wrong code, built from the code that IS outstanding so it is wrong on its
/// own account rather than by shape.
fn a_shifted_copy(issued: &str) -> String {
    shifted(issued)
}

fn a_non_digit_code(_issued: &str) -> String {
    "not-a-code".to_string()
}

fn a_longer_code(_issued: &str) -> String {
    "12345678".to_string()
}

fn a_one_character_code(_issued: &str) -> String {
    "7".to_string()
}

/// One row: a preference whose first issued code is refused a chosen number of times,
/// optionally completed, and then re-issued a chosen number of times.
struct Row {
    label: &'static str,
    portal: &'static str,
    /// Submissions refused against the FIRST issued code before a fresh one is asked for:
    /// below the budget, exactly at it, and past it.
    failures_before_resend: u32,
    /// Whether the outstanding code is submitted successfully before the fresh one is
    /// asked for, so the rule holds whether the previous round ended refused or completed.
    complete_before_resend: bool,
    /// How many fresh codes follow one another, so the count restarts with EACH issue
    /// rather than once.
    resends: u32,
    wrong_code: fn(&str) -> String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no failure before the fresh code",
            portal: portal_at(0),
            failures_before_resend: 0,
            complete_before_resend: false,
            resends: 1,
            wrong_code: a_shifted_copy,
        },
        Row {
            label: "one failure before the fresh code",
            portal: portal_at(1),
            failures_before_resend: 1,
            complete_before_resend: false,
            resends: 1,
            wrong_code: a_non_digit_code,
        },
        Row {
            label: "the budget exactly spent before the fresh code",
            portal: portal_at(2),
            failures_before_resend: VERIFICATION_ATTEMPT_BUDGET,
            complete_before_resend: false,
            resends: 1,
            wrong_code: a_longer_code,
        },
        Row {
            label: "past the budget before the fresh code, then two fresh codes",
            portal: portal_at(3),
            failures_before_resend: VERIFICATION_ATTEMPT_BUDGET + 2,
            complete_before_resend: false,
            resends: 2,
            wrong_code: a_shifted_copy,
        },
        Row {
            label: "the budget spent, then the validation completed, then a fresh code",
            portal: portal_at(0),
            failures_before_resend: VERIFICATION_ATTEMPT_BUDGET,
            complete_before_resend: true,
            resends: 1,
            wrong_code: a_one_character_code,
        },
        Row {
            label: "three fresh codes after one failure",
            portal: portal_at(2),
            failures_before_resend: 1,
            complete_before_resend: false,
            resends: 3,
            wrong_code: a_longer_code,
        },
    ]
}

/// RULE: the attempt count a refused code submission reports is the count of submissions
/// refused against the code currently outstanding, so the call that issues a code starts
/// that count again — the first submission refused against a freshly issued code is
/// attempt 1 of the budget, and the remedy the spent-budget refusal names really restores
/// what it points at.
#[tokio::test]
async fn a_freshly_issued_code_is_refused_from_the_first_attempt() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in rows().iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    row.portal,
                    &format!("BUYER-BUDGET-{index}"),
                    &format!("SUPPLIER-BUDGET-{index}"),
                ),
            )
            .await,
        );

        // The first round: a code, and the submissions this row refuses against it.
        let first = issue_code(&router, &engine, &arn).await;
        let wrong = (row.wrong_code)(&first);
        assert_ne!(
            wrong, first,
            "case {}: the row's code has to be the wrong one, got {wrong:?}",
            row.label
        );
        for attempt in 1..=row.failures_before_resend {
            refused(
                &router,
                &format!("{} / first code attempt {attempt}", row.label),
                &arn,
                &wrong,
                attempt,
            )
            .await;
        }
        if row.complete_before_resend {
            ok(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": first}),
            )
            .await;
        }

        // Each fresh code restarts the count: the submissions refused against it are its
        // own attempts 1, 2, … and the number stays inside the budget it is stated
        // against until that code's own submissions pass it.
        let mut previous = first;
        for round in 1..=row.resends {
            let fresh = issue_code(&router, &engine, &arn).await;
            assert_ne!(
                fresh, previous,
                "case {} / round {round}: a send issues a code of its own, got the previous \
                 {previous:?}",
                row.label
            );
            let wrong = (row.wrong_code)(&fresh);
            assert_ne!(
                wrong, fresh,
                "case {} / round {round}: the row's code has to be the wrong one, got \
                 {wrong:?}",
                row.label
            );
            for attempt in 1..=SUBMISSIONS_PER_ROUND {
                refused(
                    &router,
                    &format!("{} / round {round} attempt {attempt}", row.label),
                    &arn,
                    &wrong,
                    attempt,
                )
                .await;
            }
            previous = fresh;
        }

        // The budget is reported, not enforced: the code the last send issued is still
        // outstanding after those refusals and still completes the validation.
        assert_eq!(
            outstanding_code(&engine, &arn).as_deref(),
            Some(previous.as_str()),
            "case {}: the refusals neither issue nor retire a code",
            row.label
        );
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": previous}),
        )
        .await;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {}: the fresh code still completes the validation, got {completed}",
            row.label
        );
    }
}

/// RULE (the same one, from its other side): the count restarts because a code was
/// ISSUED, so a call that WRITES the preference without issuing one — a configuration
/// update — leaves the attempts already made against the outstanding code counted, and the
/// next refusal continues from them.
///
/// The rows differ along how many submissions were refused before that write arrives —
/// below the budget, at it and past it.
#[tokio::test]
async fn a_write_that_issues_no_code_starts_no_fresh_budget() {
    let (router, engine) = emulator_with_engine();

    let cases: &[(&str, u32)] = &[
        ("one refusal before the write", 1),
        ("two refusals before the write", 2),
        (
            "one refusal past the budget before the write",
            VERIFICATION_ATTEMPT_BUDGET + 1,
        ),
        ("the budget spent before the write", VERIFICATION_ATTEMPT_BUDGET),
    ];

    for (index, (label, failures_before_write)) in cases.iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    PORTAL_NAMES[index % PORTAL_NAMES.len()],
                    &format!("BUYER-OTHER-WRITE-BUDGET-{index}"),
                    &format!("SUPPLIER-OTHER-WRITE-BUDGET-{index}"),
                ),
            )
            .await,
        );
        let issued = issue_code(&router, &engine, &arn).await;
        let wrong = shifted(&issued);

        for attempt in 1..=*failures_before_write {
            refused(
                &router,
                &format!("{label}, attempt {attempt}"),
                &arn,
                &wrong,
                attempt,
            )
            .await;
        }

        // A configuration write issues no code, so the count it would have restarted
        // stands where those refusals left it — and the code stays the outstanding one.
        ok(
            &router,
            "PutProcurementPortalPreference",
            &accepted_update_round(&arn, index as i64),
        )
        .await;
        assert_eq!(
            outstanding_code(&engine, &arn).as_deref(),
            Some(issued.as_str()),
            "case {label}: a write that issues no code leaves the outstanding one alone"
        );
        refused(
            &router,
            &format!("{label}, after the write"),
            &arn,
            &wrong,
            failures_before_write + 1,
        )
        .await;

        // A SEND issues a code, and THAT restarts the count.
        let fresh = issue_code(&router, &engine, &arn).await;
        assert_ne!(
            fresh, issued,
            "case {label}: a send issues a fresh code"
        );
        refused(
            &router,
            &format!("{label} after a fresh code"),
            &arn,
            &shifted(&fresh),
            1,
        )
        .await;
    }
}

/// RULE (where it has to hold independently): the count belongs to the preference it was
/// recorded against as well as to the code, so issuing a fresh code for one preference
/// restarts that preference's count and leaves every other preference's where it was.
#[tokio::test]
async fn a_fresh_code_restarts_only_its_own_preferences_count() {
    let (router, engine) = emulator_with_engine();

    let mut arns: Vec<String> = Vec::new();
    let mut created: Vec<Value> = Vec::new();
    for (index, portal) in PORTAL_NAMES.iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    portal,
                    &format!("BUYER-ISOLATED-{index}"),
                    &format!("SUPPLIER-ISOLATED-{index}"),
                ),
            )
            .await,
        );
        created.push(get_preference(&router, &arn).await);
        arns.push(arn);
    }

    // Every preference spends its budget against the code issued for it.
    let mut wrong_codes: Vec<String> = Vec::new();
    for (index, arn) in arns.iter().enumerate() {
        let issued = issue_code(&router, &engine, arn).await;
        let wrong = shifted(&issued);
        for attempt in 1..=VERIFICATION_ATTEMPT_BUDGET {
            refused(
                &router,
                &format!("preference {index} / attempt {attempt}"),
                arn,
                &wrong,
                attempt,
            )
            .await;
        }
        wrong_codes.push(wrong);
    }

    // One of them asks for a fresh code: its count restarts, and only its own.
    let refreshed = 0;
    let fresh = issue_code(&router, &engine, &arns[refreshed]).await;
    let fresh_wrong = shifted(&fresh);
    refused(
        &router,
        "the refreshed preference",
        &arns[refreshed],
        &fresh_wrong,
        1,
    )
    .await;
    for (index, arn) in arns.iter().enumerate().skip(1) {
        refused(
            &router,
            &format!("preference {index} / after another preference's fresh code"),
            arn,
            &wrong_codes[index],
            VERIFICATION_ATTEMPT_BUDGET + 1,
        )
        .await;
    }

    // And no preference's reported CONFIGURATION moved under any of it, while the version
    // each one reports counts the writes that preference took.
    for (index, arn) in arns.iter().enumerate() {
        let reported = get_preference(&router, arn).await;
        assert_eq!(
            without_write_markers(&reported),
            without_write_markers(&created[index]),
            "case preference {index}: a refused submission and a fresh code write no \
             configuration, got {reported}"
        );
        let writes = 1 // the create
            + 1 // the code issued for this preference
            + i64::from(VERIFICATION_ATTEMPT_BUDGET) // the submissions refused against it
            + if index == refreshed {
                2 // its fresh code, and the submission refused against that
            } else {
                1 // the submission refused after another preference's fresh code
            };
        assert_eq!(
            version(&reported),
            writes,
            "case preference {index}: expected Version {writes}, one per write it took, got \
             {reported}"
        );
    }
}
