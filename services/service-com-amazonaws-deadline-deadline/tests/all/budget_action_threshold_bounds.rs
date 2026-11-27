// SPDX-License-Identifier: CC0-1.0

//! A budget action's `thresholdPercentage`, the one modeled bound that sits INSIDE a list
//! element of a budget request body.
//!
//! RULE: a modeled numeric bound a budget request body violates — wherever in that body
//! the member sits, including inside a list element — is refused with HTTP 400
//! `ValidationException` whose message is this service's body-validator finding list (the
//! bound as the model declares it, the value the request sent, and the fixed trailing
//! `float` clause), never the framework's generic constraint sentence; each finding names
//! the member by the path the validator spells for it, and the refused request writes
//! nothing. The value AT either bound is accepted.
//!
//! The bound comes from the model: `BudgetActionToAdd$thresholdPercentage` and
//! `BudgetActionToRemove$thresholdPercentage` both target `ThresholdPercentage`,
//! `range {min 0, max 100}` (`smithy_query(query="shape",
//! shape="BudgetActionToAdd,BudgetActionToRemove,ThresholdPercentage")`).
//!
//! The rows span what the rule generalises over: which list member of which operation
//! carries the offending action (`CreateBudget$actions`, `UpdateBudget$actionsToAdd`,
//! `UpdateBudget$actionsToRemove`), which side of the bound was crossed and how far, WHERE
//! in the list the offending element sits, and how many elements are at fault at once.
//!
//! The last test also reads the budget's schedule back: `FixedBudgetSchedule`'s two
//! `@timestampFormat("date-time")` members are declared from the model like every other
//! shape of this crate, and the wire form they take comes from the service-wide protocol
//! seam (`src/service.rs`'s `override_protocol`), not from a per-shape schema — so a budget
//! created with RFC 3339 strings reports them as RFC 3339 strings. The rule itself is owned
//! by `tests/all/date_time_wire_form.rs`; the assertion here keeps it beside the budget
//! shapes this suite declares.
//!
//! The harness (`Emulator`) is the one every other suite in this target drives the service
//! with, so each call goes through routing, deserialization, boundary constraint
//! validation, the handler and serialization.

use serde_json::{json, Value};

use crate::farm_queue_job::{error_code, id_of, string_at, Emulator};

/// `POST`/`GET` collection path of the budget resource, from
/// `smithy_query(query="operation", operation="CreateBudget")`.
fn budgets_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/budgets")
}

/// Single-budget path, from `smithy_query(query="operation", operation="GetBudget")`.
fn budget_path(farm_id: &str, budget_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/budgets/{budget_id}")
}

/// One legal budget action at `threshold`
/// (`smithy_query(query="shape", shape="BudgetActionType")` for the type).
fn action(threshold: f64) -> Value {
    json!({"type": "STOP_SCHEDULING_AND_COMPLETE_TASKS", "thresholdPercentage": threshold})
}

/// The `startTime` every create of this suite sends, as an SDK serializes a `date-time`.
const SCHEDULE_START: &str = "2026-01-01T00:00:00Z";

/// The `endTime` every create of this suite sends. Whole seconds, so the string the read
/// reports is the string the request sent: the `date-time` writer trims a trailing zero
/// from a fractional second, a form `tests/all/date_time_wire_form.rs` covers by comparing
/// the INSTANT instead.
const SCHEDULE_END: &str = "2026-01-02T00:00:00Z";

/// A `CreateBudget` body carrying `actions`, with every other member the model requires.
fn create_body(display_name: &str, queue_id: &str, actions: Value) -> Value {
    json!({
        "displayName": display_name,
        "description": "a budget",
        "usageTrackingResource": {"queueId": queue_id},
        "approximateDollarLimit": 10.0,
        "actions": actions,
        "schedule": {"fixed": {"startTime": SCHEDULE_START, "endTime": SCHEDULE_END}},
    })
}

/// The `message` member of a refusal body.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a message member in {body}"))
        .to_string()
}

/// The members a refusal's `fieldList` names, in order.
fn field_names(body: &Value) -> Vec<String> {
    body.get("fieldList")
        .and_then(Value::as_array)
        .map(|fields| fields.iter().map(|field| string_at(field, "name")).collect())
        .unwrap_or_default()
}

/// The `thresholdPercentage` of every action a budget read reports, in order.
fn stored_thresholds(body: &Value) -> Vec<f64> {
    body.get("actions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an actions array in {body}"))
        .iter()
        .map(|a| {
            a.get("thresholdPercentage")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a thresholdPercentage in {a}"))
        })
        .collect()
}

/// A farm with one queue to track usage against.
async fn farm_with_queue(emu: &Emulator, label: &str) -> (String, String) {
    let farm_id = emu.farm(&format!("{label}-farm")).await;
    let queue_id = emu.queue(&farm_id, &format!("{label}-queue")).await;
    (farm_id, queue_id)
}

/// The whole message for a refusal stating `findings`: the bracketed list plus the fixed
/// clause this service's validator appends to every budget-body finding list.
fn expected_message(findings: &[String]) -> String {
    let mut stated = findings.to_vec();
    stated.push("format attribute \"float\" not supported".to_string());
    format!("[{}]", stated.join(", "))
}

/// The finding for a threshold above the modeled maximum, at `path`.
fn above_max(path: &str, found: &str) -> String {
    format!(
        "{path}: numeric instance is greater than the required maximum \
         (maximum: 100, found: {found})"
    )
}

/// The finding for a threshold below the modeled minimum, at `path`.
fn below_min(path: &str, found: &str) -> String {
    format!(
        "{path}: numeric instance is lower than the required minimum \
         (minimum: 0, found: {found})"
    )
}

/// The sentence the FRAMEWORK states for a bound it enforces at the boundary. This
/// service's budget bodies must never be refused in these words: it is the wording a
/// re-rendered `BudgetActionToAdd` without this crate's `skip_validation` brings back.
const FRAMEWORK_CONSTRAINT_SENTENCE: &str = "failed to satisfy constraint";

/// A refusal case: the member path and the detail of each finding expected, as
/// `"<path>: <detail>"` pairs split back apart by [`split_finding`].
type Findings = Vec<String>;

/// Split a `"<path>: <detail>"` row into the member path and the sentence about it.
fn split_finding(row: &str) -> (String, String) {
    let (path, detail) = row
        .split_once(": ")
        .unwrap_or_else(|| panic!("a finding row names its member: {row}"));
    (path.to_string(), detail.to_string())
}

/// Assert one refusal: the status, the code, the reason, the finding list in the message,
/// the members named in `fieldList`, and that the framework's own sentence is absent.
fn assert_refused(case: &str, status: u16, body: &Value, findings: &Findings) {
    let split: Vec<(String, String)> = findings.iter().map(|row| split_finding(row)).collect();
    assert_eq!(
        status, 400,
        "case {case:?}: expected the refusal to answer 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected code ValidationException, got {body}"
    );
    assert_eq!(
        string_at(body, "reason"),
        "FIELD_VALIDATION_FAILED",
        "case {case:?}: expected the field-validation reason, got {body}"
    );
    let details: Vec<String> = split.iter().map(|(_, detail)| detail.clone()).collect();
    let expected = expected_message(&details);
    assert_eq!(
        message_of(body),
        expected,
        "case {case:?}: expected message {expected:?}, got {:?}",
        message_of(body)
    );
    assert!(
        !message_of(body).contains(FRAMEWORK_CONSTRAINT_SENTENCE),
        "case {case:?}: expected this service's finding list, got the framework's \
         sentence: {:?}",
        message_of(body)
    );
    let expected_fields: Vec<String> = split.iter().map(|(path, _)| path.clone()).collect();
    assert_eq!(
        field_names(body),
        expected_fields,
        "case {case:?}: expected fieldList to name {expected_fields:?}, got {body}"
    );
}

/// RULE, on the create path: an action threshold outside the modeled range is refused in
/// this service's own words, with the element's own path, and no budget is made.
#[tokio::test]
async fn a_create_action_threshold_outside_the_modeled_range_is_refused_in_the_services_words() {
    // case, actions sent, the findings expected as "<path>: <detail>"
    let cases: [(&str, Value, Findings); 5] = [
        (
            "the only action past the maximum",
            json!([action(150.0)]),
            vec![above_max("actions.1.member.thresholdPercentage", "150")],
        ),
        (
            "just past the maximum",
            json!([action(100.5)]),
            vec![above_max("actions.1.member.thresholdPercentage", "100.5")],
        ),
        (
            "the third action below the minimum",
            json!([action(10.0), action(20.0), action(-1.0)]),
            vec![below_min("actions.3.member.thresholdPercentage", "-1")],
        ),
        (
            "just below the minimum",
            json!([action(-0.5)]),
            vec![below_min("actions.1.member.thresholdPercentage", "-0.5")],
        ),
        (
            "two actions at fault at once",
            json!([action(-20.0), action(50.0), action(120.0)]),
            vec![
                below_min("actions.1.member.thresholdPercentage", "-20"),
                above_max("actions.3.member.thresholdPercentage", "120"),
            ],
        ),
    ];

    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "create-threshold").await;

    for (case, actions, findings) in cases {
        let (status, body) = emu
            .post(&budgets_path(&farm_id), create_body(case, &queue_id, actions))
            .await;
        assert_refused(case, status, &body, &findings);

        let (status, listed) = emu.get(&budgets_path(&farm_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBudgets to answer 200, got {status} with {listed}"
        );
        let named = listed
            .get("budgets")
            .and_then(Value::as_array)
            .is_some_and(|budgets| {
                budgets
                    .iter()
                    .any(|b| b.get("displayName").and_then(Value::as_str) == Some(case))
            });
        assert!(
            !named,
            "case {case:?}: a refused CreateBudget must leave no budget behind, got {listed}"
        );
    }
}

/// RULE, on the update path: the same bound, the same wording, for both action lists the
/// update body carries — and the refused update changes nothing.
#[tokio::test]
async fn an_update_action_threshold_outside_the_modeled_range_is_refused_in_the_services_words() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "update-threshold").await;
    let (status, created) = emu
        .post(
            &budgets_path(&farm_id),
            create_body("before", &queue_id, json!([action(80.0)])),
        )
        .await;
    assert_eq!(status, 200, "expected the budget to be created, got {created}");
    let budget_id = id_of(&created, "budgetId");
    let path = budget_path(&farm_id, &budget_id);

    // case, update body, the findings expected as "<path>: <detail>"
    let cases: [(&str, Value, Findings); 4] = [
        (
            "an added action past the maximum",
            json!({"displayName": "after", "actionsToAdd": [action(101.0)]}),
            vec![above_max("actionsToAdd.1.member.thresholdPercentage", "101")],
        ),
        (
            "the second added action below the minimum",
            json!({"displayName": "after", "actionsToAdd": [action(40.0), action(-3.0)]}),
            vec![below_min("actionsToAdd.2.member.thresholdPercentage", "-3")],
        ),
        (
            "a removed action below the minimum",
            json!({"displayName": "after", "actionsToRemove": [action(-0.25)]}),
            vec![below_min("actionsToRemove.1.member.thresholdPercentage", "-0.25")],
        ),
        (
            "both lists at fault at once",
            json!({
                "displayName": "after",
                "actionsToAdd": [action(200.0)],
                "actionsToRemove": [action(300.0)],
            }),
            vec![
                above_max("actionsToAdd.1.member.thresholdPercentage", "200"),
                above_max("actionsToRemove.1.member.thresholdPercentage", "300"),
            ],
        ),
    ];

    for (case, body, findings) in cases {
        let (status, refused) = emu.call("PATCH", &path, Some(body), None).await;
        assert_refused(case, status, &refused, &findings);

        let (_, read) = emu.get(&path).await;
        assert_eq!(
            string_at(&read, "displayName"),
            "before",
            "case {case:?}: a refused UpdateBudget must change nothing, got {read}"
        );
        assert_eq!(
            stored_thresholds(&read),
            vec![80.0],
            "case {case:?}: a refused UpdateBudget must leave the stored actions alone, \
             got {read}"
        );
    }
}

/// The `fixed` schedule of a budget read back, as `{startTime, endTime}`.
fn fixed_schedule_of(body: &Value) -> &Value {
    body.get("schedule")
        .and_then(|schedule| schedule.get("fixed"))
        .unwrap_or_else(|| panic!("expected schedule.fixed in {body}"))
}

/// RULE's other side: a threshold AT either modeled bound, or anywhere between them, is
/// accepted — on the create path and on the update path alike — and reads back as sent.
///
/// The schedule is read back with it: the two `date-time` members the budget was created
/// with come back as the RFC 3339 STRINGS the request sent, never the epoch-seconds number
/// the restJson1 protocol defaults to.
#[tokio::test]
async fn an_action_threshold_at_or_inside_the_modeled_range_is_accepted() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "in-range").await;

    // case, the thresholds a create sends
    let cases: [(&str, Vec<f64>); 4] = [
        ("at the minimum", vec![0.0]),
        ("at the maximum", vec![100.0]),
        ("just inside both bounds", vec![0.5, 99.5]),
        ("every bound at once", vec![0.0, 50.0, 100.0]),
    ];

    for (case, thresholds) in cases {
        let actions: Value = thresholds.iter().copied().map(action).collect();
        let (status, body) = emu
            .post(&budgets_path(&farm_id), create_body(case, &queue_id, actions))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} with {body}"
        );
        let budget_id = id_of(&body, "budgetId");
        let path = budget_path(&farm_id, &budget_id);
        let (status, read) = emu.get(&path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBudget to answer 200, got {status} with {read}"
        );
        assert_eq!(
            stored_thresholds(&read),
            thresholds,
            "case {case:?}: expected the thresholds to read back as sent, got {read}"
        );
        let schedule = fixed_schedule_of(&read);
        for (member, sent) in [("startTime", SCHEDULE_START), ("endTime", SCHEDULE_END)] {
            let reported = schedule.get(member).unwrap_or_else(|| {
                panic!("case {case:?}: expected a {member} in {schedule}")
            });
            assert_eq!(
                reported.as_str(),
                Some(sent),
                "case {case:?}: {member} is modeled @timestampFormat(\"date-time\"), so it \
                 must read back as the RFC 3339 string {sent:?} and never as the \
                 protocol-default epoch number, got {reported}"
            );
        }

        // The same values are accepted on the update path, and the budget then holds both
        // the created and the added actions.
        let added = thresholds.iter().copied().map(action).collect::<Value>();
        let (status, updated) = emu
            .call("PATCH", &path, Some(json!({"actionsToAdd": added})), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} with {updated}"
        );
        let (_, read) = emu.get(&path).await;
        let expected: Vec<f64> = thresholds.iter().chain(thresholds.iter()).copied().collect();
        assert_eq!(
            stored_thresholds(&read),
            expected,
            "case {case:?}: expected the added thresholds beside the created ones, got {read}"
        );
    }
}

/// RULE: the bound is the MEMBER's, so every other constraint on a budget action is still
/// enforced — an action `description` past its modeled `@length` maximum is refused, and a
/// threshold inside the range is not what makes a request legal on its own.
///
/// `BudgetActionToAdd$description` targets `Description`, `length {min 0, max 100}`
/// (`smithy_query(query="shape", shape="BudgetActionToAdd")`).
#[tokio::test]
async fn an_action_member_other_than_the_threshold_is_still_bounded() {
    const MAX_DESCRIPTION: usize = 100;
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "description").await;

    // case, how long the description is, whether the request is served
    let cases: [(&str, usize, bool); 3] = [
        ("at the description maximum", MAX_DESCRIPTION, true),
        ("one past the description maximum", MAX_DESCRIPTION + 1, false),
        ("far past the description maximum", MAX_DESCRIPTION * 3, false),
    ];

    for (case, length, served) in cases {
        let mut one = action(50.0);
        one["description"] = json!("d".repeat(length));
        let (status, body) = emu
            .post(&budgets_path(&farm_id), create_body(case, &queue_id, json!([one])))
            .await;
        let expected = if served { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected CreateBudget to answer {expected}, got {status} \
             with {body}"
        );
    }
}
