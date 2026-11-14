// SPDX-License-Identifier: CC0-1.0

//! `CreateRestoreTestingPlan` and the read over what it stores
//! (`GetRestoreTestingPlan`).
//!
//! The rules under test:
//!
//! * a restore testing plan is a STORED resource addressed by name: the body a
//!   create submits is what every later read of that name reports, with the two
//!   members the model gives a default for filled in where the create stated
//!   neither (`StartWindowHours` 24, `SelectionWindowDays` 30, and an empty
//!   `ExcludeVaults`);
//! * the name identifies one plan of the account, so however many creates of one
//!   name arrive together exactly ONE stores a plan and every other is refused as
//!   already existing — and the survivor is what the read reports;
//! * a repeated `CreatorRequestId` takes no second action: it answers with the
//!   plan that token created;
//! * a value outside what the model documents for a plan body — the name's
//!   alphabet and length, a schedule that is no cron expression, a window past
//!   its published bound — is refused `InvalidParameterValueException`, naming
//!   the member it rejected;
//! * a name the account holds no plan under is the one not-found refusal.
//!
//! Every case drives real HTTP through the router; the harness is the shared one
//! in `vault_and_backup_job.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{call, error_code, router, string_at, ACCOUNT, REGION};

/// The sentence this service states for "the restore testing plan asked about is
/// not one this account holds" — fixed text, naming no part of the request.
const NOT_FOUND: &str = "Cannot find restore testing plan";

/// The sentence this service states for a create naming a plan the account
/// already holds — fixed text, naming no part of the request, as this service
/// states the same condition for a vault, a backup plan, one of its selections
/// and a tiering configuration.
const ALREADY_EXISTS: &str = "Restore testing plan with the same name already exists";

/// The windows a stored plan carries when its create states neither:
/// "Defaults to 24 hours" (model doc for `StartWindowHours`) and "If not
/// included, the value defaults to 30" (model doc for `SelectionWindowDays`).
const DEFAULT_START_WINDOW_HOURS: i64 = 24;
const DEFAULT_SELECTION_WINDOW_DAYS: i64 = 30;

/// The published bounds of a STATED start window — "a maximum value of 168 hours
/// (one week)" (model doc for `StartWindowHours`) — and of a stated selection
/// window — "integers from 1 to 365" (model doc for `SelectionWindowDays`).
const MAX_START_WINDOW_HOURS: i64 = 168;
const MIN_SELECTION_WINDOW_DAYS: i64 = 1;
const MAX_SELECTION_WINDOW_DAYS: i64 = 365;

/// The most characters a `RestoreTestingPlanName` may carry: "Maximum length is
/// 50" (model doc for the member).
const MAX_NAME_LENGTH: usize = 50;

/// `PUT /restore-testing/plans`, as the model binds the create.
async fn create_plan(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/restore-testing/plans", Some(&body.to_string())).await
}

/// `GET /restore-testing/plans/{RestoreTestingPlanName}`.
async fn get_plan(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/restore-testing/plans/{name}"),
        None,
    )
    .await
}

/// A create body carrying `plan` as its `RestoreTestingPlan`.
fn create_body(plan: Value) -> Value {
    json!({ "RestoreTestingPlan": plan })
}

/// The smallest plan body the model admits: a name, a schedule and a selection
/// naming the vaults recovery points may come from.
fn minimal_plan(name: &str) -> Value {
    json!({
        "RestoreTestingPlanName": name,
        "ScheduleExpression": "cron(0 5 ? * * *)",
        "RecoveryPointSelection": {
            "Algorithm": "LATEST_WITHIN_WINDOW",
            "RecoveryPointTypes": ["SNAPSHOT"],
            "IncludeVaults": ["*"],
        },
    })
}

/// The plan body a read reports (`RestoreTestingPlan`), or `Value::Null`.
fn read_plan(body: &Value) -> Value {
    body.get("RestoreTestingPlan").cloned().unwrap_or(Value::Null)
}

/// An integer member of a body, or `i64::MIN` when it is absent — so an absent
/// member can never read as a value the service might legitimately report.
fn int_at(body: &Value, member: &str) -> i64 {
    body.get(member).and_then(Value::as_i64).unwrap_or(i64::MIN)
}

/// A name of `length` characters from the alphabet the model documents for a
/// `RestoreTestingPlanName` ("only alphanumeric characters and underscores"), so
/// the length bound is exercised across that whole alphabet.
fn name_of_length(length: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";
    (0..length)
        .map(|i| char::from(ALPHABET[(i * 7 + length) % ALPHABET.len()]))
        .collect()
}

/// A created plan round-trips: the read reports the body that was stored, with
/// the documented defaults filled in where the create stated nothing.
///
/// The rows differ along what the rule generalises over: the spelling of the
/// schedule (wrapped in `cron(…)` and bare), whether a timezone is stated, the
/// selection algorithm and recovery-point types, the vaults included and
/// excluded, the name's length (1 and the published maximum of 50) and both
/// windows — absent, at the floor of their range and at the published ceiling.
#[tokio::test]
async fn a_created_restore_testing_plan_round_trips_through_the_read() {
    let vault_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:tested");
    let other_vault_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:excluded");
    let cases: Vec<(&str, Value)> = vec![
        (
            "no window and no timezone stated",
            minimal_plan("plan_defaults"),
        ),
        (
            "both windows at the floor of their range, bare cron",
            json!({
                "RestoreTestingPlanName": name_of_length(1),
                "ScheduleExpression": "0 12 * * ? *",
                "StartWindowHours": 1,
                "RecoveryPointSelection": {
                    "Algorithm": "RANDOM_WITHIN_WINDOW",
                    "RecoveryPointTypes": ["CONTINUOUS"],
                    "IncludeVaults": [vault_arn.clone()],
                    "SelectionWindowDays": MIN_SELECTION_WINDOW_DAYS,
                },
            }),
        ),
        (
            "both windows at their published ceiling, a stated timezone",
            json!({
                "RestoreTestingPlanName": name_of_length(MAX_NAME_LENGTH),
                "ScheduleExpression": "cron(15 * ? * * *)",
                "ScheduleExpressionTimezone": "Europe/Dublin",
                "StartWindowHours": MAX_START_WINDOW_HOURS,
                "RecoveryPointSelection": {
                    "Algorithm": "LATEST_WITHIN_WINDOW",
                    "RecoveryPointTypes": ["SNAPSHOT", "CONTINUOUS"],
                    "IncludeVaults": ["*"],
                    "ExcludeVaults": [other_vault_arn.clone()],
                    "SelectionWindowDays": MAX_SELECTION_WINDOW_DAYS,
                },
            }),
        ),
        (
            "two included vaults, an interior window",
            json!({
                "RestoreTestingPlanName": "plan_two_vaults",
                "ScheduleExpression": "cron(0 1 ? * SUN *)",
                "StartWindowHours": 72,
                "RecoveryPointSelection": {
                    "Algorithm": "LATEST_WITHIN_WINDOW",
                    "RecoveryPointTypes": ["SNAPSHOT"],
                    "IncludeVaults": [vault_arn.clone(), other_vault_arn.clone()],
                    "ExcludeVaults": [],
                    "SelectionWindowDays": 7,
                },
            }),
        ),
    ];

    for (case, plan) in cases {
        let router = router();
        let name = string_at(&plan, "RestoreTestingPlanName");
        let (status, created) = create_plan(&router, &create_body(plan.clone())).await;
        assert_eq!(
            (status, error_code(&created).as_str()),
            (201, "<none>"),
            "case {case:?}: expected CreateRestoreTestingPlan 201, got {status} {created:?}"
        );
        let expected_arn =
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:restore-testing-plan:{name}");
        assert_eq!(
            (
                string_at(&created, "RestoreTestingPlanName"),
                string_at(&created, "RestoreTestingPlanArn")
            ),
            (name.clone(), expected_arn.clone()),
            "case {case:?}: the create must answer with the plan it stored, got {created:?}"
        );
        assert!(
            created.get("CreationTime").is_some(),
            "case {case:?}: the create reports when it stored the plan, got {created:?}"
        );

        let (status, read) = get_plan(&router, &name).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {case:?}: expected GetRestoreTestingPlan 200, got {status} {read:?}"
        );
        let stored = read_plan(&read);
        assert_eq!(
            (
                string_at(&stored, "RestoreTestingPlanName"),
                string_at(&stored, "RestoreTestingPlanArn"),
                string_at(&stored, "ScheduleExpression"),
                string_at(&stored, "CreationTime"),
            ),
            (
                name.clone(),
                expected_arn.clone(),
                string_at(&plan, "ScheduleExpression"),
                string_at(&created, "CreationTime"),
            ),
            "case {case:?}: the read must report what the create stored, got {read:?}"
        );
        assert_eq!(
            stored.get("ScheduleExpressionTimezone").cloned(),
            plan.get("ScheduleExpressionTimezone").cloned(),
            "case {case:?}: the stated timezone is stored as submitted, got {stored:?}"
        );
        assert_eq!(
            int_at(&stored, "StartWindowHours"),
            plan.get("StartWindowHours")
                .and_then(Value::as_i64)
                .unwrap_or(DEFAULT_START_WINDOW_HOURS),
            "case {case:?}: a stated start window is kept and an absent one defaults, got \
             {stored:?}"
        );
        let submitted = plan
            .get("RecoveryPointSelection")
            .cloned()
            .unwrap_or(Value::Null);
        let selection = stored
            .get("RecoveryPointSelection")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            (
                string_at(&selection, "Algorithm"),
                selection.get("RecoveryPointTypes").cloned(),
                selection.get("IncludeVaults").cloned(),
            ),
            (
                string_at(&submitted, "Algorithm"),
                submitted.get("RecoveryPointTypes").cloned(),
                submitted.get("IncludeVaults").cloned(),
            ),
            "case {case:?}: the selection is stored as submitted, got {selection:?}"
        );
        assert_eq!(
            selection.get("ExcludeVaults").cloned(),
            Some(
                submitted
                    .get("ExcludeVaults")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
            ),
            "case {case:?}: an unlisted ExcludeVaults defaults to the empty list, got \
             {selection:?}"
        );
        assert_eq!(
            int_at(&selection, "SelectionWindowDays"),
            submitted
                .get("SelectionWindowDays")
                .and_then(Value::as_i64)
                .unwrap_or(DEFAULT_SELECTION_WINDOW_DAYS),
            "case {case:?}: a stated selection window is kept and an absent one defaults, got \
             {selection:?}"
        );
        assert!(
            stored.get("LastUpdateTime").is_none() && stored.get("LastExecutionTime").is_none(),
            "case {case:?}: a plan nothing has updated or run reports neither instant, got \
             {stored:?}"
        );
    }
}

/// However many creates of ONE name arrive together, exactly one stores a plan:
/// every other is refused as already existing, and the read reports the single
/// survivor.
///
/// The rows differ in how many creates race for the name, so the rule is
/// asserted over the class rather than over one burst size.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_of_one_name_have_exactly_one_winner() {
    for racers in [2_usize, 6, 11] {
        let router = Arc::new(router());
        let body = create_body(minimal_plan("plan_contended"));
        // Every create is in flight at once, on four worker threads, so each one
        // reaches the store without knowing about the others.
        let mut pending = Vec::new();
        for _ in 0..racers {
            let router = Arc::clone(&router);
            let body = body.clone();
            pending.push(tokio::spawn(
                async move { create_plan(&router, &body).await },
            ));
        }
        let mut answers: Vec<(u16, Value)> = Vec::with_capacity(racers);
        for task in pending {
            answers.push(task.await.expect("a racing create to finish"));
        }

        let winners: Vec<&(u16, Value)> =
            answers.iter().filter(|(status, _)| *status == 201).collect();
        assert_eq!(
            winners.len(),
            1,
            "case {racers} racers: expected exactly one winning create, got {answers:?}"
        );
        for (status, answer) in answers.iter().filter(|(status, _)| *status != 201) {
            assert_eq!(
                (
                    *status,
                    error_code(answer).as_str(),
                    string_at(answer, "Message").as_str()
                ),
                (400, "AlreadyExistsException", ALREADY_EXISTS),
                "case {racers} racers: expected every loser refused as already existing, got \
                 {status} {answer:?}"
            );
        }

        let (status, read) = get_plan(&router, "plan_contended").await;
        assert_eq!(
            status, 200,
            "case {racers} racers: the survivor must be readable, got {status} {read:?}"
        );
        assert_eq!(
            string_at(&read_plan(&read), "RestoreTestingPlanArn"),
            string_at(&winners[0].1, "RestoreTestingPlanArn"),
            "case {racers} racers: the read must report the winning create's plan"
        );
    }
}

/// A repeated `CreatorRequestId` takes no second action: it answers with the plan
/// that token created, even when the body it carries differs, and the store
/// still holds that one plan.
#[tokio::test]
async fn a_repeated_creator_request_id_replays_the_first_create() {
    let router = router();
    let mut body = create_body(minimal_plan("plan_replayed"));
    body["CreatorRequestId"] = json!("one-plan-only");

    let (status, first) = create_plan(&router, &body).await;
    assert_eq!(status, 201, "the first create should succeed, got {first:?}");

    // The SAME token on a body naming another plan: the token identifies the
    // call, so what it replays is the plan it already created.
    let mut retried = create_body(minimal_plan("plan_replayed_other"));
    retried["CreatorRequestId"] = json!("one-plan-only");
    let (status, replayed) = create_plan(&router, &retried).await;
    assert_eq!(
        (status, error_code(&replayed).as_str()),
        (201, "<none>"),
        "a replayed CreatorRequestId must answer as the first call did, got {status} {replayed:?}"
    );
    assert_eq!(
        (
            string_at(&first, "RestoreTestingPlanName"),
            string_at(&first, "RestoreTestingPlanArn"),
            string_at(&first, "CreationTime")
        ),
        (
            string_at(&replayed, "RestoreTestingPlanName"),
            string_at(&replayed, "RestoreTestingPlanArn"),
            string_at(&replayed, "CreationTime")
        ),
        "a replay must report the identifiers the first call reported"
    );
    let (status, read) = get_plan(&router, "plan_replayed_other").await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (400, "ResourceNotFoundException"),
        "a replay must store no second plan, got {status} {read:?}"
    );
    assert_eq!(
        string_at(&read, "Message"),
        NOT_FOUND,
        "a name nothing created answers the one not-found sentence, got {read:?}"
    );

    // A create of ANOTHER name with no token IS a second plan, which is what
    // makes the replay above about the token.
    let (status, second) = create_plan(&router, &create_body(minimal_plan("plan_second"))).await;
    assert_eq!(status, 201, "a second name should be stored, got {second:?}");
    let (status, read) = get_plan(&router, "plan_second").await;
    assert_eq!(status, 200, "the second plan must be readable, got {read:?}");
}

/// A plan body outside what the model documents is refused
/// `InvalidParameterValueException`, naming the member it rejected — and nothing
/// is stored for it.
///
/// The rows span the class: each documented rule, each at one step past its
/// bound, alongside the member the refusal has to name.
#[tokio::test]
async fn a_plan_body_outside_the_documented_rules_is_refused() {
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a name one character past the length bound",
            json!(name_of_length(MAX_NAME_LENGTH + 1)),
            "RestoreTestingPlanName",
        ),
        (
            "a name carrying a character outside the alphabet",
            json!("plan-hyphenated"),
            "RestoreTestingPlanName",
        ),
        ("an empty name", json!(""), "RestoreTestingPlanName"),
    ];
    for (case, name, member) in cases {
        let router = router();
        let mut plan = minimal_plan("placeholder");
        plan["RestoreTestingPlanName"] = name;
        assert_refused(&router, case, plan, member).await;
    }

    let schedules: Vec<(&str, Value)> = vec![
        ("a schedule that is not a cron expression", json!("daily")),
        (
            "a cron expression short of its six fields",
            json!("cron(0 5 ? *)"),
        ),
        (
            "a cron expression whose minute field is out of range",
            json!("cron(99 5 ? * * *)"),
        ),
    ];
    for (case, schedule) in schedules {
        let router = router();
        let mut plan = minimal_plan("plan_schedule");
        plan["ScheduleExpression"] = schedule;
        assert_refused(&router, case, plan, "ScheduleExpression").await;
    }

    let windows: Vec<(&str, i64, &str)> = vec![
        (
            "a start window one hour past its ceiling",
            MAX_START_WINDOW_HOURS + 1,
            "StartWindowHours",
        ),
        ("a negative start window", -1, "StartWindowHours"),
    ];
    for (case, hours, member) in windows {
        let router = router();
        let mut plan = minimal_plan("plan_start_window");
        plan["StartWindowHours"] = json!(hours);
        assert_refused(&router, case, plan, member).await;
    }

    let selection_windows: Vec<(&str, i64)> = vec![
        (
            "a selection window one day past its ceiling",
            MAX_SELECTION_WINDOW_DAYS + 1,
        ),
        ("a negative selection window", -1),
    ];
    for (case, days) in selection_windows {
        let router = router();
        let mut plan = minimal_plan("plan_selection_window");
        plan["RecoveryPointSelection"]["SelectionWindowDays"] = json!(days);
        assert_refused(&router, case, plan, "SelectionWindowDays").await;
    }
}

/// The one refusal every row of the table above expects: 400
/// `InvalidParameterValueException` naming `member`, and no plan stored.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    plan: Value,
    member: &str,
) {
    let name = string_at(&plan, "RestoreTestingPlanName");
    let (status, refused) = create_plan(router, &create_body(plan)).await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            string_at(&refused, "Message").as_str(),
            string_at(&refused, "Context").as_str()
        ),
        (
            400,
            "InvalidParameterValueException",
            format!("Parameter {member} is invalid.").as_str(),
            member
        ),
        "case {case:?}: expected the refusal to name {member}, got {status} {refused:?}"
    );
    // An empty name addresses no path the read is bound to (`GET
    // /restore-testing/plans/{RestoreTestingPlanName}` with an empty label is
    // the LISTING's path), so the "nothing was stored" half is asserted for
    // every other row.
    if name.is_empty() {
        return;
    }
    let (status, read) = get_plan(router, &name).await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: a refused create stores nothing, got {status} {read:?}"
    );
}

/// A name the account holds no plan under is the one not-found refusal, whatever
/// else the store holds.
#[tokio::test]
async fn a_read_of_an_unknown_name_is_refused() {
    let router = router();
    let (status, created) = create_plan(&router, &create_body(minimal_plan("plan_present"))).await;
    assert_eq!(status, 201, "setup: the create should succeed, got {created:?}");

    for name in ["plan_absent", "plan_Present", "PLAN_PRESENT"] {
        let (status, refused) = get_plan(&router, name).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "ResourceNotFoundException", NOT_FOUND),
            "case {name:?}: a name holding no plan is not found, got {status} {refused:?}"
        );
    }
}
