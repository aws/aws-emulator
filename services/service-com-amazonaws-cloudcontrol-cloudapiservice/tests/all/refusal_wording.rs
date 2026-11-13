// SPDX-License-Identifier: CC0-1.0

//! How Cloud Control API words and classifies a refusal.
//!
//! Every suite here is about a RULE rather than about one request, so each one is a
//! table whose rows differ along what the rule generalises over — which resource type
//! owns the resource, which operation refused, which member the request got wrong, and
//! which half of the service judged it:
//!
//! * Cloud Control is a facade over a resource type's handler, so a refusal that reports
//!   a handler verdict carries that handler's whole answer: its own diagnostic, the
//!   `HandlerErrorCode` it failed with and the `RequestToken` of the invocation that
//!   failed. A `ProgressEvent` has a member for each of those; a synchronous exception
//!   has only its `Message`, which is why the message is where the whole verdict goes.
//! * A refusal about CONTENTION names what is contended — the resource, and the request
//!   that holds it — so a caller can poll the operation that blocked it.
//! * A refusal about a request's own input is the FRONT DOOR's (`ValidationException`),
//!   while `InvalidRequestException` is modelled as a verdict the HANDLER returned about
//!   the input; the two mean different things to a client, so a request that never
//!   reached a handler is not reported as one that did.
//! * A resource type is found in the CloudFormation registry before anything that names
//!   it is resolved, so an unregistered `TypeName` is never answered as a resource that
//!   does not exist or as an empty inventory.

use serde_json::{json, Value};

use super::emulator::{code, descriptions, message, progress, text, Emulator, STATUS_FAILED};

/// A type whose primary identifier the caller supplies, and whose handler states a
/// not-found in Cloud Control's own generic terms.
const PARAMETER: &str = "AWS::SSM::Parameter";

/// A queue type, whose own service words both a not-found and an already-exists in its
/// own terms.
const QUEUE: &str = "AWS::SQS::Queue";

/// A topic type, whose handler carries its client's rendering of SNS's own exception.
const TOPIC: &str = "AWS::SNS::Topic";

/// An alarm type, whose handler states CloudWatch's own sentence.
const ALARM: &str = "AWS::CloudWatch::Alarm";

/// A bucket type, whose own service's sentence for a bucket it holds none of names no
/// resource at all — the third shape a handler diagnostic takes.
const BUCKET: &str = "AWS::S3::Bucket";

/// A stream type, for a row that needs a second type whose handler states Cloud Control's
/// generic sentence.
const STREAM: &str = "AWS::Kinesis::Stream";

/// A table type, whose own service's sentence for a resource it holds none of NAMES that
/// resource.
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";

/// A patch that changes nothing, for the rows that are about something other than the
/// patch.
fn inert_patch() -> Value {
    json!([{ "op": "add", "path": "/Tags", "value": [] }])
}

/// What a case knows about the handler diagnostic inside a refusal.
enum Detail {
    /// Every character of it is fixed text, or a value taken from the request.
    Exact(String),
    /// It is fixed text around a value the owning service mints per call, so the case
    /// pins the text on both sides and requires a value between them.
    Framing(String, String),
}

impl Detail {
    /// Check `stated` against what this case knows.
    fn check(&self, stated: &str, case: &str) {
        match self {
            Self::Exact(expected) => assert_eq!(
                stated, expected,
                "case {case:?}: expected the handler diagnostic {expected:?}, got {stated:?}"
            ),
            Self::Framing(opening, closing) => {
                assert!(
                    stated.starts_with(opening.as_str())
                        && stated.ends_with(closing.as_str())
                        && stated.len() > opening.len() + closing.len(),
                    "case {case:?}: expected a handler diagnostic between {opening:?} and \
                     {closing:?}, got {stated:?}"
                );
            }
        }
    }
}

/// The fixed text a handler-wrapped refusal brackets the handler's verdict with.
const WRAPPER_OPENING: &str = " Handler returned status FAILED: ";

/// The fixed text that introduces the handler error code.
const WRAPPER_CODE: &str = " (HandlerErrorCode: ";

/// The fixed text that introduces the token of the handler invocation that failed.
const WRAPPER_TOKEN: &str = ", RequestToken: ";

/// The three facts a handler-wrapped refusal carries, taken apart at the fixed text that
/// brackets them: the owning handler's diagnostic, the `HandlerErrorCode` it failed with
/// and the `RequestToken` of the invocation that failed.
///
/// A case asserts on each of the three rather than on one opaque sentence, so a refusal
/// that drops any of them fails here.
fn handler_failure(stated: &str, type_name: &str, case: &str) -> (String, String, String) {
    let opening = format!("{type_name}{WRAPPER_OPENING}");
    let body = stated
        .strip_prefix(&opening)
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a message opening {opening:?} and closing \")\", got {stated:?}")
        });
    let (detail, verdict) = body.rsplit_once(WRAPPER_CODE).unwrap_or_else(|| {
        panic!("case {case:?}: expected the message to name the handler error code after {WRAPPER_CODE:?}, got {stated:?}")
    });
    let (handler_error_code, request_token) = verdict.split_once(WRAPPER_TOKEN).unwrap_or_else(|| {
        panic!("case {case:?}: expected the message to name the failing request after {WRAPPER_TOKEN:?}, got {stated:?}")
    });
    assert!(
        !request_token.is_empty()
            && request_token
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-+/=".contains(character)),
        "case {case:?}: expected a RequestToken of the modelled form, got {request_token:?}"
    );
    (
        detail.to_string(),
        handler_error_code.to_string(),
        request_token.to_string(),
    )
}

/// Check that `stated` is the handler-wrapped not-found refusal for `type_name` carrying
/// `detail`, answering the request token it names.
pub(crate) fn assert_handler_not_found(
    stated: &str,
    type_name: &str,
    detail: &str,
    case: &str,
) -> String {
    let (stated_detail, handler_error_code, request_token) =
        handler_failure(stated, type_name, case);
    Detail::Exact(detail.to_string()).check(&stated_detail, case);
    assert_eq!(
        handler_error_code, "NotFound",
        "case {case:?}: expected the NotFound handler error code, got {handler_error_code:?}"
    );
    request_token
}

/// Cloud Control's own generic not-found diagnostic, for a type whose handler states one.
///
/// The resource is named by its PRIMARY IDENTIFIER: a type whose `primaryIdentifier` names
/// one property is addressed by that property's value, so the sentence carries that value
/// as the request and every answer spell it.
pub(crate) fn generic_not_found(type_name: &str, identifier: &str) -> String {
    format!("Resource of type '{type_name}' with identifier '{identifier}' was not found.")
}

/// Cloud Control's own generic already-exists diagnostic, which names the resource the
/// same way ([`generic_not_found`]).
pub(crate) fn generic_already_exists(type_name: &str, identifier: &str) -> String {
    format!("Resource of type '{type_name}' with identifier '{identifier}' already exists.")
}

#[tokio::test]
async fn a_not_found_refusal_carries_the_owning_handlers_whole_verdict() {
    let emulator = Emulator::new();
    // The rows differ along the OWNING SERVICE's wording: a type whose handler states
    // Cloud Control's generic sentence, one whose service has a sentence of its own, one
    // whose sentence NAMES the resource the request asked about, one whose sentence names
    // nothing, and one whose handler carries its client's rendering of that service's
    // exception (which names a request id the service mints per call).
    let rows: [(&str, &str, &str, Detail); 6] = [
        (
            "a type whose handler states the generic sentence",
            PARAMETER,
            "absent-parameter",
            Detail::Exact(generic_not_found(PARAMETER, "absent-parameter")),
        ),
        (
            "a second such type, so the sentence is built from the request",
            STREAM,
            "absent-stream",
            Detail::Exact(generic_not_found(STREAM, "absent-stream")),
        ),
        (
            "a type whose own service has a sentence for it",
            ALARM,
            "absent-alarm",
            Detail::Exact("Alarm does not exist".to_string()),
        ),
        (
            "a type whose service's sentence names the resource",
            DYNAMODB_TABLE,
            "absent-table",
            Detail::Exact("Table: absent-table does not exist.".to_string()),
        ),
        (
            "a type whose service's sentence names nothing",
            BUCKET,
            "absent-bucket",
            Detail::Exact("Bucket not found".to_string()),
        ),
        (
            "a type whose handler carries its client's rendering",
            TOPIC,
            "arn:aws:sns:us-east-1:123456789012:absent-topic",
            Detail::Framing(
                "Topic does not exist (Service: Sns, Status Code: 404, Request ID: ".to_string(),
                ")".to_string(),
            ),
        ),
    ];

    for (case, type_name, identifier, detail) in rows {
        // Both SYNCHRONOUS resource-level operations refuse the same way: the verdict
        // belongs to the type's handler, not to the operation that asked it.
        let answers = [
            ("GetResource", emulator.get(type_name, identifier).await),
            (
                "UpdateResource",
                emulator
                    .update(type_name, identifier, &inert_patch(), None)
                    .await,
            ),
        ];
        let mut tokens = Vec::new();
        for (operation, (status, body)) in answers {
            let case = format!("{case} ({operation})");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                code(&body)
            );
            let (stated_detail, handler_error_code, request_token) =
                handler_failure(&message(&body), type_name, &case);
            detail.check(&stated_detail, &case);
            assert_eq!(
                handler_error_code, "NotFound",
                "case {case:?}: expected the NotFound handler error code, got \
                 {handler_error_code:?}"
            );
            tokens.push(request_token);
        }
        // Each refusal reports the invocation THAT call made, so no two of them name the
        // same request.
        tokens.sort();
        let distinct = tokens.len();
        tokens.dedup();
        assert_eq!(
            tokens.len(),
            distinct,
            "case {case:?}: each refusal must name its own failing request, got {tokens:?}"
        );

        // The delete is ASYNCHRONOUS, so the same verdict is reported on the request
        // record it minted: the handler's own diagnostic in `StatusMessage` and the
        // handler error code in `ErrorCode`, each in its own member.
        let case = format!("{case} (DeleteResource)");
        let (status, body) = emulator.delete(type_name, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body:?}"
        );
        let failed = emulator
            .terminal(&text(&progress(&body), "RequestToken"))
            .await;
        assert_eq!(
            text(&failed, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: the delete of an absent resource must fail, got {failed:?}"
        );
        detail.check(&text(&failed, "StatusMessage"), &case);
        assert_eq!(
            text(&failed, "ErrorCode"),
            "NotFound",
            "case {case:?}: unexpected handler error code in {failed:?}"
        );
    }
}

#[tokio::test]
async fn a_contended_resource_is_refused_naming_the_resource_and_the_holding_request() {
    let emulator = Emulator::new();
    // One create is left running, so every operation below arrives while a request holds
    // the resource. The rows differ along which operation contends for it.
    let identifier = "contended-parameter";
    let desired = json!({ "Name": identifier, "Type": "String", "Value": "v" });
    let holding = text(
        &emulator.create_ok(PARAMETER, &desired, None).await,
        "RequestToken",
    );

    let rows = [
        (
            "a second create",
            emulator.create(PARAMETER, &desired, None).await,
        ),
        (
            "an update",
            emulator
                .update(PARAMETER, identifier, &inert_patch(), None)
                .await,
        ),
        ("a delete", emulator.delete(PARAMETER, identifier).await),
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
            format!("Concurrent operation found for resource {identifier} with RequestToken {holding}"),
            "case {case:?}: unexpected wording"
        );
    }
    emulator.settled(&holding).await;
}

#[tokio::test]
async fn a_reused_client_token_is_refused_naming_the_token_and_the_request_it_first_named() {
    let emulator = Emulator::new();
    // The rows differ along which mutating operation first used the token, and along
    // the member whose change makes the second request a different one: the desired
    // state, the type, the identifier, the patch.
    let queue = json!({ "QueueName": "token-queue" });
    let parameter = json!({ "Name": "token-parameter", "Type": "String", "Value": "v" });
    let _ = emulator.provision(QUEUE, &queue).await;
    let _ = emulator.provision(PARAMETER, &parameter).await;
    // Its own resource, so the delete row contends with nothing another row started.
    let deletable = emulator
        .provision(QUEUE, &json!({ "QueueName": "token-queue-to-delete" }))
        .await;

    let rows: [(&str, &str, Value, Value); 3] = [
        (
            "a create whose desired state differs",
            "ct-create",
            Emulator::create_request(QUEUE, &queue, Some("ct-create")),
            Emulator::create_request(
                QUEUE,
                &json!({ "QueueName": "other-queue" }),
                Some("ct-create"),
            ),
        ),
        (
            "a create of another type",
            "ct-type",
            Emulator::create_request(PARAMETER, &parameter, Some("ct-type")),
            Emulator::create_request(BUCKET, &json!({ "BucketName": "b" }), Some("ct-type")),
        ),
        (
            "a delete of another resource",
            "ct-delete",
            json!({
                "TypeName": QUEUE,
                "Identifier": deletable,
                "ClientToken": "ct-delete",
            }),
            json!({
                "TypeName": QUEUE,
                "Identifier": "another-queue",
                "ClientToken": "ct-delete",
            }),
        ),
    ];

    for (case, client_token, first, second) in rows {
        let operation = if first.get("DesiredState").is_some() {
            "CreateResource"
        } else {
            "DeleteResource"
        };
        let accepted = emulator.cc_ok(operation, first).await;
        let first_use = text(&progress(&accepted), "RequestToken");
        let (status, body) = emulator.cc(operation, second).await;
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
                "ClientToken {client_token} is already associated with an existing operation \
                 with RequestToken {first_use}"
            ),
            "case {case:?}: unexpected wording"
        );
    }
}

#[tokio::test]
async fn an_unknown_request_token_is_refused_naming_the_token() {
    let emulator = Emulator::new();
    // The rows differ along the SHAPE of a token nothing minted: the form this service
    // mints, one of a different form entirely, the shortest and the longest the model
    // admits (`RequestToken`, length 1..128, pattern [-A-Za-z0-9+/=]+).
    let rows = [
        ("the form this service mints", mint_shaped_token()),
        ("another form entirely", "cc-never-issued-00000001".to_string()),
        ("the shortest token the model admits", "t".to_string()),
        ("the longest token the model admits", "t".repeat(128)),
        ("one made only of the punctuation the pattern admits", "-+/=".to_string()),
    ];
    for (case, token) in rows {
        let (status, body) = emulator.status(&token).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("RequestTokenNotFoundException"),
            "case {case:?}: expected RequestTokenNotFoundException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Request with token {token} was not found"),
            "case {case:?}: unexpected wording"
        );
    }

    // A token this service DID mint resolves, so the refusal above is about the token
    // and not about the operation.
    let accepted = emulator
        .create_ok(PARAMETER, &json!({ "Name": "polled-parameter" }), None)
        .await;
    let (status, _) = emulator.status(&text(&accepted, "RequestToken")).await;
    assert_eq!(status, 200, "a token this service minted must resolve");
}

/// A token of the form this service mints its own: a hyphenated UUID, which nothing in a
/// fresh store has been handed out as.
fn mint_shaped_token() -> String {
    "9f8b7c6d-5e4f-4a3b-8c1d-0e9f8a7b6c5d".to_string()
}

#[tokio::test]
async fn a_patch_of_a_create_only_property_is_refused_with_the_schema_pointer() {
    let emulator = Emulator::new();
    // A create-only property is one the type's registry schema declares as such, which
    // always includes the primary identifier — what every operation addresses the
    // resource by. The rows differ along the property that carries it, which is the
    // type's own rather than one name, and along the patch operation that would change
    // it.
    let rows: [(&str, &str, Value, &str); 3] = [
        (
            "a type whose identifier property is named after it",
            QUEUE,
            json!({ "QueueName": "create-only-queue" }),
            "QueueName",
        ),
        (
            "a type whose identifier property is the bare Name",
            PARAMETER,
            json!({ "Name": "create-only-parameter", "Type": "String" }),
            "Name",
        ),
        (
            "a third type, so the pointer is built from the model",
            BUCKET,
            json!({ "BucketName": "create-only-bucket" }),
            "BucketName",
        ),
    ];

    for (case, type_name, desired, property) in rows {
        let identifier = emulator.provision(type_name, &desired).await;
        // The model the created resource reads back as, which a refused update has to
        // leave exactly as it is: the desired state itself for a type whose resources live
        // in this service's own store, and the model the OWNING service holds for a type
        // it holds the resources of.
        let model = emulator.properties(type_name, &identifier).await;
        let patches = [
            (
                "replacing it",
                json!([{ "op": "replace", "path": format!("/{property}"), "value": "renamed" }]),
            ),
            (
                "removing it",
                json!([{ "op": "remove", "path": format!("/{property}") }]),
            ),
            (
                "copying another property over it",
                json!([
                    { "op": "add", "path": "/Alias", "value": "other" },
                    { "op": "move", "from": "/Alias", "path": format!("/{property}") },
                ]),
            ),
        ];
        for (patch_case, patch) in patches {
            let case = format!("{case} ({patch_case})");
            let (status, body) = emulator.update(type_name, &identifier, &patch, None).await;
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
                format!(
                    "Invalid patch update: createOnlyProperties [/properties/{property}] \
                     cannot be updated"
                ),
                "case {case:?}: unexpected wording"
            );
            let stored = emulator.properties(type_name, &identifier).await;
            assert_eq!(
                stored, model,
                "case {case:?}: a refused update must change nothing, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_patch_of_a_create_only_property_the_model_does_not_carry_is_still_refused() {
    let emulator = Emulator::new();
    // A create-only property the type's schema declares is refused because the DOCUMENT
    // names it, so a patch naming one the stored model does not even carry
    // (`AWS::SQS::Queue` /properties/FifoQueue, which the schema declares create-only and
    // a queue created without it does not carry) is the create-only refusal rather than
    // a document that could not be applied. The rows differ along the operation, and the
    // last one names two create-only properties at once — which the diagnostic lists
    // together.
    let desired = json!({ "QueueName": "fifo-less-queue" });
    let identifier = emulator.provision(QUEUE, &desired).await;
    // The model the OWNING service holds for the created queue, which a refused update
    // has to leave exactly as it is.
    let model = emulator.properties(QUEUE, &identifier).await;
    let rows: [(&str, Value, &str); 4] = [
        (
            "replacing a create-only property the model does not carry",
            json!([{ "op": "replace", "path": "/FifoQueue", "value": true }]),
            "/properties/FifoQueue",
        ),
        (
            "adding it",
            json!([{ "op": "add", "path": "/FifoQueue", "value": true }]),
            "/properties/FifoQueue",
        ),
        (
            "removing it",
            json!([{ "op": "remove", "path": "/FifoQueue" }]),
            "/properties/FifoQueue",
        ),
        (
            "naming two create-only properties at once",
            json!([
                { "op": "add", "path": "/FifoQueue", "value": true },
                { "op": "replace", "path": "/QueueName", "value": "renamed" },
            ]),
            "/properties/QueueName, /properties/FifoQueue",
        ),
    ];
    for (case, patch, pointers) in rows {
        let (status, body) = emulator.update(QUEUE, &identifier, &patch, None).await;
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
            format!("Invalid patch update: createOnlyProperties [{pointers}] cannot be updated"),
            "case {case:?}: unexpected wording"
        );
        let stored = emulator.properties(QUEUE, &identifier).await;
        assert_eq!(
            stored, model,
            "case {case:?}: a refused update must change nothing, got {stored:?}"
        );
    }

    // A property the schema does NOT declare create-only is patched as usual, so the
    // refusals above are about the schema rather than about patching at all.
    emulator
        .update_and_settle(
            QUEUE,
            &identifier,
            &json!([{ "op": "add", "path": "/DelaySeconds", "value": 5 }]),
        )
        .await;
    let stored = emulator.properties(QUEUE, &identifier).await;
    assert_eq!(
        stored.get("DelaySeconds"),
        Some(&json!(5)),
        "a property the schema does not declare create-only must be patchable, got {stored:?}"
    );
}

#[tokio::test]
async fn a_constraint_violation_states_the_expression_without_the_models_anchors() {
    let emulator = Emulator::new();
    let identifier = emulator
        .provision(QUEUE, &json!({ "QueueName": "validated-queue" }))
        .await;

    // The rows differ along the pattern-constrained member the request gets wrong, and
    // along how its value fails. Each expression is the member's `@pattern` as the model
    // declares it, WITHOUT the `^` / `$` the model writes around it: `ClientToken`
    // ^[-A-Za-z0-9+/=]+$ and `TypeName`
    // ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$.
    let client_token_expression = "[-A-Za-z0-9+/=]+";
    let type_name_expression =
        "[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}";
    let rows: [(&str, Value, &str, &str, &str); 4] = [
        (
            "a client token carrying spaces and punctuation",
            json!({
                "TypeName": QUEUE,
                "Identifier": identifier,
                "PatchDocument": "[]",
                "ClientToken": "not a valid token!!",
            }),
            "not a valid token!!",
            "clientToken",
            client_token_expression,
        ),
        (
            "a client token carrying one character the expression excludes",
            json!({
                "TypeName": QUEUE,
                "Identifier": identifier,
                "PatchDocument": "[]",
                "ClientToken": "token.1",
            }),
            "token.1",
            "clientToken",
            client_token_expression,
        ),
        (
            "a type name carrying a character the expression excludes",
            json!({
                "TypeName": "AWS::SQS::Queue!!!!",
                "Identifier": identifier,
                "PatchDocument": "[]",
            }),
            "AWS::SQS::Queue!!!!",
            "typeName",
            type_name_expression,
        ),
        (
            "a type name of the wrong shape",
            json!({
                "TypeName": "AWS::SQS::Queue::Extra",
                "Identifier": identifier,
                "PatchDocument": "[]",
            }),
            "AWS::SQS::Queue::Extra",
            "typeName",
            type_name_expression,
        ),
    ];

    for (case, request, value, member, expression) in rows {
        let (status, body) = emulator.cc("UpdateResource", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!(
                "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {expression}"
            ),
            "case {case:?}: unexpected wording"
        );
    }
}

#[tokio::test]
async fn a_patch_document_the_front_door_refuses_is_a_validation_failure() {
    let emulator = Emulator::new();
    let identifier = emulator
        .provision(QUEUE, &json!({ "QueueName": "patched-queue", "Tags": [] }))
        .await;
    // The model the OWNING service holds for the created queue, which a refused update has
    // to leave exactly as it is.
    let model = emulator.properties(QUEUE, &identifier).await;

    // The rows differ along WHICH HALF of the service judges the document: one the front
    // door will not read, or whose operation names a path the resource model does not
    // have, never reaches a handler; a verdict on the model the handler would have been
    // asked to store is the handler's own. A document the reader will not take names
    // where in the document it stopped, so the rows also differ along where that is.
    let rows: [(&str, &str, &str, &str); 6] = [
        (
            "a document that is not JSON",
            "not json",
            "ValidationException",
            "Invalid PatchDocument: A JSONArray text must start with '[' at 1 \
             [character 2 line 1]",
        ),
        (
            "a document that is not a list of operations",
            "{\"op\":\"add\",\"path\":\"/A\",\"value\":1}",
            "ValidationException",
            "Invalid PatchDocument: A JSONArray text must start with '[' at 1 \
             [character 2 line 1]",
        ),
        (
            "a document whose value begins on another line",
            " \n not json",
            "ValidationException",
            "Invalid PatchDocument: A JSONArray text must start with '[' at 4 \
             [character 3 line 2]",
        ),
        (
            "an operation RFC 6902 does not define",
            "[{\"op\":\"frobnicate\",\"path\":\"/A\"}]",
            "ValidationException",
            "Invalid PatchDocument: the document carries an operation that is not a JSON \
             Patch operation",
        ),
        (
            "a replace of a path the model does not carry",
            "[{\"op\":\"replace\",\"path\":\"/NoSuchProp\",\"value\":1}]",
            "ValidationException",
            "Invalid PatchDocument: the replace operation at path '/NoSuchProp' could not \
             be applied to the resource model",
        ),
        (
            "a document that would leave something other than a resource model",
            "[{\"op\":\"replace\",\"path\":\"\",\"value\":[]}]",
            "InvalidRequestException",
            "Invalid request provided: the patch document would leave a resource model \
             that is not a JSON object",
        ),
    ];

    for (case, patch_document, expected_code, expected_message) in rows {
        let (status, body) = emulator
            .cc(
                "UpdateResource",
                json!({
                    "TypeName": QUEUE,
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
        let stored = emulator.properties(QUEUE, &identifier).await;
        assert_eq!(
            stored,
            model,
            "case {case:?}: a refused update must change nothing, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_patch_that_grows_the_model_past_what_it_can_hold_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let identifier = emulator
        .provision(QUEUE, &json!({ "QueueName": "grown-queue", "Tags": [] }))
        .await;
    // The model the OWNING service holds for the created queue, which a refused update has
    // to leave exactly as it is.
    let model = emulator.properties(QUEUE, &identifier).await;

    // What one patch document can make of a resource model is bounded in BOTH dimensions
    // RFC 6902's `copy` and `move` multiply — a copy of the model into a point inside
    // itself doubles its nesting depth as well as its size — so the rows differ along
    // which dimension the document grows: a short document that doubles the depth until it
    // is past what this service can read, and one that duplicates a large value until the
    // model is longer than one it can report.
    let deepening: Vec<Value> = {
        // Each copy puts the whole model at the deepest point the model already has, so
        // the chain of nested members, and the depth with it, about doubles per operation.
        let mut operations = Vec::new();
        let mut chain = 1_usize;
        for _ in 0..8 {
            operations.push(json!({
                "op": "copy",
                "from": "",
                "path": "/Nest".repeat(chain),
            }));
            chain *= 2;
        }
        operations
    };
    let bulky = {
        // Three copies of a value that is itself over a third of the bound.
        let value = "g".repeat(100_000);
        let mut operations = vec![json!({ "op": "add", "path": "/Big", "value": value })];
        for key in ["/BigA", "/BigB"] {
            operations.push(json!({ "op": "copy", "from": "/Big", "path": key }));
        }
        operations
    };
    let rows = [
        (
            "a document that doubles the model's depth",
            Value::Array(deepening),
            "Invalid request provided: the patch document would leave a resource model \
             nested deeper than 128 levels",
        ),
        (
            "a document that duplicates a large value",
            Value::Array(bulky),
            "Invalid request provided: the patch document would leave a resource model \
             longer than 262144 characters",
        ),
    ];

    for (case, patch_document, expected_message) in rows {
        let (status, body) = emulator
            .update(QUEUE, &identifier, &patch_document, None)
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
            expected_message,
            "case {case:?}: unexpected wording"
        );
        let stored = emulator.properties(QUEUE, &identifier).await;
        assert_eq!(
            stored, model,
            "case {case:?}: a refused update must change nothing, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_create_whose_resource_exists_fails_with_the_handlers_already_exists_verdict() {
    let emulator = Emulator::new();
    // A resource that is already there is the OWNING handler's verdict, so the request is
    // accepted and terminates FAILED carrying that handler's error code and its own
    // wording. The rows differ along the owning service's wording: one that has a
    // sentence of its own, and the types reported in Cloud Control's generic sentence.
    let rows: [(&str, &str, Value, String); 3] = [
        (
            "a type whose own service has a sentence for it",
            QUEUE,
            json!({ "QueueName": "duplicate-queue" }),
            "Queue creation failed because the queue already exists".to_string(),
        ),
        (
            "a type reported in the generic sentence",
            PARAMETER,
            json!({ "Name": "duplicate-parameter", "Type": "String", "Value": "v" }),
            generic_already_exists(PARAMETER, "duplicate-parameter"),
        ),
        (
            "a second such type, so the sentence is built from the request",
            BUCKET,
            json!({ "BucketName": "duplicate-bucket" }),
            generic_already_exists(BUCKET, "duplicate-bucket"),
        ),
    ];

    for (case, type_name, desired, expected_message) in rows {
        let identifier = emulator.provision(type_name, &desired).await;
        // The model the created resource reads back as, which a failed create has to leave
        // exactly as it is.
        let model = emulator.properties(type_name, &identifier).await;

        // The duplicate names the identifier the first create resolved.
        let (status, body) = emulator.create(type_name, &desired, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to be accepted, got {status} with body {body:?}"
        );
        let failed = emulator
            .terminal(&text(&progress(&body), "RequestToken"))
            .await;
        assert_eq!(
            text(&failed, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: expected the request to fail, got {failed:?}"
        );
        assert_eq!(
            text(&failed, "ErrorCode"),
            "AlreadyExists",
            "case {case:?}: expected the AlreadyExists handler error code, got {failed:?}"
        );
        assert_eq!(
            text(&failed, "StatusMessage"),
            expected_message,
            "case {case:?}: unexpected wording"
        );
        assert_eq!(
            text(&failed, "Identifier"),
            identifier,
            "case {case:?}: the failed request must name the resource it was for, got {failed:?}"
        );
        let stored = emulator.properties(type_name, &identifier).await;
        assert_eq!(
            stored, model,
            "case {case:?}: the failed create must leave the resource as it was, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_type_the_registry_does_not_hold_is_refused_before_anything_it_names_is_resolved() {
    let emulator = Emulator::new();
    // The rows differ along how a syntactically valid `TypeName` fails to name a
    // registered extension: a service namespace that does not exist, one that looks like
    // a probe, one whose organization is not a publisher, and a private type nothing
    // registered in this account.
    let unregistered = [
        "AWS::NoSuchService::FakeResource",
        "AWS::SimParityProbe::NoSuchType",
        "AWS::NoSuch::Servicexyz",
        "Organisation::Service::Resource",
    ];
    for type_name in unregistered {
        // Every operation that names a type asks the registry first.
        let answers = [
            (
                "CreateResource",
                emulator
                    .create(type_name, &json!({ "Name": "whatever" }), None)
                    .await,
            ),
            ("GetResource", emulator.get(type_name, "whatever").await),
            (
                "ListResources",
                emulator
                    .cc("ListResources", Emulator::list_request(type_name, None, None, None))
                    .await,
            ),
            (
                "UpdateResource",
                emulator
                    .update(type_name, "whatever", &inert_patch(), None)
                    .await,
            ),
            (
                "DeleteResource",
                emulator.delete(type_name, "whatever").await,
            ),
        ];
        for (operation, (status, body)) in answers {
            let case = format!("{type_name} ({operation})");
            // RECORDED behaviour: the model annotates TypeNotFoundException
            // `@httpError 404`, and this awsJson1_0 service answers it with 400.
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains("TypeNotFoundException"),
                "case {case:?}: expected TypeNotFoundException, got {:?}",
                code(&body)
            );
            assert_eq!(
                message(&body),
                format!("The type '{type_name}' cannot be found."),
                "case {case:?}: unexpected wording"
            );
            assert!(
                progress(&body).is_null(),
                "case {case:?}: a refused request mints no resource operation, got {body:?}"
            );
        }
    }

    // And a type the registry DOES hold goes ahead — including one no row above names, so
    // the rule is about the registry rather than about the types the other suites use.
    for (type_name, desired) in [
        ("AWS::Kinesis::Stream", json!({ "StreamName": "registered" })),
        ("AWS::Lambda::Function", json!({ "FunctionName": "registered" })),
        ("Alexa::ASK::Skill", json!({ "Id": "registered-skill" })),
    ] {
        let identifier = emulator.provision(type_name, &desired).await;
        let listed = emulator.list(type_name, None, None).await;
        assert_eq!(
            descriptions(&listed).len(),
            1,
            "type {type_name:?}: the listing must report the resource that was created, got \
             {listed:?}"
        );
        let stored = emulator.properties(type_name, &identifier).await;
        assert_eq!(
            stored, desired,
            "type {type_name:?}: the resource must read back, got {stored:?}"
        );
    }
}
