// SPDX-License-Identifier: CC0-1.0

//! The round trip every resource the model update added makes through its own
//! operations, and the PAGE each of their listings reports.
//!
//! Each suite here drives one resource end to end over the wire — create, read, list,
//! update, delete, read again — so what a create reports is shown to be what the store
//! holds and what a later reader answers with, and nothing a delete removed is reported
//! afterwards. The listings are asked for ONE record at a time, so the `nextToken` each
//! of them issues is exercised rather than assumed.
//!
//! The rules under test:
//!
//! * a create's own answer, a read of the resource and its entry in a listing all report
//!   the one stored record, so no two of them can disagree;
//! * a paginated listing of this service reports the records of the Region the request was
//!   signed for in one stable order, a page at a time, and issues a `nextToken` only while
//!   a further record is there to be fetched;
//! * a delete removes the resource — at once where the model gives the operation HTTP 200,
//!   and when the removal falls due where it gives HTTP 202 — after which the id names no
//!   resource for any operation, the delete itself included.

use crate::harness::{
    call, create_function, create_question_set, minimal_question_set, output, poll_until, router,
    PAGE_SIZE,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Every record a listing reports, asked for ONE at a time so the `nextToken` it issues
/// is followed rather than assumed.
///
/// `path` is the collection's own path and `member` the member it reports its page under.
/// Returns the records in the order the listing walked them.
async fn paged_records(
    router: &MultiServiceRouter,
    path: &str,
    member: &str,
    case: &str,
) -> Vec<Value> {
    let mut records: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One page per record, plus the page that ends the walk: a listing of this service
    // cannot report more than a Region holds, so this bound cannot be reached by a
    // correct listing and a `nextToken` that never ends fails the case instead of
    // looping.
    for _ in 0..=PAGE_SIZE {
        let uri = match &token {
            None => format!("{path}?maxResults=1"),
            Some(token) => format!("{path}?maxResults=1&nextToken={token}"),
        };
        let (status, page) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GET {uri} answers 200, got {status} with {page}"
        );
        let batch = page[member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the page reports a {member} array, got {page}"))
            .clone();
        assert!(
            batch.len() <= 1,
            "case {case:?}: a page of one holds at most one record, got {page}"
        );
        records.extend(batch);
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return records,
        }
    }
    panic!("case {case:?}: the listing of {path} never ran out of pages");
}

/// A function makes the whole round trip: the create's answer, a read and a listing agree,
/// an update is visible to a later read, and the delete removes it.
#[tokio::test]
async fn a_function_round_trips_through_its_own_operations() {
    let router = router();
    let (id, arn) = create_function(&router, "roundtripfn").await;

    // A read reports the stored record, and the id and ARN are the ones the create
    // minted.
    let (status, read) = call(&router, "GET", &format!("/v1/function/{id}"), None).await;
    assert_eq!(
        (status, read["id"].clone(), read["arn"].clone(), read["name"].clone()),
        (200, json!(id.clone()), json!(arn.clone()), json!("roundtripfn")),
        "a read reports the function the create minted, got {status} with {read}"
    );
    // `eventBridgeDelivery` declares `@default: "DISABLED"`, so a create that states none
    // stores that value rather than leaving the member absent.
    assert_eq!(
        read["eventBridgeDelivery"], json!("DISABLED"),
        "the declared default is stored, got {read}"
    );

    // A second function, so the listing really has a page to turn.
    let (other_id, _other_arn) = create_function(&router, "roundtripfn2").await;
    let listed = paged_records(&router, "/v1/functions", "functions", "two functions").await;
    let mut listed_ids: Vec<String> = listed
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_ids.sort();
    let mut expected = vec![id.clone(), other_id.clone()];
    expected.sort();
    assert_eq!(
        listed_ids, expected,
        "the listing reports every function of this endpoint, got {listed:?}"
    );

    // An update is visible to a later read: the PATCH reaches the store, not just the
    // response.
    let (status, updated) = call(
        &router,
        "PATCH",
        &format!("/v1/function/{id}"),
        Some(json!({"description": "round trip"})),
    )
    .await;
    assert_eq!(
        (status, updated["description"].clone()),
        (200, json!("round trip")),
        "UpdateFunction answers 200 with what it wrote, got {status} with {updated}"
    );
    let (status, read) = call(&router, "GET", &format!("/v1/function/{id}"), None).await;
    assert_eq!(
        (status, read["description"].clone()),
        (200, json!("round trip")),
        "a later read reports what the update wrote, got {status} with {read}"
    );

    // The delete is ACCEPTED, and once the removal falls due the id names nothing.
    let (status, accepted) = call(&router, "DELETE", &format!("/v1/function/{id}"), None).await;
    assert_eq!(
        (status, accepted["status"].clone()),
        (202, json!("DELETING")),
        "DeleteFunction is accepted with the function reporting DELETING, got {status} with \
         {accepted}"
    );
    let (status, gone) = poll_until(
        &router,
        &format!("/v1/function/{id}"),
        "a function whose deletion has completed",
        |status, _body| status == 404,
    )
    .await;
    assert_eq!(
        (status, gone["__type"].clone()),
        (404, json!("ResourceNotFoundException")),
        "a removed function resolves to nothing, got {status} with {gone}"
    );
    let remaining = paged_records(&router, "/v1/functions", "functions", "after the delete").await;
    assert_eq!(
        remaining
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>(),
        vec![other_id],
        "the listing no longer reports the deleted function, got {remaining:?}"
    );
}

/// A question set makes the whole round trip: DRAFT is read back, an update is visible, a
/// version is published and listed, and both deletes remove what they name.
#[tokio::test]
async fn a_question_set_round_trips_through_its_own_operations() {
    let router = router();
    let id = create_question_set(&router, "roundtripqs").await;
    let other_id = create_question_set(&router, "roundtripqs2").await;

    let (status, read) = call(&router, "GET", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, read["version"].clone(), read["name"].clone()),
        (200, json!("DRAFT"), json!("roundtripqs")),
        "a read reports DRAFT by default, got {status} with {read}"
    );

    // An update is visible to a later read.
    let (status, updated) = call(
        &router,
        "PATCH",
        &format!("/v1/question-set/{id}"),
        Some(json!({"yesNo": [{"name": "is_replay", "question": "Is this a replay?"}]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateQuestionSet answers 200, got {status} with {updated}"
    );
    let (status, read) = call(&router, "GET", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, read["yesNo"].clone()),
        (
            200,
            json!([{"name": "is_replay", "question": "Is this a replay?"}])
        ),
        "a later read reports the questions the update wrote, got {status} with {read}"
    );

    // Publishing DRAFT gives version 1, which the version listing then reports after
    // DRAFT.
    let (status, published) = call(
        &router,
        "POST",
        &format!("/v1/question-set/{id}/versions"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        (status, published["version"].clone()),
        (200, json!("1")),
        "CreateQuestionSetVersion publishes version 1, got {status} with {published}"
    );
    let versions = paged_records(
        &router,
        &format!("/v1/question-set/{id}/versions"),
        "versions",
        "DRAFT and one published version",
    )
    .await;
    assert_eq!(
        versions
            .iter()
            .map(|summary| summary["version"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("DRAFT"), json!("1")],
        "the version listing starts with DRAFT, got {versions:?}"
    );

    // The collection listing reports both question sets, a page at a time.
    let listed = paged_records(
        &router,
        "/v1/question-sets",
        "questionSets",
        "two question sets",
    )
    .await;
    let mut listed_ids: Vec<String> = listed
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_ids.sort();
    let mut expected = vec![id.clone(), other_id.clone()];
    expected.sort();
    assert_eq!(
        listed_ids, expected,
        "the listing reports every question set of this endpoint, got {listed:?}"
    );

    // Deleting the version leaves the set; deleting the set leaves nothing.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/v1/question-set/{id}/versions/1"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteQuestionSetVersion answers 200, got {status} with {deleted}"
    );
    let versions = paged_records(
        &router,
        &format!("/v1/question-set/{id}/versions"),
        "versions",
        "after the version was deleted",
    )
    .await;
    assert_eq!(
        versions
            .iter()
            .map(|summary| summary["version"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("DRAFT")],
        "the deleted version is gone from the listing, got {versions:?}"
    );

    let (status, deleted) = call(&router, "DELETE", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, deleted["id"].clone()),
        (200, json!(id.clone())),
        "DeleteQuestionSet answers 200 naming the set, got {status} with {deleted}"
    );
    let (status, gone) = call(&router, "GET", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, gone["__type"].clone()),
        (404, json!("ResourceNotFoundException")),
        "a removed question set resolves to nothing, got {status} with {gone}"
    );
    let remaining = paged_records(
        &router,
        "/v1/question-sets",
        "questionSets",
        "after the set was deleted",
    )
    .await;
    assert_eq!(
        remaining
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>(),
        vec![other_id],
        "the listing no longer reports the deleted question set, got {remaining:?}"
    );
}

/// A schedule action makes the whole round trip on its feed: it is created, listed,
/// deleted, and gone from the listing afterwards.
#[tokio::test]
async fn a_schedule_action_round_trips_on_its_feed() {
    let router = router();
    let (status, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "scheduledfeed",
            "outputs": [output("crop1", "cropping", "ENABLED")]
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the feed is created, got {status} with {created}"
    );
    let feed = created["id"].as_str().unwrap_or_default().to_string();

    // Two actions, far enough ahead that nothing this test does makes them due, so the
    // listing has a page to turn and both stay PENDING.
    let scheduled_at = now_secs() + 86_400;
    for name in ["first", "second"] {
        let (status, action) = call(
            &router,
            "POST",
            &format!("/v1/feed/{feed}/schedule-actions"),
            Some(json!({
                "actionName": name,
                "startConfiguration": {
                    "fixedModeScheduleActionStartSettings": {"scheduledAt": scheduled_at}
                },
                "operations": [
                    {"updateOutputStatus": {"outputName": "crop1", "status": "DISABLED"}}
                ]
            })),
        )
        .await;
        assert_eq!(
            (status, action["status"].clone(), action["actionName"].clone()),
            (201, json!("PENDING"), json!(name)),
            "CreateScheduleAction({name:?}) answers 201 with the action pending, got {status} \
             with {action}"
        );
    }

    let listed = paged_records(
        &router,
        &format!("/v1/feed/{feed}/schedule-actions"),
        "scheduleActions",
        "two pending actions",
    )
    .await;
    assert_eq!(
        listed
            .iter()
            .map(|action| action["actionName"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("first"), json!("second")],
        "the listing reports both actions in the order they were created, got {listed:?}"
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/v1/feed/{feed}/schedule-actions?actionName=first"),
        None,
    )
    .await;
    assert_eq!(
        (status, deleted["actionName"].clone()),
        (200, json!("first")),
        "DeleteScheduleAction answers 200 naming the action, got {status} with {deleted}"
    );
    let listed = paged_records(
        &router,
        &format!("/v1/feed/{feed}/schedule-actions"),
        "scheduleActions",
        "after the delete",
    )
    .await;
    assert_eq!(
        listed
            .iter()
            .map(|action| action["actionName"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("second")],
        "the deleted action is gone from the listing, got {listed:?}"
    );
}

/// A question set the request's own members have already minted is not minted twice: the
/// create is `@idempotent` with no `@idempotencyToken`, so several identical requests have
/// the effect of ONE.
#[tokio::test]
async fn a_repeated_question_set_create_mints_one_record() {
    let router = router();
    let body = minimal_question_set("replayedqs");
    let (status, first) = call(&router, "POST", "/v1/question-set", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "the first create answers 200, got {status} with {first}"
    );
    let (status, second) = call(&router, "POST", "/v1/question-set", Some(body)).await;
    assert_eq!(
        (status, second.clone()),
        (200, first.clone()),
        "a repeat answers with the question set the first create minted, got {status} with \
         {second}"
    );
    let listed = paged_records(
        &router,
        "/v1/question-sets",
        "questionSets",
        "after the repeated create",
    )
    .await;
    assert_eq!(
        listed.len(),
        1,
        "several identical requests have the effect of one, got {listed:?}"
    );
}

/// A UTC instant in epoch seconds, which is how `restJson1` carries the modelled
/// `Timestamp` an action's `scheduledAt` is.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}
