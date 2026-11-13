// SPDX-License-Identifier: CC0-1.0

//! The identity every answer names a resource by, and how a delete that finds nothing is
//! answered.
//!
//! Two rules, each held over a TABLE of resource types rather than over one request:
//!
//! * **`ProgressEvent$Identifier` is "The primary identifier for the resource" — the
//!   property the type's own registry schema points `primaryIdentifier` at, as the
//!   component that OWNS the resource holds it — so the value a create hands back is the
//!   one `GetResource`, `ListResources`, `UpdateResource` and `DeleteResource` accept, it
//!   appears in the resource model under that property, and so does every other identifier
//!   the type's schema declares for the same resource.**
//! * **`DeleteResource` is asynchronous — "after you have initiated a resource deletion
//!   request, you can monitor the progress of your request by calling
//!   `GetResourceRequestStatus` using the `RequestToken` of the `ProgressEvent` returned by
//!   `DeleteResource`" — so a syntactically valid request is ADMITTED and answered with an
//!   `IN_PROGRESS` event, and whether the resource exists is the owning handler's verdict,
//!   which arrives on that request record as `FAILED` carrying the `NotFound` handler error
//!   code and that handler's own diagnostic.**

use serde_json::{json, Value};

use super::emulator::{
    code, progress, text, Emulator, SIGNING_REGION, STATUS_FAILED, STATUS_IN_PROGRESS,
};

/// One row of the identity table.
struct Case {
    /// What this row is about.
    name: &'static str,
    /// The `TypeName` the resource is created under.
    type_name: &'static str,
    /// The desired state the create carries.
    desired: Value,
    /// The property the type's schema points `primaryIdentifier` at.
    property: &'static str,
    /// The identifier every answer must name the resource by.
    identifier: String,
    /// Another identifier the type's schema declares for the same resource, which a
    /// request may address it by — `None` for a type whose schema declares only the
    /// primary one.
    secondary: Option<&'static str>,
}

/// The rows: one per kind of primary identifier, so what is asserted is the RULE rather
/// than one type.
///
/// They differ along who holds the identifier and what shape it has — an ARN the owning
/// service assigns (`AWS::SNS::Topic`, `primaryIdentifier: ["/properties/TopicArn"]`), a
/// URL it assigns (`AWS::SQS::Queue`, `/properties/QueueUrl`), a name the desired state
/// carries for a resource the owning service holds (`AWS::Logs::LogGroup`,
/// `/properties/LogGroupName`), and a name for a resource this service's own store holds
/// (`AWS::Kinesis::Stream`) — and along whether the type declares a second identifier for
/// the same resource.
fn rows() -> Vec<Case> {
    let account = engine::emulator_account_id();
    vec![
        Case {
            name: "an ARN the owning service assigns",
            type_name: "AWS::SNS::Topic",
            desired: json!({ "TopicName": "orders", "DisplayName": "Orders" }),
            property: "TopicArn",
            identifier: engine::emulator_arn("sns", SIGNING_REGION, "orders"),
            secondary: Some("orders"),
        },
        Case {
            name: "a URL the owning service assigns",
            type_name: "AWS::SQS::Queue",
            desired: json!({ "QueueName": "jobs" }),
            property: "QueueUrl",
            identifier: format!("https://sqs.{SIGNING_REGION}.amazonaws.com/{account}/jobs"),
            secondary: Some("jobs"),
        },
        Case {
            name: "a name the desired state carries, for a resource the owning service holds",
            type_name: "AWS::Logs::LogGroup",
            desired: json!({ "LogGroupName": "/app/orders", "RetentionInDays": 7 }),
            property: "LogGroupName",
            identifier: "/app/orders".to_string(),
            secondary: None,
        },
        Case {
            name: "a name the desired state carries, for a resource this service's store holds",
            type_name: "AWS::Kinesis::Stream",
            desired: json!({ "StreamName": "events", "ShardCount": 2 }),
            property: "StreamName",
            identifier: "events".to_string(),
            secondary: None,
        },
    ]
}

#[tokio::test]
async fn every_answer_names_the_resource_by_the_primary_identifier_its_type_declares() {
    let emulator = Emulator::new();
    let cases = rows();

    // Started together, then waited for: the creates are independent, so one span covers
    // them all.
    let mut accepted = Vec::new();
    for case in &cases {
        let event = emulator
            .create_ok(case.type_name, &case.desired, None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }

    for (case, request_token) in cases.iter().zip(&accepted) {
        // The identifier the create's own event reports, once the request has settled.
        let settled = emulator.settled(request_token).await;
        let reported = text(&settled, "Identifier");
        assert_eq!(
            reported, case.identifier,
            "case {:?}: expected the identifier {:?}, got {reported:?}",
            case.name, case.identifier
        );

        // The resource model carries it under the property the type's schema names, so the
        // handle a caller was handed is findable as a property of the resource.
        let stored = emulator.properties(case.type_name, &reported).await;
        assert_eq!(
            stored.get(case.property),
            Some(&json!(case.identifier)),
            "case {:?}: the model must carry the identifier under {:?}, got {stored:?}",
            case.name,
            case.property
        );
        for (property, wanted) in case.desired.as_object().cloned().unwrap_or_default() {
            assert_eq!(
                stored.get(&property),
                Some(&wanted),
                "case {:?}: expected {property} to read back as {wanted:?}, got {stored:?}",
                case.name
            );
        }

        // The listing of the type reports the same identifier.
        let listed = emulator.walk(case.type_name, None).await;
        assert_eq!(
            listed,
            vec![case.identifier.clone()],
            "case {:?}: the listing must report the resource under its primary identifier, \
             got {listed:?}",
            case.name
        );

        // And so does a read through another identifier the type declares for it.
        if let Some(secondary) = case.secondary {
            let by_secondary = emulator.properties(case.type_name, secondary).await;
            assert_eq!(
                by_secondary, stored,
                "case {:?}: {secondary:?} must address the same resource, got \
                 {by_secondary:?}",
                case.name
            );
        }
    }

    // The documented flow is create, read the Identifier off the ProgressEvent, delete by
    // it — so the identifier each create handed back is one a delete accepts.
    for case in &cases {
        let deleted = emulator
            .delete_and_settle(case.type_name, &case.identifier)
            .await;
        assert_eq!(
            text(&deleted, "Identifier"),
            case.identifier,
            "case {:?}: the delete must name the resource's primary identifier, got \
             {deleted:?}",
            case.name
        );
        let (status, body) = emulator.get(case.type_name, &case.identifier).await;
        assert_eq!(
            status, 404,
            "case {:?}: GetResource after the delete: expected HTTP 404, got {status} with \
             body {body:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_resource_is_deleted_through_any_identifier_its_type_declares_for_it() {
    let emulator = Emulator::new();
    // The rows differ along the type and along which of its identifiers the delete names:
    // a resource addressed by a secondary identifier is the same resource, so the request
    // acts on — and reports — the primary one.
    let rows = [
        (
            "a topic by its name",
            "AWS::SNS::Topic",
            json!({ "TopicName": "by-name" }),
            "by-name",
            engine::emulator_arn("sns", SIGNING_REGION, "by-name"),
        ),
        (
            "a queue by its name",
            "AWS::SQS::Queue",
            json!({ "QueueName": "by-name" }),
            "by-name",
            format!(
                "https://sqs.{SIGNING_REGION}.amazonaws.com/{}/by-name",
                engine::emulator_account_id()
            ),
        ),
    ];
    for (case, type_name, desired, secondary, primary) in rows {
        let identifier = emulator.provision(type_name, &desired).await;
        assert_eq!(
            identifier, primary,
            "case {case:?}: expected the identifier {primary:?}, got {identifier:?}"
        );

        let deleted = emulator.delete_and_settle(type_name, secondary).await;
        assert_eq!(
            text(&deleted, "Identifier"),
            primary,
            "case {case:?}: a delete by another identifier must act on the primary one, got \
             {deleted:?}"
        );
        let (status, body) = emulator.get(type_name, &primary).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted resource: expected HTTP 404, got {status} with body \
             {body:?}"
        );
    }
}

/// What a row knows about the handler diagnostic a failed request reports.
enum Diagnostic {
    /// Every character of it is fixed text or a value taken from the request.
    Exact(String),
    /// It opens with fixed text and goes on with a value the owning service mints per
    /// call, so the row pins the opening alone.
    StartsWith(String),
}

impl Diagnostic {
    /// Check `stated` against what the row knows.
    fn check(&self, stated: &str, case: &str) {
        match self {
            Self::Exact(expected) => assert_eq!(
                stated, expected,
                "case {case:?}: expected the handler diagnostic {expected:?}, got {stated:?}"
            ),
            Self::StartsWith(opening) => assert!(
                stated.starts_with(opening.as_str()) && stated.len() > opening.len(),
                "case {case:?}: expected a handler diagnostic opening {opening:?}, got \
                 {stated:?}"
            ),
        }
    }
}

/// What the `AWS::Logs::LogGroup` handler states for a group it cannot find: Cloud
/// Control's own sentence, naming the resource as the type's primary-identifier object —
/// the form that handler's refusal is recorded carrying.
fn log_group_not_found(name: &str) -> String {
    let rendered = json!({ "/properties/LogGroupName": name }).to_string();
    format!("Resource of type 'AWS::Logs::LogGroup' with identifier '{rendered}' was not found.")
}

#[tokio::test]
async fn a_delete_of_an_absent_resource_is_admitted_and_fails_on_its_own_token() {
    // The owning service of one of the rows is mounted beside this one, so the row about a
    // type whose resources live THERE is about that service's handler.
    let emulator = Emulator::with_cloudwatch_logs();

    // The rows differ along where a resource of the type would live (another service's
    // store, this service's own store), along the form of the identifier (a name, a path
    // shaped one, an ARN, a name of a type whose identifier is an ARN), and along the
    // handler's own not-found wording, which each row pins.
    let rows = [
        (
            "a type whose resources live in the owning service's store",
            "AWS::Logs::LogGroup",
            "never-created-group".to_string(),
            Diagnostic::Exact(log_group_not_found("never-created-group")),
        ),
        (
            "a second identifier shape of that type",
            "AWS::Logs::LogGroup",
            "/aws/never/created".to_string(),
            Diagnostic::Exact(log_group_not_found("/aws/never/created")),
        ),
        (
            "a type whose resources live in this service's own store",
            "AWS::Kinesis::Stream",
            "never-created-stream".to_string(),
            Diagnostic::Exact(
                "Resource of type 'AWS::Kinesis::Stream' with identifier \
                 'never-created-stream' was not found."
                    .to_string(),
            ),
        ),
        (
            "a type whose own service words the absence itself",
            "AWS::DynamoDB::Table",
            "never-created-table".to_string(),
            Diagnostic::Exact("Table: never-created-table does not exist.".to_string()),
        ),
        (
            "the ARN of a type whose primary identifier is one",
            "AWS::SNS::Topic",
            engine::emulator_arn("sns", SIGNING_REGION, "never-created-topic"),
            Diagnostic::StartsWith("Topic does not exist (Service: Sns, ".to_string()),
        ),
        (
            "the name that type also answers to",
            "AWS::SNS::Topic",
            "never-created-by-name".to_string(),
            Diagnostic::StartsWith("Topic does not exist (Service: Sns, ".to_string()),
        ),
    ];

    // Admitted first, each answered with the event the caller is told to poll.
    let mut admitted = Vec::new();
    for (case, type_name, identifier, _) in &rows {
        let (status, body) = emulator.delete(type_name, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body:?}"
        );
        let event = progress(&body);
        assert_eq!(
            text(&event, "Operation"),
            "DELETE",
            "case {case:?}: expected a DELETE operation, got {event:?}"
        );
        assert_eq!(
            text(&event, "OperationStatus"),
            STATUS_IN_PROGRESS,
            "case {case:?}: an admitted delete is in progress, got {event:?}"
        );
        assert_eq!(
            text(&event, "TypeName"),
            *type_name,
            "case {case:?}: expected the type the request carried, got {event:?}"
        );
        assert_eq!(
            text(&event, "Identifier"),
            *identifier,
            "case {case:?}: expected the identifier the request carried, got {event:?}"
        );
        let request_token = text(&event, "RequestToken");
        assert!(
            !request_token.is_empty(),
            "case {case:?}: an admitted delete must carry a token to poll, got {event:?}"
        );
        assert!(
            !event["EventTime"].is_null(),
            "case {case:?}: an admitted delete must report when it was initiated, got \
             {event:?}"
        );
        admitted.push(request_token);
    }

    // Then the handler's verdict arrives on that token, and nothing was created by the
    // delete that could not find it.
    for ((case, type_name, identifier, diagnostic), request_token) in rows.iter().zip(&admitted) {
        let failed = emulator.terminal(request_token).await;
        assert_eq!(
            text(&failed, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: the delete of an absent resource must fail, got {failed:?}"
        );
        assert_eq!(
            text(&failed, "ErrorCode"),
            "NotFound",
            "case {case:?}: expected the NotFound handler error code, got {failed:?}"
        );
        diagnostic.check(&text(&failed, "StatusMessage"), case);
        let (status, body) = emulator.get(type_name, identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the resource must still not exist, got {status} with body \
             {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
    }
}

#[tokio::test]
async fn a_delete_of_a_resource_created_moments_earlier_is_accepted() {
    // The documented round trip — create, read the Identifier off the ProgressEvent,
    // delete by that Identifier — for a type whose identifier the caller names and one
    // whose identifier the owning service assigns.
    let emulator = Emulator::new();
    let rows = [
        (
            "an identifier the desired state carries",
            "AWS::Kinesis::Stream",
            json!({ "StreamName": "round-trip-stream", "ShardCount": 1 }),
        ),
        (
            "an identifier the owning service assigns",
            "AWS::SNS::Topic",
            json!({ "TopicName": "round-trip-topic" }),
        ),
    ];
    for (case, type_name, desired) in rows {
        let created = emulator.create_ok(type_name, &desired, None).await;
        let settled = emulator.settled(&text(&created, "RequestToken")).await;
        let identifier = text(&settled, "Identifier");

        let (status, body) = emulator.delete(type_name, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete of the identifier the create reported: expected \
             HTTP 200, got {status} with body {body:?}"
        );
        let removed = emulator
            .settled(&text(&progress(&body), "RequestToken"))
            .await;
        assert_eq!(
            text(&removed, "Identifier"),
            identifier,
            "case {case:?}: the delete must name the resource it removed, got {removed:?}"
        );
        let (status, body) = emulator.get(type_name, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted resource: expected HTTP 404, got {status} with body \
             {body:?}"
        );
    }
}
