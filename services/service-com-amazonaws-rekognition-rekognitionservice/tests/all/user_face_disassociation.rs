// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DisassociateFaces` over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.<Operation>`), dispatched through the same router the
//! production emulator mounts.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body: nothing reaches into the service's state struct, so a response that is not
//! computed from persisted state cannot pass.
//!
//! Each test states a RULE and asserts it over a TABLE of inputs:
//!
//! * a disassociation takes a requested face off the named user exactly when that user
//!   holds it, reporting every other requested face on the unsuccessful side with the
//!   reason the model names for it, and ignoring a face that is already off a user; what
//!   it persisted is what `ListFaces`, `ListUsers` and `DeleteFaces` then answer from;
//! * a face is disassociated only from a user and a collection the store holds, and a
//!   refused request persists nothing;
//! * a disassociation completes once per `ClientRequestToken`: the same token with the
//!   same request answers with the same response and takes nothing off a second time,
//!   while the same token with a different request is a parameter mismatch;
//! * a call that disassociated nothing leaves the user carrying the status it already had.

use serde_json::{json, Value};

// The wire harness is the collection suite's, so a request is built one way whichever
// operation is being exercised, and the drawn portrait comes from it too, so a picture is
// drawn one way wherever a face has to be indexed first.
use crate::face_collections::{bytes_image, call, harness, one_face_png, Reply};

/// The `UnsuccessfulFaceDisassociationReason` values
/// (`smithy_query(query="shape",shape="UnsuccessfulFaceDisassociationReason")`:
/// FACE_NOT_FOUND | ASSOCIATED_TO_A_DIFFERENT_USER) — the whole published catalogue of
/// reasons this operation reports.
const FACE_NOT_FOUND: &str = "FACE_NOT_FOUND";
const ASSOCIATED_TO_A_DIFFERENT_USER: &str = "ASSOCIATED_TO_A_DIFFERENT_USER";

/// `UserMatchThreshold` is `Percent`, `range 0..100`
/// (`smithy_query(query="operation",operation="AssociateFaces")`). The bottom of the range
/// is what the setup of each case associates with, so every row below is decided by which
/// face it names rather than by a measurement.
const PERCENT_MIN: f64 = 0.0;

/// A `FaceId` the service never minted, in the published form of one
/// (`pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`,
/// `smithy_query(query="shape",shape="FaceId")`), so it is the handler and not the
/// boundary validator that answers for it.
const UNMINTED_FACE_ID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The `UserStatus` values this suite branches on. `UserStatus` is
/// ACTIVE | UPDATING | CREATING | CREATED (`smithy_query(query="shape",shape="UserStatus")`);
/// `UPDATING` is what a call that changed a user's faces answers while the update is still
/// being applied, and `CREATED` what a user no face is associated with carries.
const UPDATING: &str = "UPDATING";
const CREATED: &str = "CREATED";

async fn create_collection(router: &smithy_server_runtime::MultiServiceRouter, collection_id: &str) {
    let reply = call(
        router,
        "CreateCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {collection_id}: the collection must be created, got {:?}",
        reply.body
    );
}

async fn create_user(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
    user_id: &str,
) {
    let reply = call(
        router,
        "CreateUser",
        json!({ "CollectionId": collection_id, "UserId": user_id }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {collection_id}/{user_id}: the user must be created, got {:?}",
        reply.body
    );
}

/// Index the suite's one-face portrait into `collection_id` and answer the `FaceId` minted
/// for the one face it carries.
async fn index_face(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
) -> String {
    let reply = call(
        router,
        "IndexFaces",
        json!({
            "CollectionId": collection_id,
            "Image": bytes_image(&one_face_png(11)),
        }),
    )
    .await;
    let records = reply.list("FaceRecords");
    assert_eq!(
        records.len(),
        1,
        "case {collection_id}: the portrait carries one face, so one face must be indexed, \
         got {:?}",
        reply.body
    );
    records[0]
        .get("Face")
        .and_then(|face| face.get("FaceId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Associate `face_ids` with `user_id`, asserting the association went through: the setup
/// every disassociation case starts from.
async fn associate_faces(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
    user_id: &str,
    face_ids: &[String],
) {
    let reply = call(
        router,
        "AssociateFaces",
        json!({
            "CollectionId": collection_id,
            "UserId": user_id,
            "FaceIds": face_ids,
            "UserMatchThreshold": PERCENT_MIN,
        }),
    )
    .await;
    let associated: Vec<String> = reply
        .list("AssociatedFaces")
        .iter()
        .map(|face| {
            face.get("FaceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        (reply.status, associated),
        (200, face_ids.to_vec()),
        "case {collection_id}/{user_id}: every face of the setup must be associated, got {:?}",
        reply.body
    );
}

/// `DisassociateFaces` in `collection_id` for `user_id` over `face_ids`, with the
/// `ClientRequestToken` when one is given.
async fn disassociate_faces(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
    user_id: &str,
    face_ids: &[String],
    token: Option<&str>,
) -> Reply {
    let mut body = json!({
        "CollectionId": collection_id,
        "UserId": user_id,
        "FaceIds": face_ids,
    });
    if let Some(token) = token {
        body["ClientRequestToken"] = json!(token);
    }
    call(router, "DisassociateFaces", body).await
}

/// The `FaceId` of every face an answer reports as disassociated, in the order it reports
/// them.
fn disassociated(reply: &Reply) -> Vec<String> {
    reply
        .list("DisassociatedFaces")
        .iter()
        .map(|face| {
            face.get("FaceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every reported `UnsuccessfulFaceDisassociation`, as the triple a caller branches on: the
/// face, the user that holds it and the reasons.
fn unsuccessful(reply: &Reply) -> Vec<(String, Option<String>, Vec<String>)> {
    reply
        .list("UnsuccessfulFaceDisassociations")
        .iter()
        .map(|entry| {
            (
                entry
                    .get("FaceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("UserId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                entry
                    .get("Reasons")
                    .and_then(Value::as_array)
                    .map(|reasons| {
                        reasons
                            .iter()
                            .map(|reason| reason.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// Every `(FaceId, UserId)` `ListFaces` reports for `collection_id`, with the filter the
/// caller asked for.
async fn list_faces(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
    user_id: Option<&str>,
) -> Vec<(String, Option<String>)> {
    let mut body = json!({ "CollectionId": collection_id });
    if let Some(user_id) = user_id {
        body["UserId"] = json!(user_id);
    }
    let reply = call(router, "ListFaces", body).await;
    assert_eq!(
        reply.status, 200,
        "case {collection_id}/{user_id:?}: listing the faces must succeed, got {:?}",
        reply.body
    );
    reply
        .list("Faces")
        .iter()
        .map(|face| {
            (
                face.get("FaceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                face.get("UserId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// Every `(UserId, UserStatus)` `ListUsers` reports for `collection_id`.
async fn list_users(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
) -> Vec<(String, String)> {
    let reply = call(router, "ListUsers", json!({ "CollectionId": collection_id })).await;
    assert_eq!(
        reply.status, 200,
        "case {collection_id}: listing the users must succeed, got {:?}",
        reply.body
    );
    reply
        .list("Users")
        .iter()
        .map(|user| {
            (
                user.get("UserId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                user.get("UserStatus")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_disassociation_is_decided_face_by_face_and_read_back_from_the_store() {
    let router = harness();
    create_collection(&router, "people").await;
    create_collection(&router, "others").await;
    create_user(&router, "people", "alice").await;
    create_user(&router, "people", "bob").await;

    // Rows: two faces the named user holds, one of them named TWICE, a face no user holds,
    // a face another user of the collection holds, a well-formed id the service never
    // minted, a face that was indexed and then removed, and a face that belongs to a
    // different collection. One portrait draws them all, so every row is decided by which
    // face it names.
    let held_first = index_face(&router, "people").await;
    let held_second = index_face(&router, "people").await;
    let taken = index_face(&router, "people").await;
    let free = index_face(&router, "people").await;
    let removed = index_face(&router, "people").await;
    let elsewhere = index_face(&router, "others").await;

    associate_faces(
        &router,
        "people",
        "alice",
        &[held_first.clone(), held_second.clone()],
    )
    .await;
    associate_faces(&router, "people", "bob", std::slice::from_ref(&taken)).await;
    let deleted = call(
        &router,
        "DeleteFaces",
        json!({ "CollectionId": "people", "FaceIds": [removed.clone()] }),
    )
    .await;
    assert_eq!(
        deleted.list("DeletedFaces").len(),
        1,
        "the face no user holds must be removed, got {:?}",
        deleted.body
    );

    let requested = vec![
        held_first.clone(),
        held_second.clone(),
        held_second.clone(),
        free.clone(),
        taken.clone(),
        UNMINTED_FACE_ID.to_string(),
        removed.clone(),
        elsewhere.clone(),
    ];
    let reply = disassociate_faces(&router, "people", "alice", &requested, None).await;
    assert_eq!(
        reply.status, 200,
        "a request naming faces of every kind is answered rather than refused, got {:?}",
        reply.body
    );
    assert_eq!(
        disassociated(&reply),
        vec![held_first.clone(), held_second.clone()],
        "case disassociated: only the faces this user held come off it, each reported once, \
         got {:?}",
        reply.body
    );
    assert_eq!(
        unsuccessful(&reply),
        vec![
            (
                taken.clone(),
                Some("bob".to_string()),
                vec![ASSOCIATED_TO_A_DIFFERENT_USER.to_string()],
            ),
            (
                UNMINTED_FACE_ID.to_string(),
                None,
                vec![FACE_NOT_FOUND.to_string()],
            ),
            (removed.clone(), None, vec![FACE_NOT_FOUND.to_string()]),
            (elsewhere.clone(), None, vec![FACE_NOT_FOUND.to_string()]),
        ],
        "case unsuccessful: every face that could not be disassociated is reported once, \
         with the reason the model names for it, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("UserStatus").as_deref(),
        Some(UPDATING),
        "case UserStatus: a call that disassociated a face leaves the user updating, got {:?}",
        reply.body
    );

    // The disassociation is recorded ON the stored face, so every reader answers from it:
    // each face stays in the collection carrying no user, and the `UserId` filter reports
    // it for nobody.
    assert_eq!(
        list_faces(&router, "people", None).await,
        vec![
            (held_first.clone(), None),
            (held_second.clone(), None),
            (taken.clone(), Some("bob".to_string())),
            (free.clone(), None),
        ],
        "case ListFaces: a disassociated face stays in the collection with no user"
    );
    assert_eq!(
        list_faces(&router, "people", Some("alice")).await,
        Vec::new(),
        "case ListFaces(UserId=alice): the user holds none of the faces it was emptied of"
    );
    assert_eq!(
        list_faces(&router, "people", Some("bob")).await,
        vec![(taken.clone(), Some("bob".to_string()))],
        "case ListFaces(UserId=bob): the other user's face is untouched"
    );

    // What the disassociation persisted is also what `DeleteFaces` answers from: a face no
    // user holds any more is free to be removed, while the one still held is not.
    let delete = call(
        &router,
        "DeleteFaces",
        json!({
            "CollectionId": "people",
            "FaceIds": [held_first.clone(), taken.clone()],
        }),
    )
    .await;
    assert_eq!(
        (
            delete.list("DeletedFaces"),
            delete
                .list("UnsuccessfulFaceDeletions")
                .first()
                .and_then(|entry| entry.get("FaceId"))
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        (vec![json!(held_first.clone())], Some(taken.clone())),
        "case DeleteFaces: the disassociated face is removed and the face a user still \
         holds is not, got {:?}",
        delete.body
    );
}

/// One row of the refusal table: the collection and the user a disassociation names, and
/// the class and text the refusal carries.
struct MissingCase {
    case: &'static str,
    collection_id: String,
    user_id: String,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_face_is_disassociated_only_from_a_user_and_a_collection_that_exist() {
    let router = harness();
    create_collection(&router, "people").await;
    create_collection(&router, "gone").await;
    create_user(&router, "people", "alice").await;
    create_user(&router, "people", "removed").await;
    let face = index_face(&router, "people").await;
    associate_faces(&router, "people", "alice", std::slice::from_ref(&face)).await;

    // Rows: a collection id nothing created, one created and then deleted, a user id
    // nothing created, and one created and then deleted — each with the wording this
    // service refuses that half of the identity with.
    let dropped = call(
        &router,
        "DeleteCollection",
        json!({ "CollectionId": "gone" }),
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "the collection to be deleted must go, got {:?}",
        dropped.body
    );
    let dropped_user = call(
        &router,
        "DeleteUser",
        json!({ "CollectionId": "people", "UserId": "removed" }),
    )
    .await;
    assert_eq!(
        dropped_user.status, 200,
        "the user to be deleted must go, got {:?}",
        dropped_user.body
    );

    let cases = vec![
        MissingCase {
            case: "a collection nothing created",
            collection_id: "absent".to_string(),
            user_id: "alice".to_string(),
            code: "ResourceNotFoundException",
            message: "The collection id: absent does not exist".to_string(),
        },
        MissingCase {
            case: "a collection that was deleted",
            collection_id: "gone".to_string(),
            user_id: "alice".to_string(),
            code: "ResourceNotFoundException",
            message: "The collection id: gone does not exist".to_string(),
        },
        MissingCase {
            case: "a user nothing created",
            collection_id: "people".to_string(),
            user_id: "nobody".to_string(),
            code: "ResourceNotFoundException",
            message: "The user id: nobody does not exist".to_string(),
        },
        MissingCase {
            case: "a user that was deleted",
            collection_id: "people".to_string(),
            user_id: "removed".to_string(),
            code: "ResourceNotFoundException",
            message: "The user id: removed does not exist".to_string(),
        },
    ];
    for case in cases {
        let reply = disassociate_faces(
            &router,
            &case.collection_id,
            &case.user_id,
            std::slice::from_ref(&face),
            None,
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.string("Message")),
            (400, case.code.to_string(), Some(case.message.clone())),
            "case {:?}: expected {} / {:?}, got {:?}",
            case.case,
            case.code,
            case.message,
            reply.body
        );
    }

    // A refused request persists nothing: the face the rows named is still held by the
    // user it was associated with.
    assert_eq!(
        list_faces(&router, "people", Some("alice")).await,
        vec![(face.clone(), Some("alice".to_string()))],
        "a refused disassociation leaves the association as it was"
    );
}

#[tokio::test]
async fn a_disassociation_completes_once_per_client_request_token() {
    let router = harness();
    create_collection(&router, "people").await;
    create_user(&router, "people", "alice").await;
    let first = index_face(&router, "people").await;
    let second = index_face(&router, "people").await;
    associate_faces(&router, "people", "alice", &[first.clone(), second.clone()]).await;

    let committed = disassociate_faces(
        &router,
        "people",
        "alice",
        std::slice::from_ref(&first),
        Some("token-one"),
    )
    .await;
    assert_eq!(
        (committed.status, disassociated(&committed)),
        (200, vec![first.clone()]),
        "case first call: the face the user held comes off it, got {:?}",
        committed.body
    );

    // The face is put back, so a second run of the same request would be visible: the
    // token must answer with what it committed and take nothing off again.
    associate_faces(&router, "people", "alice", std::slice::from_ref(&first)).await;
    let retried = disassociate_faces(
        &router,
        "people",
        "alice",
        std::slice::from_ref(&first),
        Some("token-one"),
    )
    .await;
    assert_eq!(
        (
            retried.status,
            disassociated(&retried),
            unsuccessful(&retried),
            retried.string("UserStatus"),
        ),
        (
            200,
            disassociated(&committed),
            unsuccessful(&committed),
            committed.string("UserStatus"),
        ),
        "case retry: the same token answers with the response it committed, got {:?}",
        retried.body
    );
    assert_eq!(
        list_faces(&router, "people", Some("alice")).await,
        vec![
            (first.clone(), Some("alice".to_string())),
            (second.clone(), Some("alice".to_string())),
        ],
        "case retry: the retry takes no face off the user a second time"
    );

    let mismatch = disassociate_faces(
        &router,
        "people",
        "alice",
        std::slice::from_ref(&second),
        Some("token-one"),
    )
    .await;
    assert_eq!(
        (mismatch.status, mismatch.error_code()),
        (400, "IdempotentParameterMismatchException".to_string()),
        "case mismatch: the token committed a different request, got {:?}",
        mismatch.body
    );
    assert_eq!(
        list_faces(&router, "people", Some("alice")).await,
        vec![
            (first.clone(), Some("alice".to_string())),
            (second.clone(), Some("alice".to_string())),
        ],
        "case mismatch: a refused request takes nothing off the user"
    );

    let fresh = disassociate_faces(
        &router,
        "people",
        "alice",
        std::slice::from_ref(&first),
        Some("token-two"),
    )
    .await;
    assert_eq!(
        (fresh.status, disassociated(&fresh)),
        (200, vec![first.clone()]),
        "case fresh token: another token carries the same request out, got {:?}",
        fresh.body
    );
    assert_eq!(
        list_faces(&router, "people", Some("alice")).await,
        vec![(second.clone(), Some("alice".to_string()))],
        "case fresh token: the face named by the fresh token's request comes off the user"
    );
}

#[tokio::test]
async fn a_call_that_disassociates_nothing_leaves_the_user_as_it_was() {
    let router = harness();
    create_collection(&router, "people").await;
    create_user(&router, "people", "carol").await;
    let free = index_face(&router, "people").await;

    // Rows: a face the collection holds that no user holds — the ignored case — and a
    // well-formed id the service never minted. Neither changes the user, so it keeps the
    // status of a user no face has been associated with.
    for (case, face_id, expected_unsuccessful) in [
        ("a face no user holds", free.clone(), Vec::new()),
        (
            "an id the service never minted",
            UNMINTED_FACE_ID.to_string(),
            vec![(
                UNMINTED_FACE_ID.to_string(),
                None,
                vec![FACE_NOT_FOUND.to_string()],
            )],
        ),
    ] {
        let reply = disassociate_faces(
            &router,
            "people",
            "carol",
            std::slice::from_ref(&face_id),
            None,
        )
        .await;
        assert_eq!(
            (
                reply.status,
                disassociated(&reply),
                unsuccessful(&reply),
                reply.string("UserStatus"),
            ),
            (
                200,
                Vec::new(),
                expected_unsuccessful,
                Some(CREATED.to_string()),
            ),
            "case {case:?}: nothing comes off the user and it keeps the status it had, got \
             {:?}",
            reply.body
        );
    }

    assert_eq!(
        list_users(&router, "people").await,
        vec![("carol".to_string(), CREATED.to_string())],
        "the user is still the one a create recorded"
    );
    assert_eq!(
        list_faces(&router, "people", None).await,
        vec![(free.clone(), None)],
        "the face the collection holds is untouched and still held by no user"
    );
}
