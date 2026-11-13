// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteRevision`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `harness`, `call`, `create_data_set`) is the one
//! `data_sets.rs` already uses, with `create_revision` from `revisions.rs` and
//! `import_assets` from `revision_finalization.rs`.
//!
//! The rule under test: `DeleteRevision` removes the named revision of the named
//! data set — with everything the record holds, its assets and its tags — and
//! answers 204 with no body; every later reader then reports it absent, while every
//! other revision, of this data set and of any other, is untouched. A revision in the
//! state the model's `Finalized` documentation calls read-only cannot be removed at
//! all, which is the modelled 409. A well-formed id naming no data set, or no revision
//! of it, is the modelled 404.
//!
//! The rows span what the rule generalises over: how many revisions the data set
//! holds, which position in the creation order is deleted, what the deleted record
//! carries (comment, tags, assets), and which named resource is absent.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, emulator, harness, is_modelled_id, Answer};
use crate::resource_tags::percent_encode;
use crate::revision_finalization::import_assets;
use crate::revisions::create_revision;

/// Well-formed per the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` (32 characters)
/// but never created.
const NEVER_CREATED: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// The model's `ResourceType` values the 404s below report, from the `ResourceType`
/// enum (`ASSET | DATA_GRANT | DATA_SET | EVENT_ACTION | JOB | REVISION`).
/// Source: `smithy_query(query="shape", shape="ResourceType")`.
const RESOURCE_TYPE_REVISION: &str = "REVISION";
const RESOURCE_TYPE_DATA_SET: &str = "DATA_SET";

/// The whole `Message` AWS answers an action on a finalized revision with — fixed text
/// with no variable part, stated here independently of the constant the crate
/// implements it with.
const FINALIZED_ACTION_MESSAGE: &str = "Cannot take action on a Finalized Revision.";

/// `DELETE /v1/data-sets/{DataSetId}/revisions/{RevisionId}`.
async fn delete_revision(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        None,
    )
    .await
}

/// `GET /v1/data-sets/{DataSetId}/revisions/{RevisionId}`, the single-revision reader.
async fn get_revision(router: &MultiServiceRouter, data_set_id: &str, revision_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        None,
    )
    .await
}

/// The ids `ListDataSetRevisions` reports for a data set, in the order it pages
/// them (newest first, per that operation's documentation).
async fn listed_revision_ids(router: &MultiServiceRouter, data_set_id: &str) -> Vec<String> {
    let listed = call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions?maxResults=200"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case list {data_set_id}: expected 200 from ListDataSetRevisions, got {} body {:?}",
        listed.status, listed.body
    );
    listed
        .member("Revisions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed revision carries no Id: {entry:?}"))
                .to_string()
        })
        .collect()
}

/// Whether state still holds the revision at all, read straight off the store — so an
/// assertion is about what the call persisted and not only about what a reader says.
fn stored(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
) -> Option<bool> {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    let data_set = store.data_sets.get(data_set_id)?;
    Some(
        data_set.revisions.contains_key(revision_id)
            || data_set.revision_order.iter().any(|id| id == revision_id),
    )
}

/// One row of the deletion catalog: how many revisions the data set holds, which of
/// them (by creation position) is deleted, and what that record carries.
struct DeleteCase {
    case: &'static str,
    revisions: usize,
    position: usize,
    comment: Option<&'static str>,
    tags: Option<Value>,
    assets: usize,
}

/// The rows differ along the count of revisions, the position deleted (only / first /
/// middle / last), and the state of the deleted record: bare, commented, tagged and
/// holding assets. A finalized revision cannot be deleted at all, which
/// [`deleting_a_finalized_revision_is_refused`] is about.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "the only revision, nothing on it",
            revisions: 1,
            position: 0,
            comment: None,
            tags: None,
            assets: 0,
        },
        DeleteCase {
            case: "the first of two, commented",
            revisions: 2,
            position: 0,
            comment: Some("first cut of the monthly feed"),
            tags: None,
            assets: 0,
        },
        DeleteCase {
            case: "the middle of three, tagged",
            revisions: 3,
            position: 1,
            comment: None,
            tags: Some(json!({ "team": "data", "tier": "gold" })),
            assets: 0,
        },
        DeleteCase {
            case: "the last of five, holding one asset",
            revisions: 5,
            position: 4,
            comment: Some(""),
            tags: Some(json!({ "регион": "eu" })),
            assets: 1,
        },
        DeleteCase {
            case: "the middle of four, holding several assets",
            revisions: 4,
            position: 2,
            comment: Some("published"),
            tags: Some(json!({ "team": "data" })),
            assets: 3,
        },
    ]
}

/// RULE: `DeleteRevision` removes the named revision of the named data set from
/// state and answers 204 with no body; afterwards both readers report it absent and
/// every other revision of that data set keeps its record and its place in the
/// order.
#[tokio::test]
async fn deleting_a_revision_removes_only_that_revision() {
    let (router, engine) = harness();

    for DeleteCase {
        case,
        revisions,
        position,
        comment,
        tags,
        assets,
    } in delete_cases()
    {
        let data_set_id = create_data_set(&router, case, "d", "S3_SNAPSHOT", None)
            .await
            .string("Id");

        let mut ids = Vec::new();
        for index in 0..revisions {
            let target = index == position;
            let created = create_revision(
                &router,
                &data_set_id,
                if target {
                    comment
                } else {
                    Some("kept alongside")
                },
                if target { tags.as_ref() } else { None },
                case,
            )
            .await;
            ids.push(created.string("Id"));
        }
        let target_id = ids[position].clone();
        if assets > 0 {
            import_assets(&engine, &data_set_id, &target_id, assets);
        }

        let deleted = delete_revision(&router, &data_set_id, &target_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204 from DeleteRevision, got {} body {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case}: expected no response body, got {:?}",
            deleted.body
        );

        // State no longer holds the record, in the map OR in the order the list pages.
        assert_eq!(
            stored(&engine, &data_set_id, &target_id),
            Some(false),
            "case {case}: expected the deleted revision to be gone from state"
        );

        // The single-revision reader reports it absent, naming the revision as the
        // absent resource.
        let fetched = get_revision(&router, &data_set_id, &target_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected 404 from GetRevision after the delete, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.member("ResourceType"),
            &json!(RESOURCE_TYPE_REVISION),
            "case {case}: expected the 404 to name the revision, got {:?}",
            fetched.member("ResourceType")
        );

        // The list reader drops it and keeps the rest, in the order it pages them:
        // newest first, which the model's own documentation states as "sorted by
        // CreatedAt in descending order".
        let expected: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != position)
            .map(|(_, id)| id.clone())
            .rev()
            .collect();
        assert_eq!(
            listed_revision_ids(&router, &data_set_id).await,
            expected,
            "case {case}: expected ListDataSetRevisions to report the surviving revisions \
             newest first"
        );

        // Every surviving revision is still readable, with its own record.
        for (index, id) in ids.iter().enumerate() {
            if index == position {
                continue;
            }
            let kept = get_revision(&router, &data_set_id, id).await;
            assert_eq!(
                kept.status, 200,
                "case {case}: expected the revision at position {index} to survive, got {} \
                 body {:?}",
                kept.status, kept.body
            );
            assert_eq!(
                kept.member("Comment"),
                &json!("kept alongside"),
                "case {case}: expected the surviving revision's own Comment, got {:?}",
                kept.member("Comment")
            );
        }

        // The data set itself is still there — only its revision went.
        let data_set = call(&router, "GET", &format!("/v1/data-sets/{data_set_id}"), None).await;
        assert_eq!(
            data_set.status, 200,
            "case {case}: expected the data set to survive the revision delete, got {} \
             body {:?}",
            data_set.status, data_set.body
        );
    }
}

/// RULE: a revision in the state the model's `Finalized` documentation calls read-only
/// cannot be removed: the delete is refused with the modelled 409 `ConflictException`
/// carrying the fixed sentence for this error class and naming the revision, and the
/// revision — with everything it holds — is still there afterwards.
///
/// The rows differ along what the finalized revision holds and how many revisions the
/// data set has, because the refusal is about the revision's state and nothing else.
#[tokio::test]
async fn deleting_a_finalized_revision_is_refused() {
    let (router, engine) = harness();

    for (case, revisions, assets) in [
        ("the only revision, holding one asset", 1usize, 1usize),
        ("one of three, holding several assets", 3, 3),
    ] {
        let data_set_id = create_data_set(&router, case, "d", "S3_SNAPSHOT", None)
            .await
            .string("Id");
        let mut ids = Vec::new();
        for _ in 0..revisions {
            ids.push(
                create_revision(&router, &data_set_id, Some("published"), None, case)
                    .await
                    .string("Id"),
            );
        }
        let target_id = ids[revisions - 1].clone();
        import_assets(&engine, &data_set_id, &target_id, assets);
        let finalized = call(
            &router,
            "PATCH",
            &format!("/v1/data-sets/{data_set_id}/revisions/{target_id}"),
            Some(json!({ "Finalized": true })),
        )
        .await;
        assert_eq!(
            finalized.status, 200,
            "case {case}: expected 200 finalizing the revision, got {} body {:?}",
            finalized.status, finalized.body
        );

        let refused = delete_revision(&router, &data_set_id, &target_id).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409 deleting a finalized revision, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.string("Message"),
            FINALIZED_ACTION_MESSAGE,
            "case {case}: expected the fixed refusal sentence, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.member("ResourceId").as_str(),
            Some(target_id.as_str()),
            "case {case}: expected ResourceId to name the revision, got {:?}",
            refused.member("ResourceId")
        );
        assert_eq!(
            refused.member("ResourceType"),
            &json!(RESOURCE_TYPE_REVISION),
            "case {case}: expected ResourceType {RESOURCE_TYPE_REVISION}, got {:?}",
            refused.member("ResourceType")
        );

        // Nothing was removed: state still holds the record, and both readers still
        // report it.
        assert_eq!(
            stored(&engine, &data_set_id, &target_id),
            Some(true),
            "case {case}: expected the refused delete to leave the revision in state"
        );
        let kept = get_revision(&router, &data_set_id, &target_id).await;
        assert_eq!(
            kept.status, 200,
            "case {case}: expected the revision to still read back, got {} body {:?}",
            kept.status, kept.body
        );
        assert_eq!(
            listed_revision_ids(&router, &data_set_id).await.len(),
            revisions,
            "case {case}: expected every revision to survive the refused delete"
        );
    }
}

/// RULE: the record goes whole — the tags a deleted revision carried are gone with
/// it, so the tag reader no longer addresses its ARN, and a fresh revision of the
/// same data set inherits nothing from it.
#[tokio::test]
async fn a_deleted_revision_takes_its_tags_with_it() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "tagged-deletes", "t", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Rows differ along how many tags the deleted revision carried and how many
    // assets it held: the record goes whole in every case.
    let rows: Vec<(&str, Option<Value>, usize)> = vec![
        ("no tags, no assets", None, 0),
        ("one tag", Some(json!({ "team": "data" })), 0),
        (
            "several tags and assets",
            Some(json!({ "team": "data", "tier": "gold", "owner": "ops" })),
            2,
        ),
    ];

    for (case, tags, assets) in rows {
        let created = create_revision(&router, &data_set_id, None, tags.as_ref(), case).await;
        let revision_id = created.string("Id");
        let arn = created.string("Arn");
        if assets > 0 {
            import_assets(&engine, &data_set_id, &revision_id, assets);
        }

        let before = call(&router, "GET", &format!("/tags/{}", percent_encode(&arn)), None).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected the tag reader to address the revision before the \
             delete, got {} body {:?}",
            before.status, before.body
        );

        let deleted = delete_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204 from DeleteRevision, got {} body {:?}",
            deleted.status, deleted.body
        );

        let after = call(&router, "GET", &format!("/tags/{}", percent_encode(&arn)), None).await;
        assert_eq!(
            after.status, 404,
            "case {case}: expected the tag reader to answer 404 for the deleted revision's \
             ARN, got {} body {:?}",
            after.status, after.body
        );

        // A new revision of the same data set is a new record: a fresh id, no tags and
        // no assets carried over from the one that was deleted.
        let fresh = create_revision(&router, &data_set_id, None, None, case).await;
        assert_ne!(
            fresh.string("Id"),
            revision_id,
            "case {case}: expected a freshly created revision to get its own id"
        );
        assert!(
            is_modelled_id(&fresh.string("Id")),
            "case {case}: expected an Id matching ^[a-zA-Z0-9]{{30,40}}$, got {:?}",
            fresh.member("Id")
        );
        assert_eq!(
            fresh.member("Tags"),
            &Value::Null,
            "case {case}: expected the fresh revision to carry no tags, got {:?}",
            fresh.member("Tags")
        );
        let deleted_again = delete_revision(&router, &data_set_id, &fresh.string("Id")).await;
        assert_eq!(
            deleted_again.status, 204,
            "case {case}: expected 204 deleting the fresh revision too, got {} body {:?}",
            deleted_again.status, deleted_again.body
        );
    }
}

/// RULE: a well-formed id naming no data set, or no revision of the named data set,
/// is a 404 `ResourceNotFoundException` whose `ResourceType` names which resource was
/// absent — so a delete is not idempotent in state it never held: the second delete
/// of one revision is that same 404.
#[tokio::test]
async fn deleting_an_absent_revision_is_a_404() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "absent-deletes", "a", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let other_data_set_id = create_data_set(&router, "other-deletes", "o", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let revision_id = create_revision(&router, &data_set_id, None, None, "setup")
        .await
        .string("Id");
    let other_revision_id = create_revision(&router, &other_data_set_id, None, None, "setup")
        .await
        .string("Id");

    // Rows differ along WHICH named resource is absent: the data set, the revision,
    // or a pair that each exist but not together.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "unknown data set, unknown revision",
            NEVER_CREATED.to_string(),
            NEVER_CREATED.to_string(),
            RESOURCE_TYPE_DATA_SET,
        ),
        (
            "unknown data set, a revision that exists elsewhere",
            NEVER_CREATED.to_string(),
            revision_id.clone(),
            RESOURCE_TYPE_DATA_SET,
        ),
        (
            "known data set, unknown revision",
            data_set_id.clone(),
            NEVER_CREATED.to_string(),
            RESOURCE_TYPE_REVISION,
        ),
        (
            "known data set, a revision of another data set",
            data_set_id.clone(),
            other_revision_id.clone(),
            RESOURCE_TYPE_REVISION,
        ),
        (
            "known data set, a data set id used as a revision id",
            data_set_id.clone(),
            data_set_id.clone(),
            RESOURCE_TYPE_REVISION,
        ),
    ];

    for (case, data_set, revision, resource_type) in &rows {
        let answer = delete_revision(&router, data_set, revision).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.member("ResourceType"),
            &json!(resource_type),
            "case {case}: expected ResourceType {resource_type}, got {:?}",
            answer.member("ResourceType")
        );
        assert_eq!(
            answer.member("ResourceId"),
            &json!(match *resource_type {
                RESOURCE_TYPE_DATA_SET => data_set.clone(),
                _ => revision.clone(),
            }),
            "case {case}: expected the 404 to name the absent id, got {:?}",
            answer.member("ResourceId")
        );
    }

    // The revision of the other data set was NOT deleted by the refused call above.
    let survivor = get_revision(&router, &other_data_set_id, &other_revision_id).await;
    assert_eq!(
        survivor.status, 200,
        "case refused cross-data-set delete: expected the revision to survive, got {} body {:?}",
        survivor.status, survivor.body
    );

    // A delete removes the revision once; the next one names nothing, whichever
    // reader last saw it.
    let first = delete_revision(&router, &data_set_id, &revision_id).await;
    assert_eq!(
        first.status, 204,
        "case first delete: expected 204, got {} body {:?}",
        first.status, first.body
    );
    for round in 1..=2 {
        let case = format!("repeat delete {round}");
        let again = delete_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected 404 for a revision already deleted, got {} body {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.member("ResourceType"),
            &json!(RESOURCE_TYPE_REVISION),
            "case {case}: expected the 404 to name the revision, got {:?}",
            again.member("ResourceType")
        );
    }

    // And once the data set is gone, the data set is the absent resource.
    let data_set_deleted = call(&router, "DELETE", &format!("/v1/data-sets/{data_set_id}"), None).await;
    assert_eq!(
        data_set_deleted.status, 204,
        "case delete the data set: expected 204 from DeleteDataSet, got {} body {:?}",
        data_set_deleted.status, data_set_deleted.body
    );
    let after = delete_revision(&router, &data_set_id, &revision_id).await;
    assert_eq!(
        after.status, 404,
        "case delete after the data set went: expected 404, got {} body {:?}",
        after.status, after.body
    );
    assert_eq!(
        after.member("ResourceType"),
        &json!(RESOURCE_TYPE_DATA_SET),
        "case delete after the data set went: expected the data set to be the absent \
         resource, got {:?}",
        after.member("ResourceType")
    );
}

/// RULE: an id outside the modelled `^[a-zA-Z0-9]{30,40}$` is a 400
/// `ValidationException` in either label position, not a 404; an id inside the
/// pattern is acceptable, so the answer is then about the resource.
#[tokio::test]
async fn a_malformed_id_in_either_position_is_a_400() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "malformed-deletes", "m", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Rows differ along HOW the id breaks the pattern — too short, too long, a
    // disallowed character — plus both published bounds, which are well formed.
    let ids = [
        ("one short of the minimum length", "a".repeat(29)),
        ("at the minimum length", "a".repeat(30)),
        ("at the maximum length", "a".repeat(40)),
        ("one past the maximum length", "a".repeat(41)),
        (
            "a plus sign is not alphanumeric",
            format!("{}+{}", "a".repeat(16), "b".repeat(15)),
        ),
    ];

    for (how, id) in ids {
        for position in ["DataSetId", "RevisionId"] {
            let case = format!("{position} {how}");
            let answer = match position {
                "DataSetId" => delete_revision(&router, &id, NEVER_CREATED).await,
                _ => delete_revision(&router, &data_set_id, &id).await,
            };
            if is_modelled_id(&id) {
                assert_eq!(
                    answer.status, 404,
                    "case {case}: expected 404 for a well-formed but unknown id {id:?}, \
                     got {} body {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400 for {id:?}, got {} body {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ValidationException"),
                    "case {case}: expected ValidationException for {id:?}, got {:?}",
                    answer.error
                );
            }
        }
    }
}
