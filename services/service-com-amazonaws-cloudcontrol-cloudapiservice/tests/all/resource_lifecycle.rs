// SPDX-License-Identifier: CC0-1.0

//! The six resource operations, end to end over the wire.
//!
//! Every answer here has to come out of what the store holds: a create is only a create
//! if a later `GetResource` reports it, a `RequestToken` is only a token if
//! `GetResourceRequestStatus` resolves it, and a delete is only a delete if the resource
//! stops being readable and stops being listed.

use serde_json::{json, Value};

use super::emulator::{
    code, descriptions, message, progress, text, Emulator, STATUS_FAILED, STATUS_IN_PROGRESS,
    STATUS_SUCCESS,
};
use super::refusal_wording::{assert_handler_not_found, generic_already_exists};

/// A resource type whose primary identifier the caller supplies and whose resources live
/// in THIS service's own store, so every row here is about the operation rather than about
/// one owning service.
const TABLE: &str = "AWS::DynamoDB::Table";

/// A second resource type, so a case can tell one type's resources from another's. Its
/// resources live in the OWNING service's store (`AWS::Logs::LogGroup` is a CloudWatch
/// Logs log group), so what a read reports for one is the model that service holds.
const LOG_GROUP: &str = "AWS::Logs::LogGroup";

/// The model a read reports for the log group `name`: its identity, the class every group
/// carries and the read-only ARN the type's schema declares — rendered by the owning
/// service's own ARN builder, so the case pins the group Cloud Control API resolved to the
/// one CloudWatch Logs holds.
fn log_group_model(name: &str) -> Value {
    json!({
        "LogGroupName": name,
        "LogGroupClass": "STANDARD",
        "Arn": engine::emulator_arn("logs", super::emulator::SIGNING_REGION, &format!("log-group:{name}:*")),
    })
}

/// A type whose model a case grows with a patch, kept apart from the lifecycle rows.
const STREAM: &str = "AWS::Kinesis::Stream";

/// A patch that changes nothing a case cares about, for the rows that are about
/// something other than the patch.
fn inert_patch() -> Value {
    json!([{ "op": "add", "path": "/Tags", "value": [] }])
}

/// The not-found diagnostic the service that OWNS this type states for one of its
/// resources, which every refusal about an absent one of them carries: Amazon DynamoDB
/// names the table it holds none of.
fn table_not_found(identifier: &str) -> String {
    format!("Table: {identifier} does not exist.")
}

#[tokio::test]
async fn the_six_operations_run_end_to_end() {
    let emulator = Emulator::new();
    let desired = json!({
        "TableName": "demo-table",
        "StreamSpecification": { "StreamViewType": "KEYS_ONLY" },
    });

    // CreateResource answers the request it started, not the resource: the operation is
    // still running when the call returns.
    let accepted = emulator
        .create_ok(TABLE, &desired, Some("tok-create"))
        .await;
    let request_token = text(&accepted, "RequestToken");
    assert!(
        !request_token.is_empty(),
        "the create must mint a RequestToken, got {accepted:?}"
    );
    assert_eq!(
        text(&accepted, "TypeName"),
        TABLE,
        "the create must report the type it acted on, got {accepted:?}"
    );
    assert_eq!(
        text(&accepted, "Identifier"),
        "demo-table",
        "the create must report the primary identifier the desired state names, got {accepted:?}"
    );
    assert_eq!(
        text(&accepted, "Operation"),
        "CREATE",
        "the create must report a CREATE operation, got {accepted:?}"
    );
    assert_eq!(
        text(&accepted, "OperationStatus"),
        STATUS_IN_PROGRESS,
        "a create that has not completed must not report SUCCESS, got {accepted:?}"
    );
    assert!(
        accepted["EventTime"].as_f64().is_some(),
        "the create must report when the request was initiated, got {accepted:?}"
    );

    // GetResourceRequestStatus resolves that token, and the record it reports is the
    // same request — including the instant it was initiated.
    let settled = emulator.settled(&request_token).await;
    assert_eq!(
        text(&settled, "RequestToken"),
        request_token,
        "the status call must report the request its token names, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "Operation"),
        "CREATE",
        "the status call must report the operation the request ran, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "Identifier"),
        "demo-table",
        "the status call must report the resource the request acted on, got {settled:?}"
    );
    assert_eq!(
        settled["EventTime"], accepted["EventTime"],
        "EventTime is when the request was INITIATED, so it must not move as the request \
         progresses: create reported {:?}, the status call {:?}",
        accepted["EventTime"], settled["EventTime"]
    );

    // GetResource reports the model that was stored, not the request's blob echoed back.
    let stored = emulator.properties(TABLE, "demo-table").await;
    assert_eq!(
        stored, desired,
        "GetResource must report the stored resource model, expected {desired:?}, got {stored:?}"
    );

    // A second create of the same resource is the OWNING handler's verdict: the request
    // is accepted and then fails with that handler's error code and its own wording.
    let (status, body) = emulator
        .create(TABLE, &desired, Some("tok-create-again"))
        .await;
    assert_eq!(
        status, 200,
        "a second create of the same resource: expected HTTP 200, got {status} with body {body:?}"
    );
    let duplicate = emulator
        .terminal(&text(&progress(&body), "RequestToken"))
        .await;
    assert_eq!(
        text(&duplicate, "OperationStatus"),
        STATUS_FAILED,
        "a second create of the same resource must fail, got {duplicate:?}"
    );
    assert_eq!(
        text(&duplicate, "ErrorCode"),
        "AlreadyExists",
        "a second create of the same resource: unexpected handler error code"
    );
    assert_eq!(
        text(&duplicate, "StatusMessage"),
        generic_already_exists(TABLE, "demo-table"),
        "a second create of the same resource: unexpected wording"
    );
    let stored = emulator.properties(TABLE, "demo-table").await;
    assert_eq!(
        stored, desired,
        "the failed create must leave the resource as it was, got {stored:?}"
    );

    // UpdateResource applies the patch to the stored model, and a later read shows it.
    let patch = json!([
        { "op": "replace", "path": "/StreamSpecification/StreamViewType", "value": "NEW_IMAGE" },
        { "op": "add", "path": "/Tags", "value": [{ "Key": "team", "Value": "web" }] },
    ]);
    let updated = emulator.update_and_settle(TABLE, "demo-table", &patch).await;
    assert_eq!(
        text(&updated, "Operation"),
        "UPDATE",
        "the update must report an UPDATE operation, got {updated:?}"
    );
    assert_eq!(
        text(&updated, "OperationStatus"),
        STATUS_SUCCESS,
        "the update must reach SUCCESS, got {updated:?}"
    );
    let patched = emulator.properties(TABLE, "demo-table").await;
    let expected = json!({
        "TableName": "demo-table",
        "StreamSpecification": { "StreamViewType": "NEW_IMAGE" },
        "Tags": [{ "Key": "team", "Value": "web" }],
    });
    assert_eq!(
        patched, expected,
        "GetResource after the update: expected {expected:?}, got {patched:?}"
    );
    let reported: Value =
        serde_json::from_str(&text(&updated, "ResourceModel")).unwrap_or(Value::Null);
    assert_eq!(
        reported, patched,
        "the update's own ResourceModel must be the model it left behind, expected \
         {patched:?}, got {reported:?}"
    );

    // ListResources reports the resource that is actually persisted.
    let walked = emulator.walk(TABLE, None).await;
    assert_eq!(
        walked,
        vec!["demo-table".to_string()],
        "the listing must report the stored resource, got {walked:?}"
    );

    // DeleteResource removes it, and the reads agree.
    let deleted = emulator.delete_and_settle(TABLE, "demo-table").await;
    assert_eq!(
        text(&deleted, "Operation"),
        "DELETE",
        "the delete must report a DELETE operation, got {deleted:?}"
    );
    assert_eq!(
        text(&deleted, "OperationStatus"),
        STATUS_SUCCESS,
        "the delete must reach SUCCESS, got {deleted:?}"
    );
    let (status, body) = emulator.get(TABLE, "demo-table").await;
    assert_eq!(
        status, 404,
        "GetResource after the delete: expected HTTP 404, got {status} with body {body:?}"
    );
    assert!(
        code(&body).contains("ResourceNotFoundException"),
        "GetResource after the delete: expected ResourceNotFoundException, got {:?}",
        code(&body)
    );
    assert_handler_not_found(
        &message(&body),
        TABLE,
        &table_not_found("demo-table"),
        "GetResource after the delete",
    );
    let walked = emulator.walk(TABLE, None).await;
    assert!(
        walked.is_empty(),
        "the listing after the delete must report nothing, got {walked:?}"
    );

    // Every request this run started stays resolvable by its own token.
    let create_record = emulator.settled(&request_token).await;
    assert_eq!(
        text(&create_record, "Operation"),
        "CREATE",
        "the create's record must still resolve after the resource is gone, got \
         {create_record:?}"
    );
}

#[tokio::test]
async fn a_resource_operation_is_in_progress_before_it_completes() {
    let emulator = Emulator::new();
    let desired = json!({ "LogGroupName": "app-logs" });
    let accepted = emulator.create_ok(LOG_GROUP, &desired, None).await;
    let request_token = text(&accepted, "RequestToken");
    assert_eq!(
        text(&accepted, "OperationStatus"),
        STATUS_IN_PROGRESS,
        "an accepted create must report IN_PROGRESS, got {accepted:?}"
    );
    assert!(
        accepted["RetryAfter"].as_f64().is_some(),
        "an in-progress request must say when to ask again, got {accepted:?}"
    );

    // The resource is not provisioned yet, so neither read reports it.
    let (status, body) = emulator.get(LOG_GROUP, "app-logs").await;
    assert_eq!(
        status, 404,
        "GetResource during the create: expected HTTP 404, got {status} with body {body:?}"
    );
    assert!(
        code(&body).contains("ResourceNotFoundException"),
        "GetResource during the create: expected ResourceNotFoundException, got {:?}",
        code(&body)
    );
    let walked = emulator.walk(LOG_GROUP, None).await;
    assert!(
        walked.is_empty(),
        "the listing during the create must report nothing, got {walked:?}"
    );

    // A second operation against the same resource is refused while the first runs.
    let rows = [
        (
            "a second create",
            emulator.create(LOG_GROUP, &desired, None).await,
        ),
        (
            "an update",
            emulator
                .update(LOG_GROUP, "app-logs", &inert_patch(), None)
                .await,
        ),
        ("a delete", emulator.delete(LOG_GROUP, "app-logs").await),
    ];
    for (case, (status, body)) in rows {
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ConcurrentOperationException"),
            "case {case:?}: expected ConcurrentOperationException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!(
                "Concurrent operation found for resource app-logs with RequestToken \
                 {request_token}"
            ),
            "case {case:?}: unexpected wording"
        );
    }

    // Then the clock makes the work due and the resource is there.
    let settled = emulator.settled(&request_token).await;
    assert!(
        settled["RetryAfter"].is_null(),
        "a completed request has nothing left to ask again for, got {settled:?}"
    );
    // The resource lives in the owning service's store, so the model reported is the one
    // CloudWatch Logs holds for the group this create provisioned.
    let stored = emulator.properties(LOG_GROUP, "app-logs").await;
    let expected = log_group_model("app-logs");
    assert_eq!(
        stored, expected,
        "GetResource after the create: expected {expected:?}, got {stored:?}"
    );
}

#[tokio::test]
async fn a_client_token_replays_its_request_and_refuses_a_different_one() {
    let emulator = Emulator::new();
    let desired = json!({ "TableName": "token-table" });

    let first = emulator.create_ok(TABLE, &desired, Some("tok-1")).await;
    let request_token = text(&first, "RequestToken");
    let replay = emulator.create_ok(TABLE, &desired, Some("tok-1")).await;
    assert_eq!(
        text(&replay, "RequestToken"),
        request_token,
        "an identical retry must replay the first request, got {replay:?}"
    );

    emulator.settled(&request_token).await;
    let walked = emulator.walk(TABLE, None).await;
    assert_eq!(
        walked,
        vec!["token-table".to_string()],
        "a replayed client token must not mint a second resource, got {walked:?}"
    );

    // Replaying after the request completed reports the status it has reached.
    let late_replay = emulator.create_ok(TABLE, &desired, Some("tok-1")).await;
    assert_eq!(
        text(&late_replay, "RequestToken"),
        request_token,
        "a late retry must still replay the first request, got {late_replay:?}"
    );
    assert_eq!(
        text(&late_replay, "OperationStatus"),
        STATUS_SUCCESS,
        "a late retry must report the status the request has reached, got {late_replay:?}"
    );

    // The same token for a DIFFERENT request is the conflict, on every mutating
    // operation that takes one.
    let conflicts = [
        (
            "a create with another desired state",
            emulator
                .create(TABLE, &json!({ "TableName": "other-table" }), Some("tok-1"))
                .await,
        ),
        (
            "a create of another type",
            emulator
                .create(LOG_GROUP, &json!({ "LogGroupName": "app-logs" }), Some("tok-1"))
                .await,
        ),
    ];
    for (case, (status, body)) in conflicts {
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ClientTokenConflictException"),
            "case {case:?}: expected ClientTokenConflictException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!(
                "ClientToken tok-1 is already associated with an existing operation with \
                 RequestToken {request_token}"
            ),
            "case {case:?}: unexpected wording"
        );
    }

    // An update's token obeys the same rule.
    let (status, body) = emulator
        .update(TABLE, "token-table", &inert_patch(), Some("tok-2"))
        .await;
    assert_eq!(
        status, 200,
        "the first update: expected HTTP 200, got {status} with body {body:?}"
    );
    let update_token = text(&progress(&body), "RequestToken");
    let (status, body) = emulator
        .update(TABLE, "token-table", &inert_patch(), Some("tok-2"))
        .await;
    assert_eq!(
        status, 200,
        "the retried update: expected HTTP 200, got {status} with body {body:?}"
    );
    assert_eq!(
        text(&progress(&body), "RequestToken"),
        update_token,
        "an identical retried update must replay the first one, got {body:?}"
    );
    let (status, body) = emulator
        .update(
            TABLE,
            "token-table",
            &json!([{ "op": "add", "path": "/Tags", "value": [{ "Key": "a", "Value": "b" }] }]),
            Some("tok-2"),
        )
        .await;
    assert_eq!(
        status, 409,
        "an update reusing the token for another patch: expected HTTP 409, got {status} with \
         body {body:?}"
    );
    assert!(
        code(&body).contains("ClientTokenConflictException"),
        "an update reusing the token for another patch: expected ClientTokenConflictException, \
         got {:?}",
        code(&body)
    );
    emulator.settled(&update_token).await;

    // And a delete's token does too.
    let (status, body) = emulator
        .delete_with_token(TABLE, "token-table", Some("tok-3"))
        .await;
    assert_eq!(
        status, 200,
        "the first delete: expected HTTP 200, got {status} with body {body:?}"
    );
    let delete_token = text(&progress(&body), "RequestToken");
    let (status, body) = emulator
        .delete_with_token(TABLE, "token-table", Some("tok-3"))
        .await;
    assert_eq!(
        status, 200,
        "the retried delete: expected HTTP 200, got {status} with body {body:?}"
    );
    assert_eq!(
        text(&progress(&body), "RequestToken"),
        delete_token,
        "an identical retried delete must replay the first one, got {body:?}"
    );
}

#[tokio::test]
async fn unknown_identifiers_and_tokens_are_the_modelled_refusals() {
    let emulator = Emulator::new();

    // Nothing has been created, so every SYNCHRONOUS operation that names a resource
    // refuses.
    let rows = [
        ("GetResource", emulator.get(TABLE, "absent-table").await),
        (
            "UpdateResource",
            emulator
                .update(TABLE, "absent-table", &inert_patch(), None)
                .await,
        ),
    ];
    for (case, (status, body)) in rows {
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
        assert_handler_not_found(
            &message(&body),
            TABLE,
            &table_not_found("absent-table"),
            case,
        );
    }

    // The delete is asynchronous: it is ADMITTED and the same verdict arrives on the
    // request record it minted, which is the token a caller polls.
    let (status, body) = emulator.delete(TABLE, "absent-table").await;
    assert_eq!(
        status, 200,
        "DeleteResource of an absent resource: expected HTTP 200, got {status} with body \
         {body:?}"
    );
    let accepted = progress(&body);
    assert_eq!(
        text(&accepted, "Identifier"),
        "absent-table",
        "DeleteResource must report the identifier the request carried, got {accepted:?}"
    );
    let failed = emulator.terminal(&text(&accepted, "RequestToken")).await;
    assert_eq!(
        text(&failed, "OperationStatus"),
        STATUS_FAILED,
        "DeleteResource of an absent resource must fail, got {failed:?}"
    );
    assert_eq!(
        text(&failed, "StatusMessage"),
        table_not_found("absent-table"),
        "DeleteResource of an absent resource: unexpected handler diagnostic"
    );
    assert_eq!(
        text(&failed, "ErrorCode"),
        "NotFound",
        "DeleteResource of an absent resource: unexpected handler error code in {failed:?}"
    );

    // A request token this service never minted names no request at all, and the refusal
    // names the token it was handed.
    for token in ["bogus", "00000000-0000-0000-0000-000000000000", "AAAA="] {
        let (status, body) = emulator.status(token).await;
        assert_eq!(
            status, 404,
            "token {token:?}: expected HTTP 404, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("RequestTokenNotFoundException"),
            "token {token:?}: expected RequestTokenNotFoundException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Request with token {token} was not found"),
            "token {token:?}: unexpected wording"
        );
    }

    // A type nothing was created under lists nothing; that is not a refusal.
    let listed = emulator.list("AWS::SQS::Queue", None, None).await;
    assert!(
        descriptions(&listed).is_empty(),
        "a type holding no resource must list nothing, got {listed:?}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "an empty listing is the last page, got {listed:?}"
    );
}

#[tokio::test]
async fn a_desired_state_that_is_not_a_resource_model_is_refused() {
    let emulator = Emulator::new();
    // The rows differ along what the rule generalises over: how the blob fails to be a
    // resource model (unparseable, parseable but not an object) and how the property
    // carrying the primary identifier fails to name one.
    let rows: [(&str, &str, &str); 8] = [
        ("not JSON at all", "not json", "DesiredState is not valid JSON"),
        (
            "a truncated object",
            "{\"TableName\":",
            "DesiredState is not valid JSON",
        ),
        (
            "a JSON list",
            "[1,2,3]",
            "DesiredState must be a JSON object of resource properties",
        ),
        (
            "a bare string",
            "\"demo-table\"",
            "DesiredState must be a JSON object of resource properties",
        ),
        (
            "a bare number",
            "7",
            "DesiredState must be a JSON object of resource properties",
        ),
        (
            "a bare null",
            "null",
            "DesiredState must be a JSON object of resource properties",
        ),
        (
            "an identifier property holding an object",
            "{\"TableName\":{}}",
            "TableName must be a non-empty scalar of at most 1024 characters to identify the \
             resource",
        ),
        (
            "an identifier property holding the empty string",
            "{\"TableName\":\"\"}",
            "TableName must be a non-empty scalar of at most 1024 characters to identify the \
             resource",
        ),
    ];
    for (case, desired_state, detail) in rows {
        let (status, body) = emulator
            .cc(
                "CreateResource",
                json!({ "TypeName": TABLE, "DesiredState": desired_state }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Invalid request provided: {detail}"),
            "case {case:?}: unexpected wording"
        );
    }
    let walked = emulator.walk(TABLE, None).await;
    assert!(
        walked.is_empty(),
        "a refused create must store nothing, got {walked:?}"
    );
}

#[tokio::test]
async fn an_update_applies_the_patch_and_refuses_one_that_cannot_be() {
    let emulator = Emulator::new();
    let identifier = emulator
        .provision(
            TABLE,
            &json!({
                "TableName": "patch-table",
                "Tags": [],
                "PointInTimeRecoverySpecification": { "PointInTimeRecoveryEnabled": true },
            }),
        )
        .await;

    // The rows differ along the patch operation RFC 6902 defines: every one of the six,
    // and a document carrying several at once.
    let rows: [(&str, Value, Value, &[&str]); 7] = [
        (
            "add a new property",
            json!([{ "op": "add", "path": "/StreamSpecification", "value": { "StreamViewType": "NEW_IMAGE" } }]),
            json!({ "StreamSpecification": { "StreamViewType": "NEW_IMAGE" } }),
            &[],
        ),
        (
            "replace a nested value",
            json!([{ "op": "replace", "path": "/PointInTimeRecoverySpecification/PointInTimeRecoveryEnabled", "value": false }]),
            json!({ "PointInTimeRecoverySpecification": { "PointInTimeRecoveryEnabled": false } }),
            &[],
        ),
        (
            "append to a list",
            json!([{ "op": "add", "path": "/Tags/-", "value": { "Key": "team", "Value": "web" } }]),
            json!({ "Tags": [{ "Key": "team", "Value": "web" }] }),
            &[],
        ),
        (
            "copy a value",
            json!([{ "op": "copy", "from": "/TableName", "path": "/Alias" }]),
            json!({ "Alias": "patch-table" }),
            &[],
        ),
        (
            "move a value",
            json!([{ "op": "move", "from": "/Alias", "path": "/Nickname" }]),
            json!({ "Nickname": "patch-table" }),
            &["Alias"],
        ),
        (
            "remove a property",
            json!([{ "op": "remove", "path": "/Nickname" }]),
            json!({}),
            &["Nickname"],
        ),
        (
            "a document carrying several operations",
            json!([
                { "op": "test", "path": "/TableName", "value": "patch-table" },
                { "op": "add", "path": "/SSESpecification", "value": { "SSEEnabled": true } },
                { "op": "add", "path": "/Tags/-", "value": { "Key": "owner", "Value": "platform" } },
            ]),
            json!({
                "SSESpecification": { "SSEEnabled": true },
                "Tags": [
                    { "Key": "team", "Value": "web" },
                    { "Key": "owner", "Value": "platform" },
                ],
            }),
            &[],
        ),
    ];
    for (case, patch, present, absent) in rows {
        emulator.update_and_settle(TABLE, &identifier, &patch).await;
        let stored = emulator.properties(TABLE, &identifier).await;
        for (property, wanted) in present.as_object().cloned().unwrap_or_default() {
            assert_eq!(
                stored.get(&property),
                Some(&wanted),
                "case {case:?}: expected {property} to be {wanted:?}, got {stored:?}"
            );
        }
        for property in absent {
            assert_eq!(
                stored.get(*property),
                None,
                "case {case:?}: expected {property} to be gone, got {stored:?}"
            );
        }
        assert_eq!(
            stored.get("TableName"),
            Some(&json!("patch-table")),
            "case {case:?}: the primary identifier must survive every patch, got {stored:?}"
        );
    }

    // The rows a patch is refused on, each leaving the stored model untouched. The code
    // is part of the row: a document the FRONT DOOR will not read, or one naming a path
    // the resource model does not have, never reaches a handler, while a verdict on the
    // model the handler would have been asked to store is the handler's own.
    let before = emulator.properties(TABLE, &identifier).await;
    let refusals: [(&str, &str, &str, String); 7] = [
        (
            "a patch document that is not JSON",
            "not json",
            "ValidationException",
            "Invalid PatchDocument: A JSONArray text must start with '[' at 1 \
             [character 2 line 1]"
                .to_string(),
        ),
        (
            "a patch document that is not a list of operations",
            "{\"op\":\"add\",\"path\":\"/A\",\"value\":1}",
            "ValidationException",
            "Invalid PatchDocument: A JSONArray text must start with '[' at 1 \
             [character 2 line 1]"
                .to_string(),
        ),
        (
            "a patch operation RFC 6902 does not define",
            "[{\"op\":\"frobnicate\",\"path\":\"/A\"}]",
            "ValidationException",
            "Invalid PatchDocument: the document carries an operation that is not a JSON \
             Patch operation"
                .to_string(),
        ),
        (
            "a replace of a path the model does not carry",
            "[{\"op\":\"replace\",\"path\":\"/Missing/Deeper\",\"value\":1}]",
            "ValidationException",
            "Invalid PatchDocument: the replace operation at path '/Missing/Deeper' could \
             not be applied to the resource model"
                .to_string(),
        ),
        (
            "a remove of a path the model does not carry",
            "[{\"op\":\"remove\",\"path\":\"/Missing\"}]",
            "ValidationException",
            "Invalid PatchDocument: the remove operation at path '/Missing' could not be \
             applied to the resource model"
                .to_string(),
        ),
        (
            "a test operation that does not hold",
            "[{\"op\":\"test\",\"path\":\"/TableName\",\"value\":\"other\"}]",
            "ValidationException",
            "Invalid PatchDocument: the test operation at path '/TableName' could not be \
             applied to the resource model"
                .to_string(),
        ),
        (
            "a patch that would leave something other than a resource model",
            "[{\"op\":\"replace\",\"path\":\"\",\"value\":[]}]",
            "InvalidRequestException",
            "Invalid request provided: the patch document would leave a resource model that \
             is not a JSON object"
                .to_string(),
        ),
    ];
    for (case, patch_document, expected_code, expected_message) in refusals {
        let (status, body) = emulator
            .cc(
                "UpdateResource",
                json!({
                    "TypeName": TABLE,
                    "Identifier": identifier,
                    "PatchDocument": patch_document,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            expected_message,
            "case {case:?}: unexpected wording"
        );
        let after = emulator.properties(TABLE, &identifier).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must change nothing, expected {before:?}, got \
             {after:?}"
        );
    }
}

#[tokio::test]
async fn a_patch_cannot_grow_the_resource_model_without_bound() {
    let emulator = Emulator::new();
    // A model with a few kilobytes in it, so a patch that duplicates it crosses the
    // modelled `Properties` maximum in a handful of operations.
    let blob = "x".repeat(4096);
    let desired = json!({ "StreamName": "amplified", "Blob": blob });
    let identifier = emulator.provision(STREAM, &desired).await;

    // The rows differ along how the patch tries to drive work out of proportion to its
    // own size: duplicating the whole model repeatedly (each `copy` of the root roughly
    // doubles it), duplicating one growing property, and simply carrying more operations
    // than a patch may.
    let doubling: Vec<Value> = (1..=8)
        .map(|step| json!({ "op": "copy", "from": "", "path": format!("/Copy{step}") }))
        .collect();
    let growing: Vec<Value> = [
        json!({ "op": "add", "path": "/Nest", "value": {} }),
        json!({ "op": "copy", "from": "/Blob", "path": "/Nest/Seed" }),
    ]
    .into_iter()
    .chain(
        // Each copy of the nest into a child of itself doubles the nest, so eight of
        // them describe a model hundreds of times the size of the patch that asks for it.
        (1..=8).map(|step| {
            json!({ "op": "copy", "from": "/Nest", "path": format!("/Nest/Deep{step}") })
        }),
    )
    .collect();
    let over_the_count: Vec<Value> = (0..=100)
        .map(|step| json!({ "op": "add", "path": format!("/Added{step}"), "value": step }))
        .collect();

    let rows: [(&str, Value, &str); 3] = [
        (
            "copies of the whole model",
            Value::Array(doubling),
            "the patch document would leave a resource model longer than 262144 characters",
        ),
        (
            "copies of a nest into itself",
            Value::Array(growing),
            "the patch document would leave a resource model longer than 262144 characters",
        ),
        (
            "more operations than a patch may carry",
            Value::Array(over_the_count),
            "PatchDocument must carry at most 100 patch operations",
        ),
    ];
    for (case, patch, detail) in rows {
        let (status, body) = emulator.update(STREAM, &identifier, &patch, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Invalid request provided: {detail}"),
            "case {case:?}: unexpected wording"
        );
        let stored = emulator.properties(STREAM, &identifier).await;
        assert_eq!(
            stored, desired,
            "case {case:?}: a refused update must change nothing"
        );
    }

    // And a patch at the operation count bound is still applied.
    let at_the_bound: Vec<Value> = (0..100)
        .map(|step| json!({ "op": "add", "path": format!("/At{step}"), "value": step }))
        .collect();
    emulator
        .update_and_settle(STREAM, &identifier, &Value::Array(at_the_bound))
        .await;
    let stored = emulator.properties(STREAM, &identifier).await;
    assert_eq!(
        stored.get("At99"),
        Some(&json!(99)),
        "a patch of exactly 100 operations must be applied, got {stored:?}"
    );
}

#[tokio::test]
async fn a_patch_cannot_change_the_primary_identifier() {
    let emulator = Emulator::new();
    let identifier = emulator
        .provision(TABLE, &json!({ "TableName": "create-only-table" }))
        .await;

    // The primary identifier is what every operation addresses the resource by, so it is
    // create-only: changing it, dropping it, and changing its type are all refused.
    let rows = [
        (
            "replacing it",
            json!([{ "op": "replace", "path": "/TableName", "value": "renamed-table" }]),
        ),
        (
            "removing it",
            json!([{ "op": "remove", "path": "/TableName" }]),
        ),
        (
            "replacing it with a value that cannot be an identifier",
            json!([{ "op": "replace", "path": "/TableName", "value": { "Nested": true } }]),
        ),
    ];
    for (case, patch) in rows {
        let (status, body) = emulator.update(TABLE, &identifier, &patch, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("NotUpdatableException"),
            "case {case:?}: expected NotUpdatableException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            "Invalid patch update: createOnlyProperties [/properties/TableName] cannot be \
             updated",
            "case {case:?}: unexpected wording"
        );
        let stored = emulator.properties(TABLE, &identifier).await;
        assert_eq!(
            stored,
            json!({ "TableName": "create-only-table" }),
            "case {case:?}: a refused update must change nothing, got {stored:?}"
        );
    }
}
