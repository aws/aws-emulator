// SPDX-License-Identifier: CC0-1.0

//! The QUESTION SET resource of Elemental Inference and its VERSIONS, end to end over the
//! assembled wire.
//!
//! The rules under test, each over a TABLE of inputs rather than one case:
//!
//! * `sha256` "covers the name of the question set and its questions", so it changes
//!   exactly when one of those changes;
//! * `UpdateQuestionSet` "replaces questions one type at a time": an omitted array keeps
//!   DRAFT's questions of that type, a supplied one replaces them, a supplied empty one
//!   leaves DRAFT with none;
//! * publishing copies DRAFT ("the new version has the same name and questions as DRAFT,
//!   and you can't change it"), numbers start at 1 and are never reused, and "if DRAFT
//!   hasn't changed since the latest published version … it returns the latest version";
//! * the listing "starts with the DRAFT version, followed by the published versions from
//!   the highest version number to the lowest";
//! * the content rules a create and an update are both held to earn the modelled
//!   `ValidationException` (HTTP 400).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::elementalinference_wire::{call, error_code, message_of, router};

/// One yesNo question, as the modelled `YesNoQuestion` requires: a name that is unique in
/// the version, and the text to ask.
fn yes_no(name: &str, question: &str) -> Value {
    json!({"name": name, "question": question})
}

/// Create one question set and return its id together with the create's answer.
async fn create_question_set(
    router: &MultiServiceRouter,
    body: Value,
    case: &str,
) -> (String, Value) {
    let (status, created) = call(router, "POST", "/v1/question-set", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQuestionSet answers 200, got {status} with {created}"
    );
    (
        created["id"].as_str().unwrap_or_default().to_string(),
        created,
    )
}

/// `POST /v1/question-set/{id}/versions`, asserting it is served, and return the answer.
async fn publish(router: &MultiServiceRouter, id: &str, body: Value, case: &str) -> Value {
    let (status, published) = call(
        router,
        "POST",
        &format!("/v1/question-set/{id}/versions"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQuestionSetVersion answers 200, got {status} with {published}"
    );
    published
}

/// `GET /v1/question-set/{id}` at `version`, asserting it is served.
async fn get_version(router: &MultiServiceRouter, id: &str, version: &str, case: &str) -> Value {
    let (status, read) = call(
        router,
        "GET",
        &format!("/v1/question-set/{id}?questionSetVersion={version}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQuestionSet at {version} answers 200, got {status} with {read}"
    );
    read
}

/// A created question set reports its DRAFT version, and `sha256` covers the name and the
/// questions and nothing else.
#[tokio::test]
async fn the_draft_hash_covers_the_name_and_the_questions() {
    let router = router();

    // Each row: the create body, and whether it states the same name AND questions as the
    // other row marked so — the rows whose hash must agree.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "one yesNo question",
            json!({"name": "qsalpha", "yesNo": [yes_no("is_live", "Is this live action?")]}),
            true,
        ),
        (
            "the same name and question again",
            json!({"name": "qsalpha", "yesNo": [yes_no("is_live", "Is this live action?")]}),
            true,
        ),
        (
            "the same question under another name for the set",
            json!({"name": "qsbeta", "yesNo": [yes_no("is_live", "Is this live action?")]}),
            false,
        ),
        (
            "the same set name with another question text",
            json!({"name": "qsalpha", "yesNo": [yes_no("is_live", "Is this a replay?")]}),
            false,
        ),
        (
            "a chooseOne question instead",
            json!({
                "name": "qsalpha",
                "chooseOne": [{
                    "name": "shot_type",
                    "question": "What kind of shot is this?",
                    "options": [
                        {"name": "wide", "text": "Wide"},
                        {"name": "close", "text": "Close"}
                    ]
                }]
            }),
            false,
        ),
    ];

    let mut same_content_hashes: Vec<String> = Vec::new();
    let mut every_hash: Vec<String> = Vec::new();
    for (label, body, same_content) in rows {
        let (id, created) = create_question_set(&router, body, label).await;
        let hash = created["sha256"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create reports a sha256, got {created}"))
            .to_string();
        assert_eq!(
            created["version"], json!("DRAFT"),
            "case {label:?}: a create reports the DRAFT version, got {created}"
        );
        assert_eq!(
            (created["description"].clone(), created["latestVersion"].clone()),
            (Value::Null, Value::Null),
            "case {label:?}: a new question set has no description and no published version, \
             got {created}"
        );
        assert_eq!(
            created["references"], json!([]),
            "case {label:?}: a new question set is referenced by no feed, got {created}"
        );
        assert!(
            hash.len() == 64 && hash.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {label:?}: the hash is 64 lowercase hexadecimal characters, got {hash:?}"
        );

        // A read at DRAFT reports the same hash the create did.
        let read = get_version(&router, &id, "DRAFT", label).await;
        assert_eq!(
            read["sha256"], json!(hash.clone()),
            "case {label:?}: a read reports the hash the create did, got {read}"
        );

        if same_content {
            same_content_hashes.push(hash.clone());
        }
        every_hash.push(hash);
    }

    same_content_hashes.dedup();
    assert_eq!(
        same_content_hashes.len(),
        1,
        "the same name and questions hash the same, got {same_content_hashes:?}"
    );
    let mut distinct = every_hash.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        every_hash.len() - 1,
        "only the two rows with the same content share a hash, got {every_hash:?}"
    );
}

/// `UpdateQuestionSet` replaces DRAFT's questions one type at a time, and leaves the types
/// it does not mention alone.
#[tokio::test]
async fn an_update_replaces_draft_questions_one_type_at_a_time() {
    let router = router();
    let (id, _created) = create_question_set(
        &router,
        json!({
            "name": "qspatch",
            "yesNo": [yes_no("is_live", "Is this live action?")],
            "chooseOne": [{
                "name": "shot_type",
                "question": "What kind of shot is this?",
                "options": [
                    {"name": "wide", "text": "Wide"},
                    {"name": "close", "text": "Close"}
                ]
            }]
        }),
        "the question set an update patches",
    )
    .await;

    // Each row: the PATCH body, then the members DRAFT reports afterwards. A type the row
    // omits keeps what DRAFT already held.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a name alone leaves both question types alone",
            json!({"name": "qspatched"}),
            json!({"name": "qspatched", "yesNo": [yes_no("is_live", "Is this live action?")]}),
        ),
        (
            "a replacement yesNo array replaces only that type",
            json!({"yesNo": [yes_no("is_replay", "Is this a replay?")]}),
            json!({"yesNo": [yes_no("is_replay", "Is this a replay?")]}),
        ),
        (
            "an empty chooseOne array leaves DRAFT with none of that type",
            json!({"chooseOne": []}),
            json!({"chooseOne": null, "yesNo": [yes_no("is_replay", "Is this a replay?")]}),
        ),
    ];

    for (label, patch, expected) in rows {
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/v1/question-set/{id}"),
            Some(patch),
        )
        .await;
        assert_eq!(
            (status, updated["version"].clone()),
            (200, json!("DRAFT")),
            "case {label:?}: the update answers 200 for DRAFT, got {status} with {updated}"
        );
        let read = get_version(&router, &id, "DRAFT", label).await;
        for (member, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &read[member], value,
                "case {label:?}: DRAFT reports {member} as {value}, got {read}"
            );
        }
    }
}

/// Publishing copies DRAFT, numbers start at 1, an unchanged DRAFT publishes nothing new,
/// and a deleted number is never reused.
#[tokio::test]
async fn publishing_copies_draft_and_never_reuses_a_version_number() {
    let router = router();
    let (id, created) = create_question_set(
        &router,
        json!({"name": "qsversions", "yesNo": [yes_no("is_live", "Is this live action?")]}),
        "the question set versions are published from",
    )
    .await;
    let draft_hash = created["sha256"].as_str().unwrap_or_default().to_string();

    // Version 1 carries DRAFT's name, questions and hash.
    let first = publish(
        &router,
        &id,
        json!({"description": "opening set"}),
        "the first publish",
    )
    .await;
    assert_eq!(
        (
            first["version"].clone(),
            first["latestVersion"].clone(),
            first["sha256"].clone(),
            first["description"].clone(),
            first["yesNo"].clone()
        ),
        (
            json!("1"),
            json!("1"),
            json!(draft_hash.clone()),
            json!("opening set"),
            json!([yes_no("is_live", "Is this live action?")])
        ),
        "the first publish is version 1, copying DRAFT, got {first}"
    );

    // DRAFT has not changed, so a second publish creates nothing and returns version 1.
    let repeated = publish(&router, &id, json!({}), "a publish of an unchanged DRAFT").await;
    assert_eq!(
        (repeated["version"].clone(), repeated["description"].clone()),
        (json!("1"), json!("opening set")),
        "an unchanged DRAFT publishes nothing new and returns the latest version, got \
         {repeated}"
    );

    // A changed DRAFT publishes version 2, and version 1 still holds what it was
    // published with.
    let (status, patched) = call(
        &router,
        "PATCH",
        &format!("/v1/question-set/{id}"),
        Some(json!({"yesNo": [yes_no("is_replay", "Is this a replay?")]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "DRAFT is updated, got {status} with {patched}"
    );
    let second = publish(&router, &id, json!({}), "the second publish").await;
    assert_eq!(
        (second["version"].clone(), second["latestVersion"].clone()),
        (json!("2"), json!("2")),
        "a changed DRAFT publishes the next number, got {second}"
    );
    let one = get_version(&router, &id, "1", "a published version after DRAFT moved on").await;
    assert_eq!(
        (one["yesNo"].clone(), one["sha256"].clone()),
        (
            json!([yes_no("is_live", "Is this live action?")]),
            json!(draft_hash.clone())
        ),
        "a published version cannot change, got {one}"
    );

    // The listing starts with DRAFT, then the published versions highest to lowest.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/v1/question-set/{id}/versions?maxResults=100"),
        None,
    )
    .await;
    let reported: Vec<Value> = listed["versions"]
        .as_array()
        .unwrap_or_else(|| panic!("the listing reports an array, got {listed}"))
        .iter()
        .map(|summary| summary["version"].clone())
        .collect();
    assert_eq!(
        (status, reported),
        (200, vec![json!("DRAFT"), json!("2"), json!("1")]),
        "the listing starts with DRAFT, then the published versions highest to lowest, got \
         {listed}"
    );
    let summaries: Vec<Value> = listed["versions"].as_array().cloned().unwrap_or_default();
    for summary in &summaries {
        assert!(
            summary["sha256"].as_str().is_some_and(|hash| hash.len() == 64),
            "every summary carries the version's hash, got {summary}"
        );
        assert!(
            summary["lastModifiedAt"].is_number(),
            "every summary carries the time it was last modified, got {summary}"
        );
    }

    // Deleting version 1 leaves `latestVersion` alone and its number is never reused.
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
    let (status, gone) = call(
        &router,
        "GET",
        &format!("/v1/question-set/{id}?questionSetVersion=1"),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "a deleted version resolves to nothing, got {status} with {gone}"
    );
    let (status, patched) = call(
        &router,
        "PATCH",
        &format!("/v1/question-set/{id}"),
        Some(json!({"yesNo": [yes_no("is_slow_motion", "Is this slow motion?")]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "DRAFT is updated again, got {status} with {patched}"
    );
    let third = publish(&router, &id, json!({}), "the publish after a deletion").await;
    assert_eq!(
        (third["version"].clone(), third["latestVersion"].clone()),
        (json!("3"), json!("3")),
        "a deleted number is never reused, got {third}"
    );
    let (status, still) = call(&router, "GET", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, still["latestVersion"].clone()),
        (200, json!("3")),
        "the set reports the highest version it ever published, got {status} with {still}"
    );
}

/// A `sha256` the request states is the hash of the DRAFT it means to publish: a DRAFT
/// that has moved on since is refused with the modelled `ConflictException`.
#[tokio::test]
async fn a_stale_publish_precondition_is_refused() {
    let router = router();
    let (id, created) = create_question_set(
        &router,
        json!({"name": "qsprecond", "yesNo": [yes_no("is_live", "Is this live action?")]}),
        "the question set a stale publish targets",
    )
    .await;
    let stale = created["sha256"].as_str().unwrap_or_default().to_string();

    let (status, patched) = call(
        &router,
        "PATCH",
        &format!("/v1/question-set/{id}"),
        Some(json!({"yesNo": [yes_no("is_replay", "Is this a replay?")]})),
    )
    .await;
    assert_eq!(status, 200, "DRAFT is updated, got {status} with {patched}");
    let current = patched["sha256"].as_str().unwrap_or_default().to_string();

    let (status, refused) = call(
        &router,
        "POST",
        &format!("/v1/question-set/{id}/versions"),
        Some(json!({"sha256": stale})),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictException"),
        "a stale precondition is refused, got {status} with {refused}"
    );
    assert!(
        message_of(&refused).contains(&current),
        "the refusal names the hash DRAFT really has, got {}",
        message_of(&refused)
    );
    // Nothing was published.
    let (status, read) = call(&router, "GET", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, read["latestVersion"].clone()),
        (200, Value::Null),
        "the refused publish created no version, got {status} with {read}"
    );

    // The hash the read reports IS the one a publish accepts.
    let accepted = publish(
        &router,
        &id,
        json!({"sha256": current}),
        "a publish stating the hash a read reported",
    )
    .await;
    assert_eq!(
        accepted["version"], json!("1"),
        "the hash a read reports is the one a publish accepts, got {accepted}"
    );
}

/// The content rules a question set is held to, refused with the modelled
/// `ValidationException` by the create and by the update alike.
#[tokio::test]
async fn content_outside_the_question_set_rules_is_refused() {
    let router = router();
    let (id, _created) = create_question_set(
        &router,
        json!({"name": "qsrules", "yesNo": [yes_no("is_live", "Is this live action?")]}),
        "the question set the update rows patch",
    )
    .await;

    // The rows differ along which rule the content breaks: no questions at all, and a
    // name used twice across two question types.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a question set with no questions at all",
            json!({"name": "qsempty"}),
            json!({"yesNo": []}),
        ),
        (
            "a question name used by two question types",
            json!({
                "name": "qsdup",
                "yesNo": [yes_no("shot_type", "Is this a wide shot?")],
                "chooseOne": [{
                    "name": "shot_type",
                    "question": "What kind of shot is this?",
                    "options": [
                        {"name": "wide", "text": "Wide"},
                        {"name": "close", "text": "Close"}
                    ]
                }]
            }),
            json!({
                "yesNo": [yes_no("shot_type", "Is this a wide shot?")],
                "chooseOne": [{
                    "name": "shot_type",
                    "question": "What kind of shot is this?",
                    "options": [
                        {"name": "wide", "text": "Wide"},
                        {"name": "close", "text": "Close"}
                    ]
                }]
            }),
        ),
    ];

    for (label, create_body, patch_body) in rows {
        let (status, refused) = call(&router, "POST", "/v1/question-set", Some(create_body)).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {label:?}: the create is refused, got {status} with {refused}"
        );
        let (status, refused) = call(
            &router,
            "PATCH",
            &format!("/v1/question-set/{id}"),
            Some(patch_body),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {label:?}: the update is refused on the same rule, got {status} with {refused}"
        );
    }

    // The refused writes left DRAFT as it was.
    let read = get_version(&router, &id, "DRAFT", "after the refused writes").await;
    assert_eq!(
        read["yesNo"],
        json!([yes_no("is_live", "Is this live action?")]),
        "the refused writes left DRAFT as it was, got {read}"
    );
}

/// `DeleteQuestionSet` removes the set with its DRAFT and every published version, and the
/// id then names nothing.
#[tokio::test]
async fn a_deleted_question_set_leaves_the_store() {
    let router = router();
    let (id, _created) = create_question_set(
        &router,
        json!({"name": "qsdoomed", "yesNo": [yes_no("is_live", "Is this live action?")]}),
        "the question set a delete removes",
    )
    .await;
    publish(&router, &id, json!({}), "a version of the doomed set").await;

    let (status, deleted) = call(&router, "DELETE", &format!("/v1/question-set/{id}"), None).await;
    assert_eq!(
        (status, deleted["id"].clone()),
        (200, json!(id.clone())),
        "DeleteQuestionSet answers 200 naming the set, got {status} with {deleted}"
    );
    for (label, uri) in [
        ("the set itself", format!("/v1/question-set/{id}")),
        (
            "its published version",
            format!("/v1/question-set/{id}?questionSetVersion=1"),
        ),
        (
            "its version listing",
            format!("/v1/question-set/{id}/versions"),
        ),
    ] {
        let (status, gone) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            (status, error_code(&gone)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a removed question set resolves to nothing, got {status} with {gone}"
        );
    }
    let (status, listed) = call(&router, "GET", "/v1/question-sets?maxResults=100", None).await;
    assert_eq!(
        (status, listed["questionSets"].as_array().map(Vec::len)),
        (200, Some(0)),
        "the listing no longer reports it, got {status} with {listed}"
    );
}
