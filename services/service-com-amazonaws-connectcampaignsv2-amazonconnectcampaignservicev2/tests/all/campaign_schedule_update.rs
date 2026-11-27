// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignSchedule`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call sets the campaign `{id}` names to the `schedule` the request carries and
//!   REPLACES whatever it held — the schedule is one modelled structure, so a schedule
//!   that leaves `refreshFrequency` out is a campaign that carries none — which is what
//!   every later read (`DescribeCampaign`, `ListCampaigns`) projects, while every other
//!   member of the campaign stays as it was;
//! * the operation is idempotent (`@idempotent` in the model, "This API is idempotent"
//!   in its documentation): the same schedule sent twice leaves the campaign exactly as
//!   the first call left it;
//! * the window the stored schedule now carries is the one the campaign's state is
//!   derived from, so a running campaign rescheduled past its `endTime` is `Completed`
//!   and one rescheduled into an open window keeps running;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and stores
//!   nothing;
//! * a campaign in a terminal lifecycle state is past being reconfigured: HTTP 409
//!   `InvalidCampaignStateException` naming that state, storing nothing, while every
//!   non-terminal state takes the update;
//! * a `refreshFrequency` outside the modelled `Iso8601Duration` pattern is HTTP 400
//!   `ValidationException` and stores nothing.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, CAMPAIGN_NOT_FOUND_MESSAGE,
};

use super::common::{
    call, campaign_state, campaign_without, create_campaign, describe_campaign, drive_lifecycle,
    error_code, instant, router, Harness,
};

/// A `schedule` request member: a window given as offsets in seconds from now, with
/// `refreshFrequency` attached when given.
///
/// `startTime` and `endTime` are both `@required`; `refreshFrequency` is the one
/// optional member (source: the `Schedule` shape).
fn schedule(start: f64, end: f64, refresh_frequency: Option<&str>) -> Value {
    let mut value = json!({"startTime": instant(start), "endTime": instant(end)});
    if let Some(frequency) = refresh_frequency {
        value["refreshFrequency"] = json!(frequency);
    }
    value
}

/// `UpdateCampaignSchedule` for `id`, carrying `schedule`.
async fn update_schedule(
    router: &Harness,
    region: &str,
    id: &str,
    schedule: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/schedule"),
        &json!({"schedule": schedule}),
    )
    .await
}

/// The instant a timestamp member carries, in seconds since the Unix epoch. A
/// `TimeStamp` is `@timestampFormat("date-time")`, so it is an RFC 3339 string on the
/// wire; a number is accepted too, so the comparison is about the instant and not
/// about the encoding.
fn instant_secs(value: &Value, case: &str, member: &str) -> f64 {
    match value {
        Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} {text:?} is not an instant: {error}")
            })
            .as_secs_f64(),
        Value::Number(number) => number
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {member} {number} is not a number")),
        other => panic!("case {case:?}: expected {member} to carry an instant, got {other}"),
    }
}

/// Assert the `schedule` a projection carries is `expected`, member for member: the
/// same two instants, and the same `refreshFrequency` — including none at all.
fn assert_schedule_is(projection: &Value, expected: &Value, case: &str, read: &str) {
    let actual = &projection["schedule"];
    for member in ["startTime", "endTime"] {
        let want = instant_secs(&expected[member], case, member);
        let got = instant_secs(&actual[member], case, member);
        assert!(
            (got - want).abs() < 0.002,
            "case {case:?}: expected {read} to project {member} {want}, got {got} ({actual})"
        );
    }
    assert_eq!(
        actual.get("refreshFrequency"),
        expected.get("refreshFrequency"),
        "case {case:?}: expected {read} to project refreshFrequency {:?}, got {:?}",
        expected.get("refreshFrequency"),
        actual.get("refreshFrequency")
    );
}

/// The `CampaignSummary` `ListCampaigns` projects for `id`.
async fn summary_of(router: &Harness, region: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        region,
        "POST",
        "/v2/campaigns-summary",
        &json!({"maxResults": 50}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {body}"
    );
    body["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: {id} is not in the listing: {body}"))
        .clone()
}

/// One row of the replacement table in [`an_update_replaces_the_whole_schedule`].
struct Row<'a> {
    /// What an assertion calls this row.
    name: &'a str,
    /// The schedule the campaign is created with, or `None` for a campaign created
    /// with no schedule at all.
    created: Option<Value>,
    /// The schedule the update carries.
    updated: Value,
}

/// The update stores the schedule it was given, whole: every later read projects
/// exactly that schedule, and every other member of the campaign is untouched.
///
/// The rows differ along what the rule generalises over: whether the campaign had a
/// schedule at all, where the new window sits relative to now (open, entirely in the
/// future, hours long, a month long), and which form `refreshFrequency` takes —
/// including a schedule that drops the one the campaign held. The durations span the
/// modelled `Iso8601Duration` pattern (source: `Schedule$refreshFrequency`): days,
/// hours, minutes, seconds, a fractional second, a compound duration, and `P` alone.
#[tokio::test]
async fn an_update_replaces_the_whole_schedule() {
    let region = "us-west-2";
    let router = router();
    let instance = "schedule-update-instance";

    let cases: Vec<Row> = vec![
        Row {
            name: "no schedule at all, then an open window",
            created: None,
            updated: schedule(-60.0, 3_600.0, None),
        },
        Row {
            name: "an open window, then a window entirely in the future",
            created: Some(schedule(-60.0, 3_600.0, None)),
            updated: schedule(86_400.0, 172_800.0, Some("P1D")),
        },
        Row {
            name: "a refresh frequency, then none",
            created: Some(schedule(-60.0, 3_600.0, Some("PT1H"))),
            updated: schedule(-30.0, 7_200.0, None),
        },
        Row {
            name: "none, then a minute-granularity refresh frequency",
            created: Some(schedule(60.0, 120.0, None)),
            updated: schedule(-10.0, 1_800.0, Some("PT30M")),
        },
        Row {
            name: "a second-granularity refresh frequency",
            created: Some(schedule(-60.0, 600.0, Some("P1D"))),
            updated: schedule(-5.0, 900.0, Some("PT45S")),
        },
        Row {
            name: "a fractional-second refresh frequency",
            created: Some(schedule(-60.0, 600.0, Some("PT30M"))),
            updated: schedule(-1.0, 60.0, Some("PT1.500S")),
        },
        Row {
            name: "a compound refresh frequency over a month-long window",
            created: None,
            updated: schedule(-3_600.0, 2_592_000.0, Some("P1DT2H3M4S")),
        },
        Row {
            name: "the empty duration the pattern admits",
            created: Some(schedule(-60.0, 3_600.0, Some("P1DT2H3M4S"))),
            updated: schedule(-60.0, 3_600.0, Some("P")),
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let Row {
            name: case,
            created,
            updated,
        } = row;
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("campaign-{index}"),
            created.clone(),
        )
        .await;
        let before = describe_campaign(&router, region, &id, case).await;
        match created {
            Some(sent) => assert_schedule_is(&before, sent, case, "the created campaign"),
            None => assert!(
                before.get("schedule").is_none(),
                "case {case:?}: expected a campaign created with no schedule to carry none, \
                 got {:?}",
                before.get("schedule")
            ),
        }

        let (status, body) = update_schedule(&router, region, &id, updated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignSchedule expected 200, got {status} {body}"
        );

        // The stored schedule is the one the request carried, and nothing of the old
        // one survives: a schedule carrying no `refreshFrequency` leaves none behind.
        let after = describe_campaign(&router, region, &id, case).await;
        assert_schedule_is(&after, updated, case, "DescribeCampaign");
        let summary = summary_of(&router, region, &id, case).await;
        assert_schedule_is(&summary, updated, case, "the ListCampaigns summary");

        // Idempotent: the same schedule sent again is a 200 and the same stored
        // schedule.
        let (status, body) = update_schedule(&router, region, &id, updated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated update expected 200, got {status} {body}"
        );
        let repeated = describe_campaign(&router, region, &id, case).await;
        assert_schedule_is(&repeated, updated, case, "the repeated update");

        // The call changes the schedule and NOTHING else about the campaign.
        let before_rest = campaign_without(&before, "schedule");
        let after_rest = campaign_without(&repeated, "schedule");
        assert_eq!(
            before_rest, after_rest,
            "case {case:?}: expected every other member untouched, got {after_rest} \
             (was {before_rest})"
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and stores nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_takes_no_schedule() {
    let region = "eu-west-1";
    let router = router();
    let instance = "schedule-update-404-instance";

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let kept = schedule(-60.0, 3_600.0, Some("PT1H"));
    let control = create_campaign(&router, region, instance, "control", Some(kept.clone())).await;

    // One campaign created and then deleted: its id was valid a moment ago.
    let deleted = create_campaign(&router, region, instance, "doomed", Some(kept.clone())).await;
    let (status, body) = call(
        &router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{deleted}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete the doomed campaign\": expected 200, got {status} {body}"
    );

    // (case, id) — ids spanning `CampaignId` (len 1..256, pattern ^[-:/a-zA-Z0-9]+$).
    let cases: Vec<(String, String)> = vec![
        ("never created".to_string(), "missing".to_string()),
        ("single character".to_string(), "0".to_string()),
        (
            // `/` is left out: a `@httpLabel` is one path segment, so an id carrying
            // one does not address this operation's URI at all.
            "every character class the pattern admits".to_string(),
            "A-b:C1".to_string(),
        ),
        ("the 256-character maximum".to_string(), "c".repeat(256)),
        ("created and then deleted".to_string(), deleted.clone()),
    ];

    let sent = schedule(-10.0, 1_800.0, None);
    for (case, id) in cases {
        let (status, body) = update_schedule(&router, region, &id, &sent).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body["message"].as_str(),
            Some(CAMPAIGN_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the fixed not-found text, got {:?}",
            body["message"]
        );

        // A refused call stores nothing: the campaign that does exist keeps the
        // schedule it was created with.
        let campaign = describe_campaign(&router, region, &control, &case).await;
        assert_schedule_is(&campaign, &kept, &case, "the control campaign");
    }
}

/// One row of the state table in [`the_stored_schedule_decides_the_state_the_campaign_is_at`].
struct Reschedule {
    case: &'static str,
    /// The window the update writes, as offsets in seconds from now.
    window: (f64, f64),
    /// How long the case waits after the update before reading the state.
    wait: Duration,
    expected: &'static str,
}

/// The window the update stores is the one the campaign's state is derived from: a
/// campaign that has entered its run is `Completed` once the NEW `endTime` has passed,
/// and one rescheduled into a window that is still open keeps running.
///
/// The rows differ along where the new `endTime` sits relative to the read: already
/// past, passing while nothing calls, and still ahead.
#[tokio::test]
async fn the_stored_schedule_decides_the_state_the_campaign_is_at() {
    let region = "us-east-1";
    let router = router();
    let instance = "schedule-update-state-instance";

    // A window that closes shortly after the update, so the row below crosses the new
    // `endTime` with NOTHING calling in between. The wait below is comfortably longer
    // than the window, so a loaded machine delays the read rather than racing it.
    const SHORT_WINDOW_SECS: f64 = 1.0;
    const WAIT_OUT_SHORT_WINDOW: Duration = Duration::from_millis(1_500);

    let cases: &[Reschedule] = &[
        Reschedule {
            case: "rescheduled into a window that has already closed",
            window: (-3_600.0, -1.0),
            wait: Duration::ZERO,
            expected: "Completed",
        },
        Reschedule {
            case: "rescheduled into a window that closes a month ago",
            window: (-2_595_600.0, -2_592_000.0),
            wait: Duration::ZERO,
            expected: "Completed",
        },
        Reschedule {
            case: "rescheduled into a window that closes while the campaign runs",
            window: (-1.0, SHORT_WINDOW_SECS),
            wait: WAIT_OUT_SHORT_WINDOW,
            expected: "Completed",
        },
        Reschedule {
            case: "rescheduled into a window that is still open",
            window: (-60.0, 3_600.0),
            wait: WAIT_OUT_SHORT_WINDOW,
            expected: "Running",
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let Reschedule {
            case,
            window,
            wait,
            expected,
        } = row;
        // The campaign starts with no schedule at all, so the state below is owed to
        // the window this call writes and to nothing the create carried.
        let id = create_campaign(&router, region, instance, &format!("run-{index}"), None).await;
        drive_lifecycle(&router, region, &id, &["start"], case).await;
        let running = campaign_state(&router, region, &id, case).await;
        assert_eq!(
            running, "Running",
            "case {case:?}: expected the campaign to be Running before the update, got {running}"
        );

        let sent = schedule(window.0, window.1, None);
        let (status, body) = update_schedule(&router, region, &id, &sent).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignSchedule expected 200, got {status} {body}"
        );
        if !wait.is_zero() {
            tokio::time::sleep(*wait).await;
        }

        let actual = campaign_state(&router, region, &id, case).await;
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected state {expected:?}, got {actual:?}"
        );
        // Reading the state writes nothing, and the schedule the update stored is
        // what the read derived that state from.
        let campaign = describe_campaign(&router, region, &id, case).await;
        assert_schedule_is(&campaign, &sent, case, "DescribeCampaign");
    }
}

/// Which lifecycle states take the update: every non-terminal one, through the shared
/// predicate the configuration operations refuse on.
///
/// The table spans the whole `CampaignState` catalog against that predicate, drives the
/// update over HTTP in each state the lifecycle can actually reach — a different window
/// in each — and then asserts a campaign the clock has completed refuses it with HTTP
/// 409 naming that state and keeps the schedule it held.
#[tokio::test]
async fn every_non_terminal_state_takes_the_update_and_a_terminal_one_refuses_it() {
    // (state, takes a configuration change)
    let catalog: &[(CampaignState, bool)] = &[
        (CampaignState::Initialized, true),
        (CampaignState::Running, true),
        (CampaignState::Paused, true),
        (CampaignState::Stopped, true),
        (CampaignState::Completed, false),
        (CampaignState::Failed, false),
    ];
    for (state, expected) in catalog {
        let actual = state.accepts_configuration_change();
        assert_eq!(
            actual,
            *expected,
            "case {:?}: expected accepts_configuration_change {expected}, got {actual}",
            state.as_wire()
        );
    }

    let region = "ap-south-1";
    let router = router();
    let instance = "schedule-update-lifecycle-instance";
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    // Each lifecycle action the emulator's state machine offers, the state it leaves
    // the campaign in, and the window the update written there carries — each one
    // still open, so the walk is not cut short by the clock.
    let walk: &[(&str, CampaignState, (f64, f64))] = &[
        ("", CampaignState::Initialized, (-60.0, 3_600.0)),
        ("start", CampaignState::Running, (-30.0, 7_200.0)),
        ("pause", CampaignState::Paused, (60.0, 10_800.0)),
        ("stop", CampaignState::Stopped, (-3_600.0, 14_400.0)),
    ];
    for (action, state, window) in walk {
        if !action.is_empty() {
            drive_lifecycle(&router, region, &id, &[action], state.as_wire()).await;
        }
        let case = format!("in state {}", state.as_wire());
        let reported = campaign_state(&router, region, &id, &case).await;
        assert_eq!(
            reported,
            state.as_wire(),
            "case {case:?}: expected state {:?}, got {reported:?}",
            state.as_wire()
        );

        let sent = schedule(window.0, window.1, None);
        let (status, body) = update_schedule(&router, region, &id, &sent).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body}"
        );
        let campaign = describe_campaign(&router, region, &id, &case).await;
        assert_schedule_is(&campaign, &sent, &case, "DescribeCampaign");
    }

    // A campaign started inside a window that has since closed is `Completed`, the
    // terminal state nothing moves out of: it is past being rescheduled, and the
    // refusal stores nothing.
    let case = "a campaign the clock has completed";
    let kept = schedule(-7_200.0, -3_600.0, Some("PT1H"));
    let completed =
        create_campaign(&router, region, instance, "completed", Some(kept.clone())).await;
    drive_lifecycle(&router, region, &completed, &["start"], case).await;
    assert_eq!(
        campaign_state(&router, region, &completed, case).await,
        "Completed",
        "case {case:?}: expected the campaign to be Completed before the update"
    );

    let (status, refused) =
        update_schedule(&router, region, &completed, &schedule(-60.0, 3_600.0, None)).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected HTTP 409, got {status} {refused}"
    );
    assert!(
        error_code(&refused).ends_with("InvalidCampaignStateException"),
        "case {case:?}: expected InvalidCampaignStateException, got {}",
        error_code(&refused)
    );
    assert_eq!(
        refused["state"].as_str(),
        Some("Completed"),
        "case {case:?}: expected the refusal to name state \"Completed\", got {:?}",
        refused["state"]
    );
    let campaign = describe_campaign(&router, region, &completed, case).await;
    assert_schedule_is(&campaign, &kept, case, "the refused campaign");
}

/// `refreshFrequency` is an `Iso8601Duration`, so a value outside the modelled pattern
/// is refused as a bad request and stores nothing.
///
/// The rows differ along what the rule generalises over: a duration missing its `P`
/// designator, one naming a unit the pattern does not admit, one whose time part is
/// not introduced by `T`, free text, and a value past the modelled 50-character
/// maximum. A value the pattern does admit and the table above does not use is
/// asserted to be accepted, so the check refuses only what the pattern leaves out.
#[tokio::test]
async fn a_refresh_frequency_outside_the_modelled_pattern_is_refused() {
    let region = "eu-central-1";
    let router = router();
    let instance = "schedule-update-validation-instance";

    let kept = schedule(-60.0, 3_600.0, Some("PT15M"));
    let id = create_campaign(&router, region, instance, "validated", Some(kept.clone())).await;

    // (case, the `refreshFrequency` the schedule carries) — the pattern is
    // ^P(?:([-+]?[0-9]+)D)?(T(?:([-+]?[0-9]+)H)?(?:([-+]?[0-9]+)M)?(?:([-+]?[0-9]+)(?:[.,]([0-9]{0,9}))?S)?)?$
    // with @length 0..50 (source: `Schedule$refreshFrequency`).
    let cases: Vec<(&str, String)> = vec![
        ("no P designator", "1D".to_string()),
        ("a unit the pattern does not admit", "P1Y".to_string()),
        ("a time part not introduced by T", "P2H".to_string()),
        ("free text", "every hour".to_string()),
        ("past the 50-character maximum", format!("PT{}S", "1".repeat(50))),
    ];

    for (case, frequency) in cases {
        let sent = schedule(-10.0, 1_800.0, Some(&frequency));
        let (status, response) = update_schedule(&router, region, &id, &sent).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&response)
        );
        let campaign = describe_campaign(&router, region, &id, case).await;
        assert_schedule_is(&campaign, &kept, case, "the refused campaign");
    }

    // A duration the pattern admits, unused above, is accepted.
    let accepted = schedule(-10.0, 1_800.0, Some("P2DT12H"));
    let (status, response) = update_schedule(&router, region, &id, &accepted).await;
    assert_eq!(
        status, 200,
        "case \"a duration the pattern admits\": expected HTTP 200, got {status} {response}"
    );
    let campaign =
        describe_campaign(&router, region, &id, "a duration the pattern admits").await;
    assert_schedule_is(
        &campaign,
        &accepted,
        "a duration the pattern admits",
        "DescribeCampaign",
    );
}
