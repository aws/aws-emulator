// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for re-timing a scheduled change: `UpdateScheduledAction`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness comes from [`super::domain_lifecycle`] and the seeded
//! scheduled changes from [`super::scheduled_actions`] — the ONE table of them this
//! crate's tests drive; this suite adds no second copy of either.
//!
//! The rule under test: a reschedule moves ONE scheduled change of the domain the
//! request names to the instant its `ScheduleAt` names — the moment of the call, the
//! timestamp the caller gave, or the next opening of the domain's off-peak window —
//! leaving it waiting for that instant and scheduled by the caller, and reports the
//! stored record; `DesiredStartTime` belongs with exactly one `ScheduleAt` value and
//! that pairing is judged BEFORE anything the request names is resolved; a change the
//! service has already begun or finished can no longer be re-timed; and a name or an
//! action id naming nothing the account holds has no change to reschedule.
//!
//! So the assertions span a SET of inputs rather than one call: every `ScheduleAt`
//! value, a timestamp an hour, a month and a millisecond-granular offset away, an
//! off-peak window configured and left at the start the model defaults to, every value
//! of `ActionType` and of `ActionStatus`, every way the `DesiredStartTime` pairing can
//! be wrong crossed with a known target, an unknown one and a nonexistent domain, four
//! shapes of unresolvable action id and four names no domain answers to.
//!
//! The changes themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the operations that SCHEDULE one are
//! still scaffold stubs, so no wire call can leave one yet. Every rule this operation
//! can be observed by is asserted over the wire, and each accepted reschedule is read
//! back through `ListScheduledActions`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    ACTION_STATUS_VALUES, ACTION_TYPE_VALUES, SCHEDULE_AT_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::epoch_millis;

use super::domain_lifecycle::{
    baseline_create, create_baseline, error_code, Answer, Emulator, PREFIX,
    RECORDED_NOT_FOUND_STATUS,
};
use super::scheduled_actions::{base_instant, base_millis, rows, seed, Row};

/// `UpdateScheduledAction`:
/// `PUT /2021-01-01/opensearch/domain/{DomainName}/scheduledAction/update`.
///
/// `pub(crate)` because the suite that asserts this service's refusal WORDING drives the
/// same operation through this wrapper rather than growing a second copy of it.
pub(crate) async fn update(emulator: &Emulator, domain: &str, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/domain/{domain}/scheduledAction/update"),
            Some(body),
        )
        .await
}

/// The request body of a reschedule, with `DesiredStartTime` only where one is given.
///
/// `pub(crate)` for the reason [`update`] is.
pub(crate) fn body(
    action_id: &str,
    action_type: &str,
    schedule_at: &str,
    desired: Option<i64>,
) -> Value {
    let mut body = json!({
        "ActionID": action_id,
        "ActionType": action_type,
        "ScheduleAt": schedule_at,
    });
    if let Some(desired) = desired {
        body["DesiredStartTime"] = json!(desired);
    }
    body
}

/// The `ScheduledAction` of an accepted reschedule.
fn action(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let action = answer.body["ScheduledAction"].clone();
    assert!(
        action.is_object(),
        "case {case}: expected a ScheduledAction object, got {:?}",
        answer.body
    );
    action
}

/// `ListScheduledActions` for `domain`, asserting it was accepted.
async fn list(emulator: &Emulator, domain: &str, case: &str) -> Vec<Value> {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{domain}/scheduledActions"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the listing to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ScheduledActions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a ScheduledActions list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The change `action_id` of `domain`'s listing reports, as the listing reports it.
async fn listed(emulator: &Emulator, domain: &str, action_id: &str, case: &str) -> Value {
    list(emulator, domain, case)
        .await
        .into_iter()
        .find(|action| action["Id"].as_str() == Some(action_id))
        .unwrap_or_else(|| panic!("case {case}: expected the listing to report {action_id:?}"))
}

/// The seeded change `action_id` names, so a case asserts the members a reschedule
/// must NOT touch against the record it started from.
fn seeded(action_id: &str) -> Row {
    rows()
        .into_iter()
        .find(|row| row.id == action_id)
        .unwrap_or_else(|| panic!("case seeded({action_id:?}): the table holds that change"))
}

/// The next instant after `now` at `hours`:`minutes` UTC — the test's own reading of
/// when a daily off-peak window opens, computed here rather than taken from the
/// emulator.
fn next_window_opening(now: SystemTime, hours: u64, minutes: u64) -> SystemTime {
    let day = 24 * 60 * 60;
    let since = now
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs();
    let opening =
        UNIX_EPOCH + Duration::from_secs(since - since % day + hours * 3_600 + minutes * 60);
    if opening > now {
        opening
    } else {
        opening + Duration::from_secs(day)
    }
}

/// The domain whose off-peak window has a custom start time.
const WINDOWED: &str = "sched-window";

/// The domain whose off-peak window has a custom start time and whose legacy
/// `OffPeakWindowOptions$Enabled` toggle is off.
const TOGGLED_OFF: &str = "sched-window-off";

/// The domain that configured no off-peak window at all, so its window opens at the
/// start the model's `WindowStartTime` members default to.
const PLAIN: &str = "sched-no-window";

/// One accepted reschedule to drive: the domain, the change, the `ScheduleAt` it names
/// and the `DesiredStartTime` it carries.
struct Accepted {
    case: &'static str,
    domain: &'static str,
    action_id: &'static str,
    schedule_at: &'static str,
    desired: Option<i64>,
}

/// The accepted-reschedule table: one row per way an accepted call differs from
/// another — the three `ScheduleAt` values, three distances for a timestamp (an hour,
/// a month and a millisecond-granular offset) and an off-peak window configured,
/// defaulted and toggled off — between them on a change of every `ActionType`.
///
/// A function of the base instant so the timestamps it names are the ones the seeded
/// changes were measured from, and so [`every_schedule_at_value_is_covered`] can read
/// the same table the cases drive.
fn accepted_rows(base: SystemTime) -> Vec<Accepted> {
    vec![
        Accepted {
            case: "NOW",
            domain: WINDOWED,
            action_id: "ssu-pending",
            schedule_at: "NOW",
            desired: None,
        },
        Accepted {
            case: "TIMESTAMP an hour away",
            domain: WINDOWED,
            action_id: "heap-tuning",
            schedule_at: "TIMESTAMP",
            desired: Some(base_millis(base) + 3_600_000),
        },
        Accepted {
            case: "TIMESTAMP a month away",
            domain: WINDOWED,
            action_id: "young-gen",
            schedule_at: "TIMESTAMP",
            desired: Some(base_millis(base) + 30 * 86_400_000),
        },
        Accepted {
            case: "TIMESTAMP with millisecond granularity",
            domain: PLAIN,
            action_id: "heap-tuning",
            schedule_at: "TIMESTAMP",
            desired: Some(base_millis(base) + 1_234),
        },
        Accepted {
            case: "OFF_PEAK_WINDOW, custom start",
            domain: WINDOWED,
            action_id: "ssu-pending",
            schedule_at: "OFF_PEAK_WINDOW",
            desired: None,
        },
        Accepted {
            case: "OFF_PEAK_WINDOW, default start",
            domain: PLAIN,
            action_id: "young-gen",
            schedule_at: "OFF_PEAK_WINDOW",
            desired: None,
        },
        Accepted {
            case: "OFF_PEAK_WINDOW, window toggled off",
            domain: TOGGLED_OFF,
            action_id: "heap-tuning",
            schedule_at: "OFF_PEAK_WINDOW",
            desired: None,
        },
    ]
}

/// The seeded table spans the whole `ActionType` and `ActionStatus` catalogs, so a
/// value added to the model is not silently left untested by the cases below.
#[test]
fn every_action_type_and_status_is_covered() {
    for action_type in ACTION_TYPE_VALUES {
        assert!(
            rows().iter().any(|row| row.action_type == *action_type),
            "case ActionType={action_type}: the seeded table covers every modelled action type"
        );
    }
    for status in ACTION_STATUS_VALUES {
        assert!(
            rows().iter().any(|row| row.status == *status),
            "case ActionStatus={status}: the seeded table covers every modelled action status"
        );
    }
}

/// The accepted-reschedule table itself drives every `ScheduleAt` the model declares,
/// so a value added to the model is not silently left untested.
#[test]
fn every_schedule_at_value_is_covered() {
    let table = accepted_rows(base_instant());
    for schedule_at in SCHEDULE_AT_VALUES {
        assert!(
            table.iter().any(|row| row.schedule_at == *schedule_at),
            "case ScheduleAt={schedule_at}: the reschedule table drives every modelled value, \
             got {:?}",
            table
                .iter()
                .map(|row| row.schedule_at)
                .collect::<Vec<&str>>()
        );
    }
}

#[tokio::test]
async fn a_reschedule_moves_the_change_to_the_instant_the_request_names() {
    let emulator = Emulator::new();

    // One domain with a custom off-peak window start, one with none at all and one
    // whose legacy `Enabled` toggle is off, so `OFF_PEAK_WINDOW` is exercised against
    // a configured window, against the window the SERVICE assigns a domain that named
    // no start time, and against the toggle — which says nothing about a domain created
    // now, since "all domains created after [February 16, 2023] have an off-peak
    // window by default".
    let windowed = WINDOWED;
    let created = emulator
        .create({
            let mut request = baseline_create(windowed);
            request["OffPeakWindowOptions"] = json!({
                "Enabled": true,
                "OffPeakWindow": { "WindowStartTime": { "Hours": 2, "Minutes": 30 } },
            });
            request
        })
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({windowed:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    let toggled_off = TOGGLED_OFF;
    let created = emulator
        .create({
            let mut request = baseline_create(toggled_off);
            request["OffPeakWindowOptions"] = json!({
                "Enabled": true,
                "OffPeakWindow": { "WindowStartTime": { "Hours": 17, "Minutes": 0 } },
            });
            request
        })
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({toggled_off:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    // The legacy toggle OFF is a state no call can leave a domain in — "all domains
    // created after [February 16, 2023] have the off-peak window enabled by default"
    // and it "can't be disabled after it's enabled" — so it is written into the store
    // directly, the same way this suite seeds the changes it reschedules.
    emulator
        .store()
        .lock()
        .domains
        .get_mut(toggled_off)
        .expect("the domain just created is in the store")
        .off_peak_window_options
        .as_mut()
        .expect("the create carried off-peak window options")
        .enabled = Some(false);
    let plain = PLAIN;
    create_baseline(&emulator, plain).await;

    let base = base_instant();
    seed(&emulator, windowed, base);
    seed(&emulator, toggled_off, base);
    seed(&emulator, plain, base);

    for row in accepted_rows(base) {
        let (case, domain, action_id, schedule_at, desired) = (
            row.case,
            row.domain,
            row.action_id,
            row.schedule_at,
            row.desired,
        );
        let before = seeded(action_id);

        let sent_at = SystemTime::now();
        let answer = update(
            &emulator,
            domain,
            body(action_id, before.action_type, schedule_at, desired),
        )
        .await;
        let answered_at = SystemTime::now();
        let reported = action(&answer, case);

        assert_eq!(
            reported["Id"].as_str(),
            Some(action_id),
            "case {case}: the reschedule reports the change it moved, got {reported:?}"
        );
        // The members that belong to the CHANGE rather than to its timing are the
        // stored record's own.
        assert_eq!(
            reported["Type"].as_str(),
            Some(before.action_type),
            "case {case}: expected the stored type, got {reported:?}"
        );
        assert_eq!(
            reported["Severity"].as_str(),
            Some(before.severity),
            "case {case}: expected the stored severity, got {reported:?}"
        );
        assert_eq!(
            reported["Description"].as_str(),
            before.description,
            "case {case}: expected the stored description, got {reported:?}"
        );
        assert_eq!(
            reported["Mandatory"].as_bool(),
            Some(before.mandatory),
            "case {case}: expected the stored Mandatory, got {reported:?}"
        );
        assert_eq!(
            reported["Cancellable"].as_bool(),
            Some(before.cancellable),
            "case {case}: expected the stored Cancellable, got {reported:?}"
        );
        // A change a caller re-timed is one a caller scheduled, and one that has an
        // instant to happen at is waiting for it.
        assert_eq!(
            reported["ScheduledBy"].as_str(),
            Some("CUSTOMER"),
            "case {case}: expected the change to be scheduled by the caller, got {reported:?}"
        );
        assert_eq!(
            reported["Status"].as_str(),
            Some("PENDING_UPDATE"),
            "case {case}: expected the change to be waiting for its instant, got {reported:?}"
        );

        let scheduled = reported["ScheduledTime"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: expected a ScheduledTime, got {reported:?}"));
        match (schedule_at, desired) {
            // "Immediately schedules the update to happen in the current hour": the
            // instant of the call, which is between the two readings around it.
            ("NOW", _) => assert!(
                (epoch_millis(sent_at)..=epoch_millis(answered_at)).contains(&scheduled),
                "case {case}: expected the instant of the call, got {scheduled}"
            ),
            ("TIMESTAMP", Some(desired)) => assert_eq!(
                scheduled, desired,
                "case {case}: expected the timestamp the caller gave, got {scheduled}"
            ),
            // The next opening of the domain's off-peak window, which is at the
            // window's start time in UTC and strictly ahead of the call.
            _ => {
                let (hours, minutes) = match domain {
                    WINDOWED => (2, 30),
                    TOGGLED_OFF => (17, 0),
                    // The window the SERVICE assigns a domain that named no start
                    // time, which is what the domain carries from the moment it is
                    // created.
                    _ => (5, 0),
                };
                // The handler reads its own clock, so the opening is computed from both
                // readings around the call: a call that straddles the opening second
                // itself legitimately lands on either of them.
                let from_sent = epoch_millis(next_window_opening(sent_at, hours, minutes));
                let from_answered =
                    epoch_millis(next_window_opening(answered_at, hours, minutes));
                assert!(
                    scheduled == from_sent || scheduled == from_answered,
                    "case {case}: expected the next off-peak window opening ({from_sent} or \
                     {from_answered}), got {scheduled}"
                );
                assert!(
                    scheduled > epoch_millis(answered_at),
                    "case {case}: an upcoming window opens after the call, got {scheduled}"
                );
            }
        }

        // Read back through the listing: the record the reschedule answered with is
        // the one the domain holds.
        let listed = listed(&emulator, domain, action_id, case).await;
        assert_eq!(
            listed, reported,
            "case {case}: the listing reports the record the reschedule answered with, \
             got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_request_is_judged_before_the_change_it_names_is_looked_up() {
    let emulator = Emulator::new();
    let domain = "sched-pairing";
    create_baseline(&emulator, domain).await;
    let base = base_instant();
    seed(&emulator, domain, base);
    let baseline = list(&emulator, domain, "pairing baseline").await;

    // "Only specify this parameter if you set ScheduleAt to TIMESTAMP", and that value
    // "must also provide a value for DesiredStartTime": every way the pairing can be
    // wrong, crossed with a target that exists, one the domain holds no change under,
    // and a domain nothing created — the refusal is the same whichever, so the pairing
    // is judged before anything the request names is resolved.
    let mut cases: Vec<(String, &str, Value)> = Vec::new();
    for (target_label, target_domain, action_id) in [
        ("a change the domain holds", domain, "ssu-pending"),
        ("a change nothing scheduled", domain, "no-such-action"),
        (
            "a domain nothing created",
            "sched-no-such-domain",
            "ssu-pending",
        ),
    ] {
        cases.push((
            format!("TIMESTAMP without DesiredStartTime / {target_label}"),
            target_domain,
            body(action_id, "SERVICE_SOFTWARE_UPDATE", "TIMESTAMP", None),
        ));
        for schedule_at in ["NOW", "OFF_PEAK_WINDOW"] {
            cases.push((
                format!("{schedule_at} with DesiredStartTime / {target_label}"),
                target_domain,
                body(
                    action_id,
                    "SERVICE_SOFTWARE_UPDATE",
                    schedule_at,
                    Some(base_millis(base) + 3_600_000),
                ),
            ));
        }
    }

    for (case, target_domain, request) in cases {
        let answer = update(&emulator, target_domain, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    assert_eq!(
        list(&emulator, domain, "pairing after refusals").await,
        baseline,
        "case pairing: a refused request moves no change"
    );
}

#[tokio::test]
async fn an_action_id_that_names_no_change_of_the_domain_is_refused() {
    let emulator = Emulator::new();
    let domain = "sched-unknown-id";
    let sibling = "sched-sibling";
    create_baseline(&emulator, domain).await;
    create_baseline(&emulator, sibling).await;
    let base = base_instant();
    seed(&emulator, domain, base);
    let baseline = list(&emulator, domain, "unknown-id baseline").await;

    // One row per shape of unresolvable id: one invented, one shaped like an
    // identifier, one that is the id of a change of ANOTHER domain, and one whose
    // change this domain had until it was taken out of the store.
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let record = guard
            .domains
            .get_mut(domain)
            .expect("the domain must have been created");
        record.scheduled_actions.remove("heap-failed");
    }
    for (label, target, action_id) in [
        ("an invented id", domain, "no-such-action"),
        (
            "an id shaped like an identifier",
            domain,
            "00000000-0000-0000-0000-000000000000",
        ),
        (
            "an id from another domain's listing",
            sibling,
            "ssu-pending",
        ),
        ("an id whose change was removed", domain, "heap-failed"),
    ] {
        let case = format!("{label} ({action_id:?})");
        let answer = update(
            &emulator,
            target,
            body(action_id, "SERVICE_SOFTWARE_UPDATE", "NOW", None),
        )
        .await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }

    let after: Vec<Value> = list(&emulator, domain, "unknown-id after refusals").await;
    let expected: Vec<Value> = baseline
        .into_iter()
        .filter(|action| action["Id"].as_str() != Some("heap-failed"))
        .collect();
    assert_eq!(
        after, expected,
        "case unknown id: a refused request moves no change"
    );
    assert!(
        list(&emulator, sibling, "sibling listing").await.is_empty(),
        "case unknown id: the sibling domain still has nothing scheduled"
    );
}

#[tokio::test]
async fn an_action_type_that_is_not_the_changes_own_is_refused() {
    let emulator = Emulator::new();
    let domain = "sched-type-match";
    create_baseline(&emulator, domain).await;
    let base = base_instant();
    seed(&emulator, domain, base);

    // Every change the service has not begun crossed with every modelled `ActionType`:
    // only its own type names the same change its id does, and every other is a
    // request that cannot be served as asked.
    for row in rows() {
        if row.status == "IN_PROGRESS" || row.status == "COMPLETED" {
            continue;
        }
        for action_type in ACTION_TYPE_VALUES {
            let case = format!("{} as {action_type}", row.id);
            let answer = update(&emulator, domain, body(row.id, action_type, "NOW", None)).await;
            if *action_type == row.action_type {
                assert_eq!(
                    action(&answer, &case)["Type"].as_str(),
                    Some(row.action_type),
                    "case {case}: the change's own type is accepted, got {:?}",
                    answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected the modelled validation refusal, got {:?}",
                answer.body
            );
            let listed = listed(&emulator, domain, row.id, &case).await;
            assert_eq!(
                listed["Type"].as_str(),
                Some(row.action_type),
                "case {case}: a refused reschedule leaves the change as it was, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_change_the_service_has_begun_or_finished_can_no_longer_be_rescheduled() {
    let emulator = Emulator::new();
    let domain = "sched-statuses";
    create_baseline(&emulator, domain).await;
    let base = base_instant();
    seed(&emulator, domain, base);

    // The seeded table carries a change in every value of `ActionStatus`: one the
    // service has begun or finished is in use by that change and can no longer be
    // re-timed, and every other status leaves an instant to move.
    for row in rows() {
        let case = format!("a {} change ({})", row.status, row.id);
        let answer = update(&emulator, domain, body(row.id, row.action_type, "NOW", None)).await;
        if row.status == "IN_PROGRESS" || row.status == "COMPLETED" {
            assert_eq!(
                answer.status, 409,
                "case {case}: expected HTTP 409, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ConflictException",
                "case {case}: expected the modelled conflict refusal, got {:?}",
                answer.body
            );
            let listed = listed(&emulator, domain, row.id, &case).await;
            assert_eq!(
                listed["Status"].as_str(),
                Some(row.status),
                "case {case}: a refused reschedule leaves the change as it was, got {listed:?}"
            );
            continue;
        }
        let reported = action(&answer, &case);
        assert_eq!(
            reported["Status"].as_str(),
            Some("PENDING_UPDATE"),
            "case {case}: a re-timed change is waiting for its instant, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_the_account_does_not_hold_has_no_change_to_reschedule() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "sched-deleted-domain").await;
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/sched-deleted-domain"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(sched-deleted-domain): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Rows differ along the shape of the name: one never created, one at the shortest
    // the model admits, one at its longest, and one the account held until it was
    // deleted.
    for name in [
        "never-created",
        "abc",
        "a-domain-name-at-its-upper-b",
        "sched-deleted-domain",
    ] {
        let case = format!("UpdateScheduledAction({name:?})");
        let answer = update(
            &emulator,
            name,
            body("ssu-pending", "SERVICE_SOFTWARE_UPDATE", "NOW", None),
        )
        .await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }
}
