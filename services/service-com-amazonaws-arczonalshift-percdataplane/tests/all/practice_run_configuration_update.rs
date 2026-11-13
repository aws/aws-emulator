// SPDX-License-Identifier: CC0-1.0

//! Changing the practice run configuration ARC holds for a managed resource, driven end
//! to end over the wire.
//!
//! Every case here builds the production `MultiServiceRouter`, registers this service on
//! one shared engine and dispatches real `restJson1` requests, so what is pinned is the
//! HTTP behaviour rather than a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * a change is applied MEMBER BY MEMBER on top of the configuration ARC already holds:
//!   a member the request names replaces the held one, a member it names empty removes
//!   what was held, and a member it does not name is left exactly as held — and the
//!   result is PERSISTED, so the call's own answer and every later read
//!   (`GetManagedResource`, `ListManagedResources`) report it;
//! * a resource has to HAVE a configuration for one to be changed, so a change asked of a
//!   resource that has none is refused 409 `ConflictException`
//!   (`PracticeConfigurationDoesNotExist`) and persists nothing;
//! * the target of the call is authorized before it is resolved, so a `resourceIdentifier`
//!   ARC holds nothing for is refused 403 `AccessDeniedException` whatever the change asks
//!   for;
//! * a configuration ARC cannot read is refused 400 `ValidationException` and leaves the
//!   held configuration untouched — the merged configuration is read exactly as a created
//!   one is.
//!
//! The rows differ along what those rules generalise over: WHICH members a change names
//! (each one alone, several at once, every one, and none at all), whether it replaces or
//! removes, how MANY entries each named list carries (each end of the modelled range),
//! which configuration was held before the change, and which resource it is held for —
//! the ones this store was seeded with and one its owner registered.

use serde_json::{json, Value};

use super::practice_run_configuration::{
    alarms, assert_holds, dates, get_managed_resource, hold, listed_practice_run_status, windows,
    REGISTERED_ARN, REGISTERED_ZONES, ROWS, UNHELD_TARGETS,
};
use super::registered_managed_resources::register;
use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded};
use service_com_amazonaws_arczonalshift_percdataplane::state::{
    PRACTICE_CONFIGURATION_MISSING_MESSAGE, PRACTICE_CONFIGURATION_MISSING_REASON,
    TARGET_UNAVAILABLE_MESSAGE,
};

/// `UpdatePracticeRunConfiguration` over the wire: `PATCH
/// /configuration/{resourceIdentifier}`, the binding the model declares
/// (`smithy_query(query="operation", operation="UpdatePracticeRunConfiguration")`), with
/// the members to change in the body.
async fn update(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource: &str,
    members: &serde_json::Map<String, Value>,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/configuration/{}", encoded(resource)),
        &Value::Object(members.clone()).to_string(),
    )
    .await
}

/// The members one change names, as the request body carries them.
fn members(pairs: &[(&str, Value)]) -> serde_json::Map<String, Value> {
    pairs
        .iter()
        .map(|(member, value)| ((*member).to_string(), value.clone()))
        .collect()
}

/// The changes under test, as `(case, the members the request names)`.
///
/// One row per thing the operation documents a caller may do — "add, change, or remove
/// the blocking alarm; change the outcome alarm; or add, change, or remove blocking dates
/// or time windows" — over the five members the request may carry
/// (`smithy_query(query="operation", operation="UpdatePracticeRunConfiguration")`), with
/// the sizes at each end of each member's modelled range: `outcomeAlarms` 1..10,
/// `blockingAlarms` 0..10, and `blockedWindows`, `allowedWindows` and `blockedDates` each
/// 0..15.
fn changes() -> Vec<(&'static str, serde_json::Map<String, Value>)> {
    vec![
        (
            "names no member at all",
            members(&[]),
        ),
        (
            "changes the outcome alarms",
            members(&[("outcomeAlarms", alarms("changed-outcome", 3))]),
        ),
        (
            "changes the outcome alarms to the one the model requires at minimum",
            members(&[("outcomeAlarms", alarms("sole-outcome", 1))]),
        ),
        (
            "changes the outcome alarms to the most the model allows",
            members(&[("outcomeAlarms", alarms("changed-outcome", 10))]),
        ),
        (
            "adds or changes the blocking alarms",
            members(&[("blockingAlarms", alarms("changed-blocking", 2))]),
        ),
        (
            "removes the blocking alarms with an empty list",
            members(&[("blockingAlarms", json!([]))]),
        ),
        (
            "adds or changes the blocked windows",
            members(&[("blockedWindows", windows(2))]),
        ),
        (
            "removes the blocked windows with an empty list",
            members(&[("blockedWindows", json!([]))]),
        ),
        (
            "adds the most allowed windows the model allows",
            members(&[("allowedWindows", windows(15))]),
        ),
        (
            "removes the allowed windows with an empty list",
            members(&[("allowedWindows", json!([]))]),
        ),
        (
            "adds or changes the blocked dates",
            members(&[("blockedDates", dates(4))]),
        ),
        (
            "removes the blocked dates with an empty list",
            members(&[("blockedDates", json!([]))]),
        ),
        (
            "changes the alarms and the windows together",
            members(&[
                ("outcomeAlarms", alarms("paired-outcome", 2)),
                ("blockingAlarms", alarms("paired-blocking", 1)),
                ("blockedWindows", windows(1)),
            ]),
        ),
        (
            "removes every blocker at once",
            members(&[
                ("blockingAlarms", json!([])),
                ("blockedWindows", json!([])),
                ("allowedWindows", json!([])),
                ("blockedDates", json!([])),
            ]),
        ),
        (
            "names every member, each at the most the model allows",
            members(&[
                ("outcomeAlarms", alarms("full-outcome", 10)),
                ("blockingAlarms", alarms("full-blocking", 10)),
                ("blockedWindows", windows(15)),
                ("allowedWindows", windows(15)),
                ("blockedDates", dates(15)),
            ]),
        ),
    ]
}

/// The configuration ARC holds after `change` is applied to `held`: the named members
/// replace what was held, and the members the change does not name stay as held.
fn merged(
    held: &serde_json::Map<String, Value>,
    change: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut expected = held.clone();
    for (member, value) in change {
        expected.insert(member.clone(), value.clone());
    }
    expected
}

#[tokio::test]
async fn a_change_is_applied_member_by_member_to_the_held_configuration_and_persisted() {
    for row in ROWS {
        for (index, (change_case, change)) in changes().into_iter().enumerate() {
            // One emulator per case, so each case's reads are about its own store, and
            // the targets cycle through the resources ARC was seeded with and one its
            // owner registered: a configuration is changed the same way whichever way the
            // resource reached ARC.
            let (router, engine) = emulator();
            register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
            let mut targets: Vec<String> =
                seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
            targets.push(REGISTERED_ARN.to_string());
            let target = targets[index % targets.len()].clone();

            let case = format!("held {}, change {change_case}, {target}", row.case);
            let held = hold(&router, row, &target).await;
            let expected = merged(&held, &change);

            let (status, answer) = update(&router, &target, &change).await;
            assert_eq!(
                status, 200,
                "case {case}: UpdatePracticeRunConfiguration answers 200, got {status} \
                 {answer}"
            );
            assert_eq!(
                (answer["arn"].as_str(), answer["zonalAutoshiftStatus"].as_str()),
                (Some(target.as_str()), Some("DISABLED")),
                "case {case}: expected the resource's ARN and its current autoshift \
                 status, got {answer}"
            );
            assert!(
                answer["name"].as_str().is_some_and(|name| !name.is_empty()),
                "case {case}: the resource's name is required, got {answer}"
            );
            assert_holds(&case, &answer["practiceRunConfiguration"], &expected);

            // Read back through an implemented reader: the change is PERSISTED, not just
            // echoed.
            let (status, view) = get_managed_resource(&router, &target).await;
            assert_eq!(
                status, 200,
                "case {case}: GetManagedResource answers 200, got {status} {view}"
            );
            assert_holds(&case, &view["practiceRunConfiguration"], &expected);
            assert_eq!(
                view["practiceRunConfiguration"], answer["practiceRunConfiguration"],
                "case {case}: the read must report what the change answered, got {view}"
            );
            // The resource still HAS a configuration, so the status derived from it is
            // unchanged.
            assert_eq!(
                listed_practice_run_status(&router, &target).await.as_deref(),
                Some("ENABLED"),
                "case {case}: a resource that has a configuration reports ENABLED"
            );

            // Repeating the change asks for the same configuration, so the resource ends
            // up holding the same one.
            let (status, again) = update(&router, &target, &change).await;
            assert_eq!(
                status, 200,
                "case {case}: repeating the change answers 200, got {status} {again}"
            );
            assert_eq!(
                again["practiceRunConfiguration"], answer["practiceRunConfiguration"],
                "case {case}: repeating the change must leave the same configuration, \
                 got {again}"
            );
        }
    }
}

#[tokio::test]
async fn changing_a_configuration_a_resource_does_not_have_is_refused_409() {
    let (router, engine) = emulator();
    register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
    let mut targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
    targets.push(REGISTERED_ARN.to_string());

    // Every change is refused alike while the resource holds nothing: the refusal is
    // about the resource having no configuration, not about what the change asks for.
    for (index, (change_case, change)) in changes().into_iter().enumerate() {
        let target = targets[index % targets.len()].clone();
        let case = format!("{change_case}, {target}");
        let (status, body) = update(&router, &target, &change).await;
        assert_eq!(status, 409, "case {case}: expected 409, got {status} {body}");
        assert_eq!(
            (error_code(&body).as_str(), body["reason"].as_str()),
            (
                "ConflictException",
                Some(PRACTICE_CONFIGURATION_MISSING_REASON)
            ),
            "case {case}: expected a {PRACTICE_CONFIGURATION_MISSING_REASON} conflict, \
             got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(PRACTICE_CONFIGURATION_MISSING_MESSAGE),
            "case {case}: expected the refusal ARC words for a resource that has no \
             configuration, got {body}"
        );
        assert!(
            !body.to_string().contains(target.as_str()),
            "case {case}: the refusal must not disclose the target, got {body}"
        );

        // The refusal persisted nothing: the resource still has no configuration.
        let (_, view) = get_managed_resource(&router, &target).await;
        assert!(
            view["practiceRunConfiguration"].is_null(),
            "case {case}: a refused change must persist nothing, got {view}"
        );
    }

    // And once the resource HAS one, the same change is served: the 409 is the
    // resource's state, not the operation.
    let target = targets[0].clone();
    let held = hold(&router, &ROWS[1], &target).await;
    let (change_case, change) = changes().remove(1);
    let (status, answer) = update(&router, &target, &change).await;
    assert_eq!(
        status, 200,
        "case {change_case} once a configuration is held: expected 200, got {status} \
         {answer}"
    );
    assert_holds(
        &format!("{change_case} once a configuration is held"),
        &answer["practiceRunConfiguration"],
        &merged(&held, &change),
    );
}

#[tokio::test]
async fn a_change_for_a_target_arc_holds_nothing_for_is_refused_403() {
    let (router, engine) = emulator();
    let mut targets: Vec<(String, String)> = UNHELD_TARGETS
        .iter()
        .map(|(identifier, class)| ((*identifier).to_string(), (*class).to_string()))
        .collect();
    let (held_target, _zones) = seeded(&engine)[0].clone();
    targets.push((
        format!("{held_target}0"),
        "a near miss on an identifier the store holds".to_string(),
    ));

    // A configuration is held for a resource ARC does know, so the refusal below is the
    // authorization branch rather than the missing-configuration one.
    hold(&router, &ROWS[0], &held_target).await;

    let table = changes();
    for (index, (target, class)) in targets.iter().enumerate() {
        let (change_case, change) = &table[index % table.len()];
        let case = format!("{class}, a change that {change_case}");
        let (status, body) = update(&router, target, change).await;
        assert_eq!(
            status, 403,
            "case {case}: an unauthorized target is refused 403, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(TARGET_UNAVAILABLE_MESSAGE),
            "case {case}: expected the non-disclosing refusal message, got {body}"
        );
        assert!(
            !body.to_string().contains(target.as_str()),
            "case {case}: the refusal must not disclose the target, got {body}"
        );
    }
}

#[tokio::test]
async fn a_changed_configuration_arc_cannot_read_is_refused_400() {
    let (router, engine) = emulator();
    let (target, _zones) = seeded(&engine)[0].clone();
    let held = hold(&router, &ROWS[1], &target).await;

    // One row per way a changed configuration can fail to name what it must, as
    // `(member, value, reason, class)`. The reasons are `ValidationExceptionReason`'s
    // (`smithy_query(query="shape", shape="ValidationExceptionReason")`): each window
    // member carries the reason named for it, and a blocked date ARC cannot read is a
    // practice-run blocker it cannot read.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "blockedWindows",
            json!(["Sat:08:00-Sun:09:00"]),
            "InvalidPracticeWindows",
            "a blocked window that spans two days",
        ),
        (
            "blockedWindows",
            json!(["Sat:09:00-Sat:08:00"]),
            "InvalidPracticeWindows",
            "a blocked window that ends before it starts",
        ),
        (
            "blockedWindows",
            json!(["Sat:08:00-Sat:08:00"]),
            "InvalidPracticeWindows",
            "a blocked window of no length at all",
        ),
        (
            "allowedWindows",
            json!(["Thu:24:00-Thu:24:30"]),
            "InvalidPracticeAllowedWindow",
            "an allowed window at an hour of no day",
        ),
        (
            "allowedWindows",
            json!(["Thu:12:00-Sun:17:00"]),
            "InvalidPracticeAllowedWindow",
            "an allowed window that spans four days",
        ),
        (
            "blockedDates",
            json!(["2027-04-31"]),
            "InvalidPracticeBlocker",
            "a blocked date past the end of the month",
        ),
        (
            "blockedDates",
            json!(["2027-00-10"]),
            "InvalidPracticeBlocker",
            "a blocked date in a zeroth month",
        ),
        (
            "blockedDates",
            json!(["2027-02-29"]),
            "InvalidPracticeBlocker",
            "a leap day of a year that has none",
        ),
    ];
    for (member, value, reason, class) in rows {
        let change = members(&[(member, value)]);
        let (status, answer) = update(&router, &target, &change).await;
        assert_eq!(
            status, 400,
            "case {class}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            (error_code(&answer).as_str(), answer["reason"].as_str()),
            ("ValidationException", Some(reason)),
            "case {class}: expected a {reason} ValidationException, got {answer}"
        );
        // A refused change writes nothing: the resource still holds what it held.
        let (_, view) = get_managed_resource(&router, &target).await;
        assert_holds(class, &view["practiceRunConfiguration"], &held);
    }

    // The boundary of the same rule: a value outside a MODELLED constraint is refused 400
    // by the walk before the handler runs — one entry past the largest list the model
    // allows, a value that does not match a member's modelled pattern, an `outcomeAlarms`
    // below its smallest (which is why it can be changed but not removed), and a condition
    // type the model's `ControlConditionType` does not declare — while the largest size
    // itself is accepted (the row above that names every member at its maximum).
    for (member, outside_the_constraint, class) in [
        ("blockedDates", dates(16), "one date past the modelled maximum"),
        (
            "blockedDates",
            json!(["10-04-2027"]),
            "a date whose parts are in another order than the modelled pattern",
        ),
        (
            "blockedWindows",
            windows(16),
            "one window past the modelled maximum",
        ),
        (
            "allowedWindows",
            windows(16),
            "one window past the modelled maximum",
        ),
        (
            "blockingAlarms",
            alarms("blocking", 11),
            "one alarm past the modelled maximum",
        ),
        (
            "outcomeAlarms",
            alarms("outcome", 11),
            "one alarm past the modelled maximum",
        ),
        (
            "outcomeAlarms",
            json!([]),
            "no alarm at all, below the modelled minimum of one",
        ),
        (
            "outcomeAlarms",
            json!([{ "type": "ALARM", "alarmIdentifier": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:changed" }]),
            "a condition type the model does not declare",
        ),
    ] {
        let change = members(&[(member, outside_the_constraint)]);
        let (status, answer) = update(&router, &target, &change).await;
        assert_eq!(
            status, 400,
            "case {member} with {class}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {member} with {class}: expected ValidationException, got {answer}"
        );
        let (_, view) = get_managed_resource(&router, &target).await;
        assert_holds(class, &view["practiceRunConfiguration"], &held);
    }
}
