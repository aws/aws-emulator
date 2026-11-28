// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE ACTIONS on an Elemental Inference feed, end to end over the assembled
//! wire: `CreateScheduleAction`, `ListScheduleActions` and `DeleteScheduleAction`.
//!
//! The rules under test, each over a TABLE of inputs rather than one case:
//!
//! * an action "applies configuration changes to the feed at a time that you specify, in
//!   the same way as an UpdateFeed request", so once its scheduled time has passed the
//!   change is in the feed's own outputs and the action reports `COMPLETED`; until then it
//!   reports `PENDING`;
//! * "Elemental Inference validates the targets of the action when you create it": every
//!   operation names an output that "must exist in the feed when you create the action",
//!   and the two data-source operations may only target an event clipping output;
//! * "you can delete only an action whose status is PENDING. Deleting an action doesn't
//!   reverse changes that the action already made to the feed";
//! * the operation is `@idempotent` with no `@idempotencyToken`, so a repeat of the same
//!   action is that one request once more and the feed holds ONE action, while the same
//!   name carrying a different payload is refused;
//! * a listing that states no `maxResults` reports the documented default page size: "if
//!   you don't specify MaxResults, Elemental Inference returns a maximum of 10 results for
//!   each page" (`ListScheduleActionsRequest$maxResults`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::elementalinference_wire::{
    call, clipping_config, create_feed, cropping_config, error_code, output, poll_until, router,
};

/// A UTC instant in epoch seconds, which is how `restJson1` carries the modelled
/// `Timestamp` an action's `scheduledAt` is.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}

/// The start configuration of an action that runs once, at `scheduled_at`: fixed mode is
/// the only member the union declares.
fn starts_at(scheduled_at: i64) -> Value {
    json!({"fixedModeScheduleActionStartSettings": {"scheduledAt": scheduled_at}})
}

/// A `CreateScheduleAction` body.
fn action_body(action_name: &str, scheduled_at: i64, operations: Value) -> Value {
    json!({
        "actionName": action_name,
        "startConfiguration": starts_at(scheduled_at),
        "operations": operations
    })
}

/// The operation that enables or disables one output.
fn update_output_status(output_name: &str, status: &str) -> Value {
    json!({"updateOutputStatus": {"outputName": output_name, "status": status}})
}

/// A feed carrying one cropping output and one clipping output, so a rule about an
/// operation's TARGET can be asserted on an output that admits it and one that does not.
async fn feed_with_two_outputs(router: &MultiServiceRouter, name: &str) -> String {
    create_feed(
        router,
        name,
        json!([
            output("crop1", cropping_config()),
            output("clip1", clipping_config())
        ]),
    )
    .await
}

/// `GET /v1/feed/{id}/schedule-actions`, asserting it is served.
async fn list_actions(router: &MultiServiceRouter, feed: &str, case: &str) -> Vec<Value> {
    let (status, listed) = call(
        router,
        "GET",
        &format!("/v1/feed/{feed}/schedule-actions?maxResults=100"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListScheduleActions answers 200, got {status} with {listed}"
    );
    listed["scheduleActions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the listing reports an array, got {listed}"))
        .clone()
}

/// An action whose scheduled time has passed APPLIES its operations to the feed and
/// reports `COMPLETED`; the feed's outputs carry the change.
#[tokio::test]
async fn a_due_action_applies_its_operations_to_the_feed() {
    let router = router();

    // Each row: the operations an action carries, and the output member each one leaves
    // behind. The rows span every member of the `ScheduleActionOperation` union.
    let rows: Vec<(&str, Value, &str, Value)> = vec![
        (
            "disabling an output",
            json!([update_output_status("crop1", "DISABLED")]),
            "crop1",
            json!({"status": "DISABLED"}),
        ),
        (
            "attaching a game to an event clipping output",
            json!([{"updateClippingDataSourceConfiguration": {
                "outputName": "clip1",
                "dataSourceConfiguration": {"fixtureId": "fixture-123"}
            }}]),
            "clip1",
            json!({"dataSource": {"fixtureId": "fixture-123"}}),
        ),
        (
            "two operations in one action",
            json!([
                update_output_status("clip1", "DISABLED"),
                update_output_status("crop1", "DISABLED")
            ]),
            "crop1",
            json!({"status": "DISABLED"}),
        ),
    ];

    for (index, (label, operations, target, expected)) in rows.into_iter().enumerate() {
        let feed = feed_with_two_outputs(&router, &format!("dueaction{index}")).await;
        // A time already past is due at once, so the action runs at the next call: the
        // service stores the due instant and never sleeps for it.
        let (status, created) = call(
            &router,
            "POST",
            &format!("/v1/feed/{feed}/schedule-actions"),
            Some(action_body("roll", now_secs() - 60, operations)),
        )
        .await;
        assert_eq!(
            (status, created["status"].clone(), created["actionName"].clone()),
            (201, json!("PENDING"), json!("roll")),
            "case {label:?}: the create answers 201 with the action waiting for its time, got \
             {status} with {created}"
        );
        assert_eq!(
            created["error"], Value::Null,
            "case {label:?}: a pending action carries no error, got {created}"
        );

        // The listing materialises the move the clock already owed.
        let listed = list_actions(&router, &feed, label).await;
        assert_eq!(
            listed.len(),
            1,
            "case {label:?}: the feed holds the one action, got {listed:?}"
        );
        assert_eq!(
            (listed[0]["status"].clone(), listed[0]["error"].clone()),
            (json!("COMPLETED"), Value::Null),
            "case {label:?}: a due action reports COMPLETED and no error, got {:?}",
            listed[0]
        );

        // And the change is in the feed's own outputs.
        let (status, feed_body) = call(&router, "GET", &format!("/v1/feed/{feed}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetFeed answers 200, got {status} with {feed_body}"
        );
        let held = feed_body["outputs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: the feed reports its outputs"))
            .iter()
            .find(|held| held["name"] == json!(target))
            .unwrap_or_else(|| panic!("case {label:?}: the feed holds {target:?}"))
            .clone();
        if let Some(status) = expected.get("status") {
            assert_eq!(
                &held["status"], status,
                "case {label:?}: the action left {target:?} with the status it asked for, got \
                 {held}"
            );
        }
        if let Some(data_source) = expected.get("dataSource") {
            assert_eq!(
                &held["outputConfig"]["clipping"]["dataSourceConfiguration"], data_source,
                "case {label:?}: the action attached the game to {target:?}, got {held}"
            );
        }
    }
}

/// An action whose scheduled time is still ahead stays `PENDING`, is deletable, and the
/// feed is unchanged until it runs.
#[tokio::test]
async fn a_pending_action_changes_nothing_and_can_be_deleted() {
    let router = router();
    let feed = feed_with_two_outputs(&router, "pendingaction").await;
    // Far enough ahead that nothing this test does can make it due.
    let scheduled_at = now_secs() + 86_400;

    let (status, created) = call(
        &router,
        "POST",
        &format!("/v1/feed/{feed}/schedule-actions"),
        Some(action_body(
            "later",
            scheduled_at,
            json!([update_output_status("crop1", "DISABLED")]),
        )),
    )
    .await;
    assert_eq!(
        (status, created["status"].clone()),
        (201, json!("PENDING")),
        "the action is created pending, got {status} with {created}"
    );

    // A repeat of the SAME request is that one request once more: the feed still holds
    // one action.
    let (status, replayed) = call(
        &router,
        "POST",
        &format!("/v1/feed/{feed}/schedule-actions"),
        Some(action_body(
            "later",
            scheduled_at,
            json!([update_output_status("crop1", "DISABLED")]),
        )),
    )
    .await;
    assert_eq!(
        (status, replayed.clone()),
        (201, created.clone()),
        "a repeat answers with the action the first call stored, got {status} with {replayed}"
    );
    assert_eq!(
        list_actions(&router, &feed, "after the repeat").await.len(),
        1,
        "several identical requests have the effect of one"
    );

    // The same name carrying a different payload is a different action, which the name's
    // uniqueness within the feed refuses.
    let (status, refused) = call(
        &router,
        "POST",
        &format!("/v1/feed/{feed}/schedule-actions"),
        Some(action_body(
            "later",
            scheduled_at,
            json!([update_output_status("clip1", "DISABLED")]),
        )),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictException"),
        "the same name carrying something else is refused, got {status} with {refused}"
    );

    // The feed is untouched while the action is pending.
    let (status, held) = call(&router, "GET", &format!("/v1/feed/{feed}"), None).await;
    assert_eq!(
        (status, held["outputs"][0]["status"].clone()),
        (200, json!("ENABLED")),
        "a pending action changes nothing yet, got {status} with {held}"
    );

    // A pending action is deletable, and "a deleted action never runs".
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/v1/feed/{feed}/schedule-actions?actionName=later"),
        None,
    )
    .await;
    assert_eq!(
        (status, deleted["actionName"].clone()),
        (200, json!("later")),
        "the delete answers 200 naming the action, got {status} with {deleted}"
    );
    assert!(
        list_actions(&router, &feed, "after the delete")
            .await
            .is_empty(),
        "the deleted action is gone from the listing"
    );
    let (status, repeated) = call(
        &router,
        "DELETE",
        &format!("/v1/feed/{feed}/schedule-actions?actionName=later"),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&repeated)),
        (404, "ResourceNotFoundException"),
        "a repeat of the delete names no action of the feed, got {status} with {repeated}"
    );
}

/// An action the clock has already run is no longer deletable, and the change it made
/// stays in the feed.
#[tokio::test]
async fn a_settled_action_is_not_deletable() {
    let router = router();
    let feed = feed_with_two_outputs(&router, "settledaction").await;
    let (status, created) = call(
        &router,
        "POST",
        &format!("/v1/feed/{feed}/schedule-actions"),
        Some(action_body(
            "ran",
            now_secs() - 60,
            json!([update_output_status("crop1", "DISABLED")]),
        )),
    )
    .await;
    assert_eq!(
        status, 201,
        "the action is created, got {status} with {created}"
    );

    // Look again until the action has run: the service materialises the move on the next
    // call rather than sleeping for it.
    let (_status, listed) = poll_until(
        &router,
        &format!("/v1/feed/{feed}/schedule-actions?maxResults=100"),
        "an action whose scheduled time has passed",
        |status, body| status == 200 && body["scheduleActions"][0]["status"] == json!("COMPLETED"),
    )
    .await;
    assert_eq!(
        listed["scheduleActions"][0]["status"],
        json!("COMPLETED"),
        "the action has run, got {listed}"
    );

    let (status, refused) = call(
        &router,
        "DELETE",
        &format!("/v1/feed/{feed}/schedule-actions?actionName=ran"),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictException"),
        "only a PENDING action is deletable, got {status} with {refused}"
    );
    let (status, held) = call(&router, "GET", &format!("/v1/feed/{feed}"), None).await;
    assert_eq!(
        (status, held["outputs"][0]["status"].clone()),
        (200, json!("DISABLED")),
        "the refused delete did not reverse the change the action made, got {status} with {held}"
    );
}

/// An operation whose target the feed does not admit is refused with the modelled
/// `ValidationException`, and no action is put on the feed.
#[tokio::test]
async fn an_operation_target_the_feed_does_not_admit_is_refused() {
    let router = router();
    let feed = feed_with_two_outputs(&router, "badtarget").await;

    // The rows differ along why the target does not resolve: an output the feed does not
    // hold, and a data-source operation aimed at an output that is not an event clipping
    // output.
    let rows: Vec<(&str, Value)> = vec![
        (
            "enabling an output the feed does not hold",
            json!([update_output_status("absent1", "DISABLED")]),
        ),
        (
            "attaching a game to an output the feed does not hold",
            json!([{"updateClippingDataSourceConfiguration": {
                "outputName": "absent1",
                "dataSourceConfiguration": {"fixtureId": "fixture-123"}
            }}]),
        ),
        (
            "attaching a game to an output that is not an event clipping output",
            json!([{"updateClippingDataSourceConfiguration": {
                "outputName": "crop1",
                "dataSourceConfiguration": {"fixtureId": "fixture-123"}
            }}]),
        ),
        (
            "removing the game from an output that is not an event clipping output",
            json!([{"removeClippingDataSourceConfiguration": {"outputName": "crop1"}}]),
        ),
        (
            "one admissible operation beside one that is not",
            json!([
                update_output_status("crop1", "DISABLED"),
                update_output_status("absent1", "DISABLED")
            ]),
        ),
    ];

    for (index, (label, operations)) in rows.into_iter().enumerate() {
        let (status, refused) = call(
            &router,
            "POST",
            &format!("/v1/feed/{feed}/schedule-actions"),
            Some(action_body(
                &format!("bad{index}"),
                now_secs() + 86_400,
                operations,
            )),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {label:?}: the create is refused with the modelled 400, got {status} with \
             {refused}"
        );
        assert!(
            refused["message"].as_str().is_some_and(|message| !message.is_empty()),
            "case {label:?}: the refusal names what it could not resolve, got {refused}"
        );
    }

    assert!(
        list_actions(&router, &feed, "after the refused creates")
            .await
            .is_empty(),
        "a refused create puts no action on the feed"
    );
    let (status, held) = call(&router, "GET", &format!("/v1/feed/{feed}"), None).await;
    assert_eq!(
        (status, held["outputs"][0]["status"].clone()),
        (200, json!("ENABLED")),
        "and changes none of its outputs, got {status} with {held}"
    );
}

/// A listing that states no `maxResults` reports the DOCUMENTED default page size, and
/// hands back a `nextToken` for the rest.
#[tokio::test]
async fn a_listing_that_states_no_page_size_reports_the_documented_default() {
    let router = router();
    let feed = feed_with_two_outputs(&router, "defaultpage").await;
    // The documented default ("a maximum of 10 results for each page",
    // `ListScheduleActionsRequest$maxResults`), and enough actions that it bites.
    const DEFAULT_PAGE: usize = 10;
    let scheduled_at = now_secs() + 86_400;
    let held = DEFAULT_PAGE + 2;
    for index in 0..held {
        let (status, created) = call(
            &router,
            "POST",
            &format!("/v1/feed/{feed}/schedule-actions"),
            Some(action_body(
                &format!("action{index}"),
                scheduled_at,
                json!([update_output_status("crop1", "DISABLED")]),
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "action {index} is created, got {status} with {created}"
        );
    }

    let (status, first) = call(
        &router,
        "GET",
        &format!("/v1/feed/{feed}/schedule-actions"),
        None,
    )
    .await;
    let page = first["scheduleActions"]
        .as_array()
        .unwrap_or_else(|| panic!("the page reports an array, got {first}"))
        .clone();
    assert_eq!(
        (status, page.len()),
        (200, DEFAULT_PAGE),
        "a request that states no maxResults is answered with the documented default page \
         size, got {status} with {first}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("a page that does not end the listing carries a nextToken, got {first}")
        })
        .to_string();

    let (status, second) = call(
        &router,
        "GET",
        &format!("/v1/feed/{feed}/schedule-actions?nextToken={token}"),
        None,
    )
    .await;
    let rest = second["scheduleActions"]
        .as_array()
        .unwrap_or_else(|| panic!("the page reports an array, got {second}"))
        .clone();
    assert_eq!(
        (status, rest.len(), second["nextToken"].clone()),
        (200, held - DEFAULT_PAGE, Value::Null),
        "the token continues the listing and the last page issues none, got {status} with \
         {second}"
    );
}

/// A feed this endpoint does not hold has no schedule actions to create, list or delete.
#[tokio::test]
async fn a_feed_this_endpoint_does_not_hold_names_no_schedule_actions() {
    let router = router();
    // An id of the form a `FeedId` takes (`^[a-z0-9]{19}$`) that names no feed here.
    let absent = "aaaaaaaaaaaaaaaaaaa";
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateScheduleAction",
            "POST",
            format!("/v1/feed/{absent}/schedule-actions"),
            Some(action_body(
                "roll",
                now_secs() + 60,
                json!([update_output_status("crop1", "DISABLED")]),
            )),
        ),
        (
            "ListScheduleActions",
            "GET",
            format!("/v1/feed/{absent}/schedule-actions"),
            None,
        ),
        (
            "DeleteScheduleAction",
            "DELETE",
            format!("/v1/feed/{absent}/schedule-actions?actionName=roll"),
            None,
        ),
    ];
    for (label, method, uri, body) in rows {
        let (status, refused) = call(&router, method, &uri, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: an absent feed earns the modelled 404, got {status} with {refused}"
        );
    }
}
