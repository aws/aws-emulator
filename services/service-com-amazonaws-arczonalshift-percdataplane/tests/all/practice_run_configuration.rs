// SPDX-License-Identifier: CC0-1.0

//! The practice run configuration ARC holds for a managed resource, driven end to end
//! over the wire.
//!
//! Every case here builds the production `MultiServiceRouter`, registers this service on
//! one shared engine and dispatches real `restJson1` requests, so what is pinned is the
//! HTTP behaviour rather than a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * a practice run configuration a caller creates is PERSISTED for the resource it
//!   names, so the create's own answer and every later read — `GetManagedResource`,
//!   `ListManagedResources` — report it and the `practiceRunStatus` derived from it,
//!   whichever way the resource reached ARC (seeded, or registered by the service that
//!   owns it);
//! * a resource has at most ONE practice run configuration, so creating a second for it
//!   is refused 409 `ConflictException` and leaves the one it has untouched;
//! * the target of the call is authorized before it is resolved, so a
//!   `resourceIdentifier` ARC holds nothing for is refused 403 `AccessDeniedException`;
//! * a configuration ARC cannot read is refused 400 `ValidationException`: a window that
//!   is not one window of a single day, and a blocked date that names no day of a month
//!   (the rules the model does not express as constraints — a value outside a modelled
//!   constraint is refused at the boundary, which the same case pins).
//!
//! The rows differ along what those rules generalise over: how MANY alarms, windows and
//! dates a configuration carries (none, one, and each end of the modelled list range),
//! which optional members it carries at all, which resource it is written for, and the
//! FORM and SIZE of a target identifier.

use serde_json::{json, Value};

use super::registered_managed_resources::register;
use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded};
use service_com_amazonaws_arczonalshift_percdataplane::state::{
    PRACTICE_CONFIGURATION_EXISTS_MESSAGE, TARGET_UNAVAILABLE_MESSAGE,
};

/// A resource an owning service registers with ARC, so the rule is asserted for a
/// resource ARC did not create as well as for the ones this store was seeded with.
pub(crate) const REGISTERED_ARN: &str =
    "arn:aws:eks:us-east-1:000000000000:cluster/practice-run-eks";
pub(crate) const REGISTERED_ZONES: &[&str] = &["use1-az1", "use1-az2", "use1-az3"];

/// The ARN of an Amazon CloudWatch alarm, which is what a `ControlCondition` names an
/// alarm by (`ControlCondition$alarmIdentifier`: "The Amazon Resource Name (ARN) for an
/// Amazon CloudWatch alarm that you specify as a control condition for a practice run").
pub(crate) fn alarm_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch:us-east-1:000000000000:alarm:{name}")
}

/// `count` control conditions, each naming an alarm of its own. `CLOUDWATCH` is the only
/// condition type the model declares (`ControlConditionType`).
pub(crate) fn alarms(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "type": "CLOUDWATCH",
                    "alarmIdentifier": alarm_arn(&format!("{prefix}-{index}")),
                })
            })
            .collect(),
    )
}

/// `count` windows of a single day each, one per hour of the morning, in the
/// `DAY:HH:MM-DAY:HH:MM` format the `blockedWindows` and `allowedWindows` documentation
/// gives. The days cycle through the seven `BlockedWindow`/`AllowedWindow` pattern
/// admits, so a row of 15 windows is not 15 copies of one.
pub(crate) fn windows(count: usize) -> Value {
    // The days the model's pattern admits:
    // `^(Mon|Tue|Wed|Thu|Fri|Sat|Sun):[0-9]{2}:[0-9]{2}-...$`
    // (`smithy_query(query="shape", shape="BlockedWindows")`).
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    Value::Array(
        (0..count)
            .map(|index| {
                let day = DAYS[index % DAYS.len()];
                let hour = 1 + index % 12;
                json!(format!("{day}:{hour:02}:00-{day}:{hour:02}:30"))
            })
            .collect(),
    )
}

/// `count` calendar dates in the `YYYY-MM-DD` format `blockedDates` documents, each a
/// real day of a month.
pub(crate) fn dates(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("2026-03-{:02}", index + 1)))
            .collect(),
    )
}

/// One configuration a caller writes: how many of each list it carries, and whether it
/// carries the optional ones at all.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    outcome_alarms: usize,
    blocking_alarms: Option<usize>,
    blocked_windows: Option<usize>,
    allowed_windows: Option<usize>,
    blocked_dates: Option<usize>,
}

/// The configurations under test, spanning the modelled list ranges: `outcomeAlarms` is
/// length 1..10 and `blockingAlarms` 0..10; `blockedWindows`, `allowedWindows` and
/// `blockedDates` are each 0..15 (`smithy_query(query="operation",
/// operation="CreatePracticeRunConfiguration")`). Each end of each range is a row, and so
/// is a configuration that carries only what the model requires.
pub(crate) const ROWS: &[Row] = &[
    Row {
        case: "only the one alarm the model requires",
        outcome_alarms: 1,
        blocking_alarms: None,
        blocked_windows: None,
        allowed_windows: None,
        blocked_dates: None,
    },
    Row {
        case: "an outcome and a blocking alarm, one blocked window and one blocked date",
        outcome_alarms: 1,
        blocking_alarms: Some(1),
        blocked_windows: Some(1),
        allowed_windows: None,
        blocked_dates: Some(1),
    },
    Row {
        case: "allowed windows only, with no blockers at all",
        outcome_alarms: 2,
        blocking_alarms: Some(0),
        blocked_windows: Some(0),
        allowed_windows: Some(3),
        blocked_dates: Some(0),
    },
    Row {
        case: "every list at the largest size the model allows",
        outcome_alarms: 10,
        blocking_alarms: Some(10),
        blocked_windows: Some(15),
        allowed_windows: Some(15),
        blocked_dates: Some(15),
    },
];

impl Row {
    /// The `practiceRunConfiguration` members this row asks ARC to hold, as the request
    /// and the answer both carry them.
    pub(crate) fn configuration(&self) -> serde_json::Map<String, Value> {
        let mut members = serde_json::Map::new();
        members.insert("outcomeAlarms".to_string(), alarms("outcome", self.outcome_alarms));
        if let Some(count) = self.blocking_alarms {
            members.insert("blockingAlarms".to_string(), alarms("blocking", count));
        }
        if let Some(count) = self.blocked_windows {
            members.insert("blockedWindows".to_string(), windows(count));
        }
        if let Some(count) = self.allowed_windows {
            members.insert("allowedWindows".to_string(), windows(count));
        }
        if let Some(count) = self.blocked_dates {
            members.insert("blockedDates".to_string(), dates(count));
        }
        members
    }

    /// The request body that writes this row's configuration for `resource`.
    pub(crate) fn create_body(&self, resource: &str) -> String {
        let mut body = self.configuration();
        body.insert("resourceIdentifier".to_string(), json!(resource));
        Value::Object(body).to_string()
    }
}

/// `CreatePracticeRunConfiguration` over the wire.
pub(crate) async fn create(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: &str,
) -> (u16, Value) {
    call(router, "POST", "/configuration", body).await
}

/// `GetManagedResource` over the wire.
pub(crate) async fn get_managed_resource(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/managedresources/{}", encoded(arn)),
        "",
    )
    .await
}

/// Write `row`'s configuration for `resource` through `CreatePracticeRunConfiguration`, as
/// the state a case that acts on a held configuration starts from.
pub(crate) async fn hold(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &Row,
    resource: &str,
) -> serde_json::Map<String, Value> {
    let (status, created) = create(router, &row.create_body(resource)).await;
    assert_eq!(
        status, 201,
        "case {}: the configuration under test is created first, got {status} {created}",
        row.case
    );
    row.configuration()
}

/// The `practiceRunStatus` `ListManagedResources` reports for one resource, walking the
/// pages it hands out.
pub(crate) async fn listed_practice_run_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> Option<String> {
    let mut query = String::new();
    loop {
        let (status, body) = call(router, "GET", &format!("/managedresources{query}"), "").await;
        assert_eq!(
            status, 200,
            "ListManagedResources{query}: expected 200, got {status} {body}"
        );
        for item in body["items"].as_array().cloned().unwrap_or_default() {
            if item["arn"].as_str() == Some(arn) {
                return item["practiceRunStatus"].as_str().map(str::to_string);
            }
        }
        match body["nextToken"].as_str() {
            Some(token) => query = format!("?nextToken={}", encoded(token)),
            None => return None,
        }
    }
}

/// Every member the request asked ARC to hold is in `actual`, and a member it left out is
/// either absent or the empty list the model defaults it to.
pub(crate) fn assert_holds(case: &str, actual: &Value, expected: &serde_json::Map<String, Value>) {
    for member in [
        "outcomeAlarms",
        "blockingAlarms",
        "blockedWindows",
        "allowedWindows",
        "blockedDates",
    ] {
        match expected.get(member) {
            Some(value) => assert_eq!(
                &actual[member], value,
                "case {case}: expected {member} {value}, got {}",
                actual[member]
            ),
            None => assert!(
                actual[member].is_null() || actual[member] == json!([]),
                "case {case}: {member} was not asked for, got {}",
                actual[member]
            ),
        }
    }
}

#[tokio::test]
async fn a_created_practice_run_configuration_is_persisted_and_every_later_read_reports_it() {
    for row in ROWS {
        // One emulator per row, so each row's reads are about its own store, and the
        // targets span the resources ARC was seeded with and one its owner registered.
        let (router, engine) = emulator();
        register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
        let mut targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
        targets.push(REGISTERED_ARN.to_string());

        for target in &targets {
            let case = format!("{}: {target}", row.case);
            let expected = row.configuration();

            // Before the call, the resource carries no configuration and the status
            // derived from it is `DISABLED`: what follows is computed from the write,
            // not reported unconditionally.
            let (status, before) = get_managed_resource(&router, target).await;
            assert_eq!(
                status, 200,
                "case {case}: GetManagedResource answers 200, got {status} {before}"
            );
            assert!(
                before["practiceRunConfiguration"].is_null(),
                "case {case}: no configuration has been written yet, got {before}"
            );
            assert_eq!(
                listed_practice_run_status(&router, target).await.as_deref(),
                Some("DISABLED"),
                "case {case}: a resource with no configuration reports DISABLED"
            );

            let (status, created) = create(&router, &row.create_body(target)).await;
            assert_eq!(
                status, 201,
                "case {case}: CreatePracticeRunConfiguration answers 201, got {status} {created}"
            );
            assert_eq!(
                (created["arn"].as_str(), created["zonalAutoshiftStatus"].as_str()),
                (Some(target.as_str()), Some("DISABLED")),
                "case {case}: expected the resource's ARN and its current autoshift \
                 status, got {created}"
            );
            assert!(
                created["name"].as_str().is_some_and(|name| !name.is_empty()),
                "case {case}: the resource's name is required, got {created}"
            );
            assert_holds(&case, &created["practiceRunConfiguration"], &expected);

            // Read back through an implemented reader: the configuration is PERSISTED,
            // not just echoed.
            let (status, view) = get_managed_resource(&router, target).await;
            assert_eq!(
                status, 200,
                "case {case}: GetManagedResource answers 200, got {status} {view}"
            );
            assert_holds(&case, &view["practiceRunConfiguration"], &expected);
            assert_eq!(
                view["practiceRunConfiguration"], created["practiceRunConfiguration"],
                "case {case}: the read must report what the create answered, got {view}"
            );

            // And the status derived from it: a resource that has a configuration
            // reports `ENABLED`.
            assert_eq!(
                listed_practice_run_status(&router, target).await.as_deref(),
                Some("ENABLED"),
                "case {case}: expected ListManagedResources to report ENABLED"
            );
        }
    }
}

#[tokio::test]
async fn creating_a_second_configuration_for_a_resource_is_refused_409() {
    let (router, engine) = emulator();
    let targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();

    for (index, target) in targets.iter().enumerate() {
        let held = &ROWS[index % ROWS.len()];
        let (status, created) = create(&router, &held.create_body(target)).await;
        assert_eq!(
            status, 201,
            "case {target}: the first create answers 201, got {status} {created}"
        );

        // Every row is refused alike, whatever configuration the second call asks for:
        // the refusal is about the resource already having one.
        for row in ROWS {
            let case = format!("{target}: a second create, {}", row.case);
            let (status, body) = create(&router, &row.create_body(target)).await;
            assert_eq!(
                status, 409,
                "case {case}: expected 409, got {status} {body}"
            );
            assert_eq!(
                (error_code(&body).as_str(), body["reason"].as_str()),
                ("ConflictException", Some("PracticeConfigurationAlreadyExists")),
                "case {case}: expected a PracticeConfigurationAlreadyExists conflict, \
                 got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(PRACTICE_CONFIGURATION_EXISTS_MESSAGE),
                "case {case}: expected the refusal ARC words for a resource that already \
                 has a configuration, got {body}"
            );
        }

        // The configuration the resource holds is the one the FIRST call wrote.
        let (_, view) = get_managed_resource(&router, target).await;
        assert_eq!(
            view["practiceRunConfiguration"], created["practiceRunConfiguration"],
            "case {target}: a refused create must not change what the resource holds, \
             got {view}"
        );
    }
}

/// `resourceIdentifier`s no managed resource carries, as `(identifier, class)`: ARN-shaped
/// and not, each end of the modelled `ResourceIdentifier` length range (8..1024), and a
/// near miss on one the store holds.
pub(crate) const UNHELD_TARGETS: &[(&str, &str)] = &[
    (
        "arn:aws:elasticloadbalancing:us-east-1:111111111111:loadbalancer/app/absent/73e2d6bc24d8a067",
        "a load balancer ARN in another account",
    ),
    (
        "arn:aws:eks:us-east-1:000000000000:cluster/never-registered",
        "an EKS cluster ARN ARC has no record of",
    ),
    ("abcdefgh", "the shortest identifier the model accepts"),
    ("zonal-shift-alb", "a resource name rather than its ARN"),
];

#[tokio::test]
async fn a_configuration_for_a_target_arc_holds_nothing_for_is_refused_403() {
    let (router, engine) = emulator();
    let mut targets: Vec<(String, String)> = UNHELD_TARGETS
        .iter()
        .map(|(identifier, class)| ((*identifier).to_string(), (*class).to_string()))
        .collect();
    let (held, _zones) = seeded(&engine)[0].clone();
    targets.push((
        format!("{held}0"),
        "a near miss on an identifier the store holds".to_string(),
    ));

    for (index, (target, class)) in targets.iter().enumerate() {
        // The configuration carried with the call is varied across the rows as well, so
        // the branch is shown to be taken whatever else the request asks for.
        let row = &ROWS[index % ROWS.len()];
        let case = format!("{class}, {}", row.case);
        let (status, body) = create(&router, &row.create_body(target)).await;
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
async fn a_configuration_arc_cannot_read_is_refused_400() {
    let (router, engine) = emulator();
    let (target, _zones) = seeded(&engine)[0].clone();
    // One row per way a configuration can fail to name what it must, as
    // `(member, value, reason, class)`. The reasons are `ValidationExceptionReason`'s
    // (`smithy_query(query="shape", shape="ValidationExceptionReason")`): each window
    // member carries the reason named for it, and a blocked date ARC cannot read is a
    // practice-run blocker it cannot read.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "blockedWindows",
            json!(["Mon:18:30-Tue:19:30"]),
            "InvalidPracticeWindows",
            "a blocked window that spans two days",
        ),
        (
            "blockedWindows",
            json!(["Mon:19:30-Mon:18:30"]),
            "InvalidPracticeWindows",
            "a blocked window that ends before it starts",
        ),
        (
            "blockedWindows",
            json!(["Mon:18:30-Mon:18:30"]),
            "InvalidPracticeWindows",
            "a blocked window of no length at all",
        ),
        (
            "blockedWindows",
            json!(["Mon:25:00-Mon:26:00"]),
            "InvalidPracticeWindows",
            "a blocked window at an hour of no day",
        ),
        (
            "allowedWindows",
            json!(["Wed:12:00-Fri:17:00"]),
            "InvalidPracticeAllowedWindow",
            "an allowed window that spans three days",
        ),
        (
            "allowedWindows",
            json!(["Wed:12:70-Wed:17:00"]),
            "InvalidPracticeAllowedWindow",
            "an allowed window at a minute of no hour",
        ),
        (
            "blockedDates",
            json!(["2026-02-30"]),
            "InvalidPracticeBlocker",
            "a blocked date past the end of the month",
        ),
        (
            "blockedDates",
            json!(["2026-13-01"]),
            "InvalidPracticeBlocker",
            "a blocked date in a thirteenth month",
        ),
        (
            "blockedDates",
            json!(["2025-02-29"]),
            "InvalidPracticeBlocker",
            "a leap day of a year that has none",
        ),
    ];
    for (member, value, reason, class) in rows {
        let mut body = ROWS[0].configuration();
        body.insert("resourceIdentifier".to_string(), json!(target));
        body.insert(member.to_string(), value.clone());
        let (status, answer) = create(&router, &Value::Object(body).to_string()).await;
        assert_eq!(
            status, 400,
            "case {class}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            (error_code(&answer).as_str(), answer["reason"].as_str()),
            ("ValidationException", Some(reason)),
            "case {class}: expected a {reason} ValidationException, got {answer}"
        );
        // A refused call writes nothing: the resource still has no configuration.
        let (_, view) = get_managed_resource(&router, &target).await;
        assert!(
            view["practiceRunConfiguration"].is_null(),
            "case {class}: a refused create must persist nothing, got {view}"
        );
    }

    // The boundary of the same rule: a value outside a MODELLED constraint is refused 400
    // by the walk before the handler runs — a list one past the largest size the model
    // allows, a list below its smallest, and an alarm whose condition type is not one the
    // model's `ControlConditionType` declares — while the largest size itself is accepted
    // (the row above that carries every list at its maximum).
    for (member, outside_the_constraint, class) in [
        ("blockedDates", dates(16), "one date past the modelled maximum"),
        ("blockedWindows", windows(16), "one window past the modelled maximum"),
        ("allowedWindows", windows(16), "one window past the modelled maximum"),
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
            json!([{ "type": "ROUTE53", "alarmIdentifier": alarm_arn("outcome-0") }]),
            "a condition type the model does not declare",
        ),
        (
            "outcomeAlarms",
            json!([{ "type": "cloudwatch", "alarmIdentifier": alarm_arn("outcome-0") }]),
            "a condition type that differs from the declared one in case",
        ),
    ] {
        let mut body = ROWS[0].configuration();
        body.insert("resourceIdentifier".to_string(), json!(target));
        body.insert(member.to_string(), outside_the_constraint);
        let (status, answer) = create(&router, &Value::Object(body).to_string()).await;
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
        assert!(
            view["practiceRunConfiguration"].is_null(),
            "case {member} with {class}: a refused create must persist nothing, got {view}"
        );
    }
}
