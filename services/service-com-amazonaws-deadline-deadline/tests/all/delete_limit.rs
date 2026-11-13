// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteLimit`.
//!
//! The rules under test:
//!
//! * a delete removes the limit from its farm, so every later read of it answers the
//!   modeled 404 `ResourceNotFoundException` and the farm's listing no longer carries it;
//!   the operation is `@idempotent` and models no not-found error, so a repeat of the
//!   delete — and a delete naming a limit or a farm this emulator does not hold — is the
//!   same empty success rather than a refusal;
//! * a queue-limit association is a live reference to the limit, so while one holds it the
//!   delete is refused with the only client-input error the operation models (HTTP 400
//!   `ValidationException`) and the limit stays readable; once nothing is associated with
//!   it the same delete succeeds.
//!
//! The harness (`Emulator`) and the limit helper are the ones the other suites drive the
//! service with, so these tests go through routing, deserialization, constraint
//! validation, the handler and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::limit::create_limit;

/// Collection path of the limit resource, from
/// `smithy_query(query="operation", operation="ListLimits")`:
/// `GET /2023-10-12/farms/{farmId}/limits`.
fn limits_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/limits")
}

/// Single-limit path, from `smithy_query(query="operation", operation="DeleteLimit")`:
/// `DELETE /2023-10-12/farms/{farmId}/limits/{limitId}`, the same path `GetLimit` reads.
fn limit_path(farm_id: &str, limit_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/limits/{limit_id}")
}

/// A `CreateLimit` body carrying every member the model requires.
fn create_body(display_name: &str, amount_requirement_name: &str, max_count: i64) -> Value {
    json!({
        "displayName": display_name,
        "amountRequirementName": amount_requirement_name,
        "maxCount": max_count,
    })
}

async fn delete_limit(emulator: &Emulator, farm_id: &str, limit_id: &str) -> (u16, Value) {
    emulator
        .call("DELETE", &limit_path(farm_id, limit_id), None, None)
        .await
}

/// Associate `queue_id` with `limit_id` under `farm_id`, failing the test unless the
/// create was accepted. `PUT /2023-10-12/farms/{farmId}/queue-limit-associations`, from
/// `smithy_query(query="operation", operation="CreateQueueLimitAssociation")`.
async fn associate(emulator: &Emulator, farm_id: &str, queue_id: &str, limit_id: &str) {
    let (status, body) = emulator
        .call(
            "PUT",
            &format!("/2023-10-12/farms/{farm_id}/queue-limit-associations"),
            Some(json!({"queueId": queue_id, "limitId": limit_id})),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateQueueLimitAssociation {queue_id}/{limit_id}: expected 200, got {status} with \
         {body}"
    );
}

/// The ids the farm's listing reports.
async fn listed_limit_ids(emulator: &Emulator, farm_id: &str) -> Vec<String> {
    let (status, body) = emulator.get(&limits_path(farm_id)).await;
    assert_eq!(
        status, 200,
        "ListLimits: expected 200, got {status} with {body}"
    );
    body["limits"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a limits list in {body}"))
        .iter()
        .map(|item| string_at(item, "limitId"))
        .collect()
}

#[tokio::test]
async fn deleting_a_limit_removes_it_and_repeats_as_an_empty_success() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Deleting limit farm").await;

    // The rule: the delete removes the limit from its farm, whatever the limit holds. The
    // rows differ along every member the create accepts — `maxCount` at each published
    // bound of its modeled range (`MaxCount, range -1..2147483647`) and inside it, an
    // empty and a maximum-length `amountRequirementName` (`len ..1024`), and a description
    // absent or at its length maximum (`Description, len 0..100`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "maxCount at the modeled minimum, no description member",
            create_body("Unrestricted", "amount.maya", -1),
        ),
        (
            "maxCount just above the forbidden 0",
            json!({
                "displayName": "One license",
                "amountRequirementName": "amount.nuke",
                "maxCount": 1,
                "description": "d".repeat(100),
            }),
        ),
        (
            "maxCount at the modeled maximum",
            create_body("All the licenses", "amount.houdini", 2_147_483_647),
        ),
        (
            "an empty amountRequirementName",
            create_body("Empty requirement name", "", 7),
        ),
        (
            "an amountRequirementName at its modeled length maximum",
            create_body("Long requirement name", &"r".repeat(1024), 12),
        ),
    ];

    for (case, request) in &cases {
        let limit_id = create_limit(&emulator, &farm_id, request.clone()).await;
        assert!(
            listed_limit_ids(&emulator, &farm_id)
                .await
                .contains(&limit_id),
            "case {case:?}: expected the created limit {limit_id} to be listed before the \
             delete"
        );

        let (status, body) = delete_limit(&emulator, &farm_id, &limit_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteLimit to answer 200, got {status} with {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
            "case {case:?}: expected an empty DeleteLimit response, got {body}"
        );

        // The read of the deleted limit is the modeled 404, worded as this service words a
        // missing resource and naming the limit the request asked for.
        let (status, read) = emulator.get(&limit_path(&farm_id, &limit_id)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted limit to be gone, got {status} with {read}"
        );
        assert_eq!(
            (
                error_code(&read),
                string_at(&read, "message"),
                string_at(&read, "resourceType"),
                string_at(&read, "resourceId"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                format!("Resource of type limit with id {limit_id} does not exist."),
                "limit".to_string(),
                limit_id.clone(),
            ),
            "case {case:?}: expected the 404 to name the deleted limit, got {read}"
        );
        // A second read of a deleted limit is the same 404: the delete did not leave it
        // half-removed.
        let (status, again) = emulator.get(&limit_path(&farm_id, &limit_id)).await;
        assert_eq!(
            (status, again),
            (404, read),
            "case {case:?}: a second read of the deleted limit answered differently"
        );

        assert!(
            !listed_limit_ids(&emulator, &farm_id)
                .await
                .contains(&limit_id),
            "case {case:?}: expected the deleted limit {limit_id} to be gone from the listing"
        );

        // `@idempotent`, and the model declares no not-found error: a repeat of the delete
        // has nothing left to remove and answers the same empty success.
        let (status, body) = delete_limit(&emulator, &farm_id, &limit_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated delete to answer 200, got {status} with {body}"
        );
    }

    assert!(
        listed_limit_ids(&emulator, &farm_id).await.is_empty(),
        "expected the farm to hold no limit once every one of them was deleted"
    );
}

#[tokio::test]
async fn deleting_a_limit_this_emulator_does_not_hold_removes_nothing() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Idempotent limit delete farm").await;
    let kept = create_limit(
        &emulator,
        &farm_id,
        create_body("Kept", "amount.maya", 5),
    )
    .await;
    let absent_farm = absent_id("farm", &format!("{:032x}", 0x21));
    let absent_limit = absent_id("limit", &format!("{:032x}", 0x22));

    // The rule: with no not-found error modeled, a well-formed id naming nothing is an
    // empty success. The rows differ along which level is missing — the limit, the farm, or
    // both — and none of them touches the limit the farm does hold.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an unheld limit under a held farm", &farm_id, &absent_limit),
        ("a held limit under an unheld farm", &absent_farm, &kept),
        ("both levels unheld", &absent_farm, &absent_limit),
    ];
    for (case, farm, limit) in &cases {
        let (status, body) = delete_limit(&emulator, farm, limit).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteLimit to answer 200, got {status} with {body}"
        );
        let (status, read) = emulator.get(&limit_path(&farm_id, &kept)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the held limit to be untouched, got {status} with {read}"
        );
    }
}

#[tokio::test]
async fn a_limit_a_queue_is_still_associated_with_is_not_deleted() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Associated limit farm").await;
    let other_farm_id = emulator.farm("Unrelated farm").await;

    // One limit per row of the table below, so each row's delete is independent. Each
    // carries its own `amountRequirementName`: one queue holds at most one association per
    // name.
    let mut limits: Vec<String> = Vec::new();
    for index in 0..4 {
        limits.push(
            create_limit(
                &emulator,
                &farm_id,
                create_body(&format!("Limit {index}"), &format!("amount.{index}"), 5),
            )
            .await,
        );
    }
    // A limit of the OTHER farm, carrying the id the other farm's queue is associated with,
    // so an association outside this farm cannot block this farm's delete.
    let elsewhere = create_limit(
        &emulator,
        &other_farm_id,
        create_body("Elsewhere", "amount.elsewhere", 5),
    )
    .await;

    let sole = emulator.queue(&farm_id, "Sole holder").await;
    let first_of_two = emulator.queue(&farm_id, "First holder").await;
    let second_of_two = emulator.queue(&farm_id, "Second holder").await;
    // A queue of this farm that is associated with nothing, so a row can tell "a queue
    // exists" from "a queue holds an association".
    let _unrelated_queue = emulator.queue(&farm_id, "Associated with nothing").await;
    let other_farm_queue = emulator.queue(&other_farm_id, "Another farm's queue").await;

    associate(&emulator, &farm_id, &sole, &limits[0]).await;
    associate(&emulator, &farm_id, &first_of_two, &limits[1]).await;
    associate(&emulator, &farm_id, &second_of_two, &limits[1]).await;
    associate(&emulator, &other_farm_id, &other_farm_queue, &elsewhere).await;

    // `expected_queue` is the queue the refusal must name: the lowest id of the queues
    // holding an association, so the answer does not depend on map order. `None` means the
    // delete is expected to succeed.
    let cases: Vec<(&str, &str, &str, Option<String>)> = vec![
        (
            "one queue is associated with this limit",
            &farm_id,
            &limits[0],
            Some(sole),
        ),
        (
            "two queues are associated with this limit",
            &farm_id,
            &limits[1],
            Some(first_of_two.min(second_of_two)),
        ),
        (
            "no queue of this farm is associated with this limit",
            &farm_id,
            &limits[2],
            None,
        ),
        (
            "a queue of this farm exists but is associated with nothing",
            &farm_id,
            &limits[3],
            None,
        ),
        (
            "only a queue of ANOTHER farm is associated with a limit of that farm",
            &other_farm_id,
            &elsewhere,
            Some(other_farm_queue),
        ),
    ];

    for (case, farm, limit_id, expected_queue) in &cases {
        let (status, body) = delete_limit(&emulator, farm, limit_id).await;
        match expected_queue {
            Some(queue_id) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected DeleteLimit to answer 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body}"
                );
                assert_eq!(
                    string_at(&body, "message"),
                    format!(
                        "The limit {limit_id} can't be deleted because it is associated with \
                         the queue {queue_id}."
                    ),
                    "case {case:?}: expected the refusal to name the limit and the queue \
                     associated with it, got {body}"
                );
                // The refused delete removed nothing.
                let (status, read) = emulator.get(&limit_path(farm, limit_id)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the refused limit to stay readable, got {status} \
                     with {read}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DeleteLimit to answer 200, got {status} with {body}"
                );
                let (status, read) = emulator.get(&limit_path(farm, limit_id)).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the deleted limit to be gone, got {status} with \
                     {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_limit_is_deletable_once_nothing_is_associated_with_it() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Unblocking limit farm").await;
    let limit_id = create_limit(
        &emulator,
        &farm_id,
        create_body("Associated then freed", "amount.maya", 5),
    )
    .await;
    let queue_id = emulator.queue(&farm_id, "Holder").await;
    associate(&emulator, &farm_id, &queue_id, &limit_id).await;

    // The rule: the refusal follows the state, not the id — the same request that was
    // refused while a queue was associated with the limit succeeds once that queue is gone.
    let (status, body) = delete_limit(&emulator, &farm_id, &limit_id).await;
    assert_eq!(
        status, 400,
        "while the queue holds the association: expected 400, got {status} with {body}"
    );

    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "DeleteQueue: expected 200, got {status} with {body}"
    );

    let (status, body) = delete_limit(&emulator, &farm_id, &limit_id).await;
    assert_eq!(
        status, 200,
        "once nothing is associated with it: expected 200, got {status} with {body}"
    );
    let (status, read) = emulator.get(&limit_path(&farm_id, &limit_id)).await;
    assert_eq!(
        status, 404,
        "expected the freed limit to be gone after the delete, got {status} with {read}"
    );
}
