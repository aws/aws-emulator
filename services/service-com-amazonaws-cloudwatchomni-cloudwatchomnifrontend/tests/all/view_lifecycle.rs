// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's telemetry query surface: the views a query
//! binds to, and the query SESSIONS a query runs in.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * a view IS the SQL query it is defined by, so `CreateView` admits a `definition` a
//!   SQL parser reads as ONE query — whatever form that query takes — and refuses one it
//!   does not with `ValidationException` (400) carrying what the definition rule says
//!   about it, persisting nothing for the refusal (the message each class of defect earns
//!   is asserted row by row in `view_definition_diagnosis`);
//! * a created view is persisted and `GetView` answers from that same store, reporting
//!   the definition, the description, the `USER` type, the ARN and the timestamps the
//!   create reported;
//! * a view's name is its identity within the account and region, so a second view under
//!   a live name is a `ConflictException` (409) stating that a view already exists under
//!   the name the request carried, and leaves the first one as it was;
//! * an unknown name resolves to nothing: `ResourceNotFoundException` (404);
//! * `ListViews` reports the views the store holds, in creation order, as summaries
//!   without their definitions, restricted to the ownership category the request names
//!   and paged at `maxResults` with a `nextToken` that resumes where the page stopped;
//! * `DeleteView` releases the view its name identifies, so the name resolves to nothing
//!   and is free again, and a name the store holds nothing under is a 404;
//! * the name conflict wins over an `@idempotencyToken` repeat: a create repeated with
//!   the token that minted the view asks for a name the account holds, so it is refused
//!   like any other create under that name;
//! * `UpdateView` is a PARTIAL update of the persisted view: every member the request
//!   carries replaces the stored one, every member it omits — the identity, the ARN, the
//!   ownership category and `createdAt` among them — keeps exactly what it held, and the
//!   patched record is what `GetView` answers afterwards. A new `definition` is held to
//!   the same rule the create holds one to, so a definition that is not one SQL query is
//!   the same `ValidationException` (400) with the same message and writes nothing at
//!   all, and a name the store holds nothing under is the 404. These rows are driven over
//!   the rpcv2Cbor wire, so what a real client reads is what is asserted.
//!
//! The query-session half asserts its own rules, at the end of this file:
//!
//! * `StartTelemetryQuerySession` mints a session the caller owns, whatever name the
//!   model admits for it (and none at all), persists it and answers with the id it
//!   minted, in the form the model fixes for a `sessionId`;
//! * `ListTelemetryQuerySessions` reads those records back — the id, the name, the
//!   activity instants — in start order, paged at `maxResults` with a `nextToken` that
//!   resumes where the page stopped, and refuses a token it did not mint;
//! * a session name identifies a session among the ones ONE caller holds, so a second
//!   start under a name the caller holds is a `ConflictException` (409) stating that a
//!   session of that name already exists, and mints nothing, while another caller may
//!   hold a session under the same name;
//! * a name in the `/aws/` namespace is reserved for service integrations, so a caller
//!   asking for one is refused with `ValidationException` (400) and nothing is recorded;
//! * a listing reports only the sessions the calling principal started.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_view::{CreateView, CreateViewError, CreateViewErrorModeled};
use omni::ops::delete_view::{DeleteView, DeleteViewError};
use omni::ops::get_view::{GetView, GetViewError};
use omni::ops::list_telemetry_query_sessions::{
    ListTelemetryQuerySessions, ListTelemetryQuerySessionsError,
};
use omni::ops::list_views::{ListViews, ListViewsError};
use omni::ops::start_telemetry_query_session::{
    StartTelemetryQuerySession, StartTelemetryQuerySessionError,
};
use omni::ops::types::{
    CreateViewInput, CreateViewOutput, DeleteViewInput, DeleteViewOutput, GetViewInput,
    GetViewOutput, ListTelemetryQuerySessionsInput, ListTelemetryQuerySessionsOutput,
    ListViewsInput, ListViewsOutput, StartTelemetryQuerySessionInput,
    StartTelemetryQuerySessionOutput, UpdateViewInput, UpdateViewOutput, CREATE_VIEW_INPUT_SCHEMA,
    GET_VIEW_INPUT_SCHEMA, LIST_TELEMETRY_QUERY_SESSIONS_INPUT_SCHEMA, LIST_VIEWS_INPUT_SCHEMA,
    START_TELEMETRY_QUERY_SESSION_INPUT_SCHEMA, UPDATE_VIEW_INPUT_SCHEMA, VIEW_TYPE_VALUES,
};
use omni::scaffolded::Scaffolded;
use omni::state::{
    view_definition_rejection, INVALID_VIEW_DEFINITION, SESSION_NAME_ALREADY_EXISTS_MESSAGE,
    SESSION_NAME_NOT_VALID_MESSAGE, VIEW_ARN_RESOURCE_TYPE,
    VIEW_DEFINITION_MUST_BE_A_SELECT_QUERY, VIEW_DEFINITION_MUST_BE_ONE_STATEMENT,
    VIEW_TYPE_USER,
};

use super::validation_message_wording::error_envelope;

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite signs with.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record and `ACCESS_KEY_ID`.
fn signed_request_parts() -> http::request::Parts {
    signed_request_parts_for(ACCESS_KEY_ID)
}

/// The same request metadata, signed by `access_key_id`: a caller's identity is read off
/// the SigV4 credential scope, so this is how a second principal calls.
fn signed_request_parts_for(access_key_id: &str) -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
pub(crate) struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    pub(crate) async fn create_view(
        &self,
        input: &CreateViewInput,
    ) -> Result<CreateViewOutput, CreateViewError> {
        CreateView::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    pub(crate) async fn get_view(&self, name: &str) -> Result<GetViewOutput, GetViewError> {
        GetView::new(Arc::clone(&self.engine))
            .handle(
                GetViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    pub(crate) async fn list_views(
        &self,
        input: &ListViewsInput,
    ) -> Result<ListViewsOutput, ListViewsError> {
        ListViews::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    pub(crate) async fn delete_view(
        &self,
        name: &str,
    ) -> Result<DeleteViewOutput, DeleteViewError> {
        DeleteView::new(Arc::clone(&self.engine))
            .handle(
                DeleteViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    pub(crate) async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
pub(crate) fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                expected_status,
                "case {case:?}: expected HTTP {expected_status} for {expected_code}, got {}",
                error.http_status()
            );
        }
    }
}

/// The smallest view request the model admits, for `name` and `definition`.
pub(crate) fn create_input(name: &str, definition: &str) -> CreateViewInput {
    CreateViewInput {
        name: name.to_string(),
        definition: definition.to_string(),
        description: None,
        tags: None,
        client_token: None,
    }
}

/// Definitions a SQL parser reads as ONE query, so `CreateView` persists them.
///
/// The rows differ along the FORM of the query — projection, predicate, grouping,
/// set operation, common table expression, join and ordering, a leading comment, a
/// trailing semicolon — because the rule is about the definition being one query, not
/// about the text it is written as.
const ONE_QUERY: &[(&str, &str)] = &[
    ("select all", "SELECT * FROM logs"),
    (
        "projection and predicate",
        "SELECT service, message FROM logs WHERE level = 'ERROR'",
    ),
    (
        "aggregate and grouping",
        "SELECT service, count(*) AS errors FROM logs GROUP BY service HAVING count(*) > 10",
    ),
    (
        "set operation",
        "SELECT id FROM logs UNION SELECT id FROM spans",
    ),
    (
        "common table expression",
        "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM errors",
    ),
    (
        "join, ordering and a bound",
        "SELECT l.id FROM logs l JOIN spans s ON s.trace_id = l.trace_id ORDER BY l.id LIMIT 10",
    ),
    (
        "a leading comment",
        "-- the day's errors\nSELECT count(*) FROM logs WHERE level = 'ERROR'",
    ),
    ("a trailing semicolon", "SELECT 1 AS one;"),
    (
        "a nested query",
        "SELECT id FROM logs WHERE service IN (SELECT service FROM spans)",
    ),
];

/// Definitions a SQL parser does NOT read as one query, so `CreateView` refuses them.
///
/// The rows differ along WHY: the text is not SQL at all, it carries no statement, it is
/// a statement that is not a query, or it is more than one statement.
const NOT_ONE_QUERY: &[(&str, &str)] = &[
    ("prose", "the daily error count"),
    ("a JSON document", "{\"query\": \"SELECT 1\"}"),
    ("a truncated query", "SELECT * FROM"),
    ("an unclosed string", "SELECT * FROM logs WHERE level = 'ER"),
    ("an unbalanced parenthesis", "SELECT count(* FROM logs"),
    ("whitespace only", "   "),
    ("a comment only", "-- nothing but a comment"),
    ("a delete", "DELETE FROM logs WHERE level = 'ERROR'"),
    ("an insert", "INSERT INTO logs (id) VALUES (1)"),
    ("an update", "UPDATE logs SET level = 'INFO'"),
    ("a DDL statement", "CREATE TABLE logs (id INT)"),
    ("two queries", "SELECT 1; SELECT 2"),
    ("a query followed by a mutation", "SELECT * FROM logs; DROP TABLE logs"),
    ("three statements", "SELECT 1; SELECT 2; SELECT 3"),
];

/// A view name satisfying `ViewName`'s `^view\.[a-z0-9][a-z0-9_-]{0,250}$` with its
/// `@length(6..256)`, from `smithy_query(query="shape",shape="ViewName")`.
pub(crate) fn view_name(suffix: &str) -> String {
    format!("view.{suffix}")
}

#[tokio::test]
async fn a_definition_that_is_one_sql_query_is_persisted_and_read_back() {
    let omni = Omni::new();
    for (case, definition) in ONE_QUERY {
        let name = view_name(&case.replace(' ', "-").replace(',', ""));
        assert_eq!(
            view_definition_rejection(definition),
            None,
            "case {case:?}: the definition rule must admit {definition:?}"
        );
        let created = omni
            .create_view(&CreateViewInput {
                description: Some(format!("the {case} view")),
                ..create_input(&name, definition)
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));

        assert_eq!(
            created.definition, *definition,
            "case {case:?}: the created view must carry the definition it was given, got {:?}",
            created.definition
        );
        assert_eq!(
            created.name, name,
            "case {case:?}: the created view must carry the name it was given, got {:?}",
            created.name
        );
        assert_eq!(
            created.r#type, VIEW_TYPE_USER,
            "case {case:?}: a view created through the API is a {VIEW_TYPE_USER} view, got {:?}",
            created.r#type
        );
        assert!(
            VIEW_TYPE_VALUES.contains(&created.r#type.as_str()),
            "case {case:?}: the reported type must be a modelled ViewType, got {:?}",
            created.r#type
        );
        assert!(
            created.arn.starts_with(&format!(
                "arn:aws:cloudwatch:{REGION}:{}:{VIEW_ARN_RESOURCE_TYPE}/{name}/",
                engine::emulator_account_id()
            )),
            "case {case:?}: the ARN must name this region, account, view and view id, got {:?}",
            created.arn
        );
        assert_eq!(
            created.created_at, created.updated_at,
            "case {case:?}: a view nothing has updated was last updated when it was created, got {:?} and {:?}",
            created.created_at, created.updated_at
        );

        // The round trip: the reader answers from the store the create wrote.
        let read = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"));
        assert_eq!(
            (
                read.name.as_str(),
                read.definition.as_str(),
                read.description.as_deref(),
                read.r#type.as_str(),
                read.arn.as_str(),
                read.created_at,
                read.updated_at
            ),
            (
                created.name.as_str(),
                created.definition.as_str(),
                created.description.as_deref(),
                created.r#type.as_str(),
                created.arn.as_str(),
                created.created_at,
                created.updated_at
            ),
            "case {case:?}: GetView must answer with the persisted view, got {read:?}"
        );

        // A read reports: reading twice reports the same view.
        let read_again = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the second GetView failed: {error:?}"));
        assert_eq!(
            read_again.updated_at, read.updated_at,
            "case {case:?}: reading a view must not change it, got {:?} after {:?}",
            read_again.updated_at, read.updated_at
        );
    }
}

#[tokio::test]
async fn a_definition_that_is_not_one_sql_query_is_refused_and_persists_nothing() {
    let omni = Omni::new();
    for (index, (case, definition)) in NOT_ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("refused-{index}"));
        let result = omni.create_view(&create_input(&name, definition)).await;
        // The refusal carries what the definition rule says about this text, so a caller
        // is told why its definition was not taken.
        let rendered = format!("{result:?}");
        let reason = view_definition_rejection(definition)
            .unwrap_or_else(|| panic!("case {case:?}: the definition rule must refuse this text"));
        assert!(
            reason.starts_with(INVALID_VIEW_DEFINITION)
                || reason == VIEW_DEFINITION_MUST_BE_A_SELECT_QUERY
                || reason == VIEW_DEFINITION_MUST_BE_ONE_STATEMENT,
            "case {case:?}: the refusal must be one of the definition rule's three forms, got \
             {reason:?}"
        );
        assert!(
            rendered.contains(&reason),
            "case {case:?}: expected the refusal to carry {reason:?}, got {rendered}"
        );
        expect_modelled_error(case, result, 400, "ValidationException");
        // Nothing was persisted for the refused request, so the name resolves to
        // nothing.
        expect_modelled_error(
            case,
            omni.get_view(&name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn a_views_name_is_its_identity_within_the_account_and_region() {
    let omni = Omni::new();
    let name = view_name("errors-by-service");
    let original = omni
        .create_view(&create_input(&name, "SELECT service FROM logs"))
        .await
        .expect("the first view is created");

    for (case, definition) in [
        ("the same definition", "SELECT service FROM logs"),
        ("another definition", "SELECT count(*) FROM spans"),
    ] {
        expect_modelled_error(
            case,
            omni.create_view(&create_input(&name, definition)).await,
            409,
            "ConflictException",
        );
    }

    // The conflict left the first view as it was rather than replacing it.
    let read = omni.get_view(&name).await.expect("the first view is read");
    assert_eq!(
        (read.definition.as_str(), read.updated_at),
        (original.definition.as_str(), original.updated_at),
        "a refused duplicate must leave the live view unchanged, got {read:?}"
    );

    // Another name is another view, definition and all.
    let other = view_name("spans-by-service");
    let second = omni
        .create_view(&create_input(&other, "SELECT count(*) FROM spans"))
        .await
        .expect("a second name is a second view");
    assert_ne!(
        second.arn, original.arn,
        "two views must carry two ARNs, got {:?} twice",
        second.arn
    );
}

/// The `message` of a `CreateView` conflict, with the error code and the modelled status
/// checked first.
async fn conflict_message(omni: &Omni, case: &str, input: &CreateViewInput) -> String {
    match omni.create_view(input).await {
        Ok(output) => panic!("case {case:?}: expected a ConflictException, got Ok({output:?})"),
        Err(error) => {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("ConflictException", 409),
                "case {case:?}: expected ConflictException (409), got {error:?}"
            );
            match error {
                Scaffolded::Modeled(CreateViewErrorModeled::ConflictException(conflict)) => {
                    conflict.message
                }
                other => {
                    panic!("case {case:?}: expected the modelled ConflictException, got {other:?}")
                }
            }
        }
    }
}

#[tokio::test]
async fn a_taken_view_name_is_refused_with_the_name_it_is_held_under() {
    let omni = Omni::new();
    // Rows span the `ViewName` class the model admits (`@length(6..256)`,
    // `^view\.[a-z0-9][a-z0-9_-]{0,250}$`): the shortest name, one carrying the
    // punctuation the pattern allows, one whose first character is a digit, and the
    // longest name — because the conflict states the name the request carried whatever
    // form it takes.
    for (case, name) in [
        ("the shortest name the model admits", view_name("a")),
        ("a name with punctuation", view_name("errors_by-service")),
        ("a name starting with a digit", view_name("0-errors")),
        (
            "the longest name the model admits",
            format!("view.{}", "v".repeat(251)),
        ),
    ] {
        omni.create_view(&create_input(&name, "SELECT service FROM logs"))
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the first view failed: {error:?}"));
        // The template: the fixed "View already exists: " and the name the request
        // carried — unquoted, with no trailing period.
        let expected = format!("View already exists: {name}");
        for (replay, definition) in [
            ("the same definition", "SELECT service FROM logs"),
            ("another definition", "SELECT count(*) FROM spans"),
        ] {
            let label = format!("{case}, {replay}");
            let message = conflict_message(&omni, &label, &create_input(&name, definition)).await;
            assert_eq!(
                message, expected,
                "case {label:?}: expected {expected:?}, got {message:?}"
            );
        }

        // The same refusal reaches a real client over the rpcv2Cbor wire.
        let (status, body) = omni
            .over_the_wire(
                "CreateView",
                CREATE_VIEW_INPUT_SCHEMA,
                &create_input(&name, "SELECT service FROM logs"),
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (409, "ConflictException", expected.as_str()),
            "case {case:?}: the wire must answer 409 ConflictException with {expected:?}, got \
             {status} {code:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_view_name_resolves_to_nothing() {
    let omni = Omni::new();
    for (case, name) in [
        ("never created", view_name("no-such-view")),
        ("the shortest admitted name", view_name("a")),
    ] {
        expect_modelled_error(
            case,
            omni.get_view(&name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn the_name_conflict_wins_over_an_idempotency_token_replay() {
    let omni = Omni::new();
    let name = view_name("token-replayed");
    let request = CreateViewInput {
        client_token: Some("a-client-token".to_string()),
        ..create_input(&name, "SELECT * FROM logs")
    };
    let original = omni
        .create_view(&request)
        .await
        .expect("the first call creates the view");

    // A view's name is its identity, so a repeat of a create that LANDED asks for a name
    // the account already holds — and that conflict is the answer whatever the request
    // carries, the token of the create that minted it included. Rows span what a repeat
    // can differ in beside the token: nothing at all, the definition, and the
    // description.
    for (case, replay) in [
        ("the same request, token and all", request.clone()),
        (
            "the same token with another definition",
            CreateViewInput {
                definition: "SELECT count(*) FROM spans".to_string(),
                ..request.clone()
            },
        ),
        (
            "the same token with another description",
            CreateViewInput {
                description: Some("a second description".to_string()),
                ..request.clone()
            },
        ),
    ] {
        let message = conflict_message(&omni, case, &replay).await;
        let expected = format!("View already exists: {name}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The refusal left the first request's view exactly as it was.
        let read = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"));
        assert_eq!(
            (
                read.definition.as_str(),
                read.description.as_deref(),
                read.arn.as_str(),
                read.created_at,
                read.updated_at
            ),
            (
                original.definition.as_str(),
                original.description.as_deref(),
                original.arn.as_str(),
                original.created_at,
                original.updated_at
            ),
            "case {case:?}: the refused repeat changed the stored view, got {read:?}"
        );
    }

    // The token is not what refused it: the SAME token under a free name creates a view,
    // and the name it was first used under is free again once the view is deleted.
    let fresh_name = view_name("token-fresh");
    let fresh = omni
        .create_view(&CreateViewInput {
            client_token: Some("a-client-token".to_string()),
            ..create_input(&fresh_name, "SELECT * FROM logs")
        })
        .await
        .expect("the same token under a free name creates a view");
    assert_ne!(
        fresh.arn, original.arn,
        "a second name must be a second view, got {:?} twice",
        fresh.arn
    );
    omni.delete_view(&name)
        .await
        .expect("the first view is deleted");
    let recreated = omni
        .create_view(&request)
        .await
        .expect("the freed name accepts the same request again");
    assert_ne!(
        recreated.arn, original.arn,
        "the recreate must mint a new view rather than resurrect the deleted one, got {:?} \
         twice",
        recreated.arn
    );
}

#[tokio::test]
async fn the_view_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let name = view_name("over-the-wire");

    let (status, body) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&name, "SELECT count(*) FROM logs"),
        )
        .await;
    assert_eq!(status, 200, "CreateView over the wire: expected 200");
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let created = CreateViewOutput::deserialize(&mut deserializer)
        .expect("the CreateView response deserializes");

    let (status, body) = omni
        .over_the_wire(
            "GetView",
            GET_VIEW_INPUT_SCHEMA,
            &GetViewInput { name: name.clone() },
        )
        .await;
    assert_eq!(status, 200, "GetView over the wire: expected 200");
    let mut deserializer = codec.create_deserializer(&body);
    let read =
        GetViewOutput::deserialize(&mut deserializer).expect("the GetView response deserializes");
    assert_eq!(
        (read.name.as_str(), read.definition.as_str()),
        (created.name.as_str(), created.definition.as_str()),
        "GetView over the wire must answer with the persisted view, got {read:?}"
    );

    // The refusal travels as the modelled 400 over the same wire.
    let (status, _) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&view_name("refused-over-the-wire"), "not a sql query"),
        )
        .await;
    assert_eq!(
        status, 400,
        "a definition that is not one SQL query must be refused with 400"
    );

    // And the duplicate name travels as the modelled 409.
    let (status, _) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&name, "SELECT count(*) FROM logs"),
        )
        .await;
    assert_eq!(
        status, 409,
        "a second view under a live name must be refused with 409"
    );

    // The listing travels over the same wire and reports the view the create persisted.
    let (status, body) = omni
        .over_the_wire(
            "ListViews",
            LIST_VIEWS_INPUT_SCHEMA,
            &ListViewsInput::default(),
        )
        .await;
    assert_eq!(status, 200, "ListViews over the wire: expected 200");
    let mut deserializer = codec.create_deserializer(&body);
    let listed = ListViewsOutput::deserialize(&mut deserializer)
        .expect("the ListViews response deserializes");
    assert!(
        listed.items.iter().any(|item| item.name == name),
        "ListViews over the wire must report the created view, got {listed:?}"
    );
}

/// `ViewType`'s value that is not [`VIEW_TYPE_USER`], read off the modelled enum
/// (`VIEW_TYPE_VALUES`, generated from `ViewType`) rather than written out here: AWS
/// provisions views of that category, so this store holds none of them.
fn the_other_view_type() -> &'static str {
    VIEW_TYPE_VALUES
        .iter()
        .copied()
        .find(|value| *value != VIEW_TYPE_USER)
        .expect("ViewType declares a category other than USER")
}

#[tokio::test]
async fn the_listing_reports_the_views_the_store_holds_in_creation_order() {
    let omni = Omni::new();
    // Created in a known order, with a description on some of them, so the listing's
    // order and its member set are both observable.
    let mut created: Vec<String> = Vec::new();
    for (index, (case, definition)) in ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("listed-{index}"));
        omni.create_view(&CreateViewInput {
            description: (index % 2 == 0).then(|| format!("the {case} view")),
            ..create_input(&name, definition)
        })
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));
        created.push(name);
    }

    let listed = omni
        .list_views(&ListViewsInput::default())
        .await
        .expect("ListViews answers");
    assert_eq!(
        listed
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<String>>(),
        created,
        "the listing must report every view in creation order, got {listed:?}"
    );
    assert_eq!(
        listed.next_token, None,
        "a listing that reported every view must carry no nextToken, got {:?}",
        listed.next_token
    );

    // A summary is the view's member set WITHOUT the definition, read off the same
    // record `GetView` answers from.
    for item in &listed.items {
        let read = omni
            .get_view(&item.name)
            .await
            .unwrap_or_else(|error| panic!("case {:?}: GetView failed: {error:?}", item.name));
        assert_eq!(
            (
                item.r#type.as_str(),
                item.description.as_deref(),
                item.created_at,
                item.updated_at
            ),
            (
                read.r#type.as_str(),
                read.description.as_deref(),
                read.created_at,
                read.updated_at
            ),
            "case {:?}: the summary and the view must agree, got {item:?}",
            item.name
        );
    }

    // Listing is reporting: the second listing answers exactly what the first did.
    let again = omni
        .list_views(&ListViewsInput::default())
        .await
        .expect("the second ListViews answers");
    assert_eq!(
        again
            .items
            .iter()
            .map(|item| (item.name.clone(), item.updated_at))
            .collect::<Vec<_>>(),
        listed
            .items
            .iter()
            .map(|item| (item.name.clone(), item.updated_at))
            .collect::<Vec<_>>(),
        "listing views must not change them, got {again:?}"
    );
}

#[tokio::test]
async fn the_listing_is_restricted_to_the_ownership_category_the_request_names() {
    let omni = Omni::new();
    let names = ["categorised-a", "categorised-b", "categorised-c"];
    for name in names {
        omni.create_view(&create_input(&view_name(name), "SELECT 1"))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: CreateView failed: {error:?}"));
    }

    for (case, requested, expected) in [
        ("no filter", None, names.len()),
        ("the created category", Some(VIEW_TYPE_USER), names.len()),
        ("the other category", Some(the_other_view_type()), 0),
    ] {
        let listed = omni
            .list_views(&ListViewsInput {
                r#type: requested.map(str::to_string),
                ..ListViewsInput::default()
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
        assert_eq!(
            listed.items.len(),
            expected,
            "case {case:?}: expected {expected} views for type {requested:?}, got {listed:?}"
        );
        for item in &listed.items {
            assert!(
                requested.is_none_or(|category| item.r#type == category),
                "case {case:?}: a filtered listing must report only {requested:?}, got {item:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_pages_at_max_results_and_resumes_where_the_page_stopped() {
    let omni = Omni::new();
    let total = 7;
    let mut created: Vec<String> = Vec::new();
    for index in 0..total {
        let name = view_name(&format!("paged-{index}"));
        omni.create_view(&create_input(&name, "SELECT 1"))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: CreateView failed: {error:?}"));
        created.push(name);
    }

    // The rows span the page sizes around the collection: smaller than it, a divisor of
    // it, one short of it, exactly it, larger than it, and the modelled maximum.
    for page_size in [1, 2, 3, total - 1, total, total + 1, 100] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = omni
                .list_views(&ListViewsInput {
                    max_results: Some(page_size as i32),
                    next_token: token.clone(),
                    ..ListViewsInput::default()
                })
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
            pages += 1;
            assert!(
                page.items.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} views, got {page:?}"
            );
            assert!(
                pages <= total + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            seen.extend(page.items.iter().map(|item| item.name.clone()));
            match page.next_token {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: the pages together must be every view in creation order"
        );
        assert_eq!(
            pages,
            total.div_ceil(page_size),
            "case {case:?}: expected {} pages for {total} views, got {pages}",
            total.div_ceil(page_size)
        );
    }

    // A token this service did not mint names no resume point, so it is a bad request
    // rather than an empty page.
    for (case, token) in [
        ("not hex", "not-a-token"),
        ("hex that decodes to other text", "6e6f70652d31"),
    ] {
        expect_modelled_error(
            case,
            omni.list_views(&ListViewsInput {
                next_token: Some(token.to_string()),
                ..ListViewsInput::default()
            })
            .await,
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn a_deleted_view_resolves_to_nothing_and_frees_its_name() {
    let omni = Omni::new();
    // The rows differ along what else the view carries, because what a delete releases
    // is the view its NAME identifies: a bare view, one with a description, and one a
    // client token minted.
    let cases: Vec<(&str, CreateViewInput)> = vec![
        (
            "a bare view",
            create_input(&view_name("deleted-bare"), "SELECT 1"),
        ),
        (
            "a described view",
            CreateViewInput {
                description: Some("the described view".to_string()),
                ..create_input(&view_name("deleted-described"), "SELECT 2")
            },
        ),
        (
            "a view a token minted",
            CreateViewInput {
                client_token: Some("deleted-token".to_string()),
                ..create_input(&view_name("deleted-tokened"), "SELECT 3")
            },
        ),
    ];

    for (case, request) in &cases {
        let created = omni
            .create_view(request)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));
        omni.delete_view(&created.name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteView failed: {error:?}"));

        // The view is gone from both readers: the one that resolves a name and the one
        // that enumerates the store.
        expect_modelled_error(
            case,
            omni.get_view(&created.name).await,
            404,
            "ResourceNotFoundException",
        );
        let listed = omni
            .list_views(&ListViewsInput::default())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
        assert!(
            !listed.items.iter().any(|item| item.name == created.name),
            "case {case:?}: a deleted view must not be listed, got {listed:?}"
        );

        // Deleting it again is a miss: the name holds no view now.
        expect_modelled_error(
            case,
            omni.delete_view(&created.name).await,
            404,
            "ResourceNotFoundException",
        );

        // The name is free again, token and all: the same request creates a new view
        // rather than resurrecting the deleted one.
        let recreated = omni
            .create_view(request)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the recreate failed: {error:?}"));
        assert_eq!(
            recreated.name, created.name,
            "case {case:?}: the freed name must be usable again, got {:?}",
            recreated.name
        );
        assert_eq!(
            omni.get_view(&created.name)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"))
                .definition,
            recreated.definition,
            "case {case:?}: the recreated view must be the one the store now holds"
        );
    }

    // A name no view was ever created under is a miss too.
    expect_modelled_error(
        "never created",
        omni.delete_view(&view_name("never-created")).await,
        404,
        "ResourceNotFoundException",
    );
}

// --- telemetry query sessions ------------------------------------------------------

/// A second principal: the access key id a request signs with is what this service reads
/// the caller's identity from, so this is a different owner of sessions.
const OTHER_ACCESS_KEY_ID: &str = "AKIDSECONDCALLER";

impl Omni {
    /// The request metadata of a call another principal signed, as a context the
    /// operations take.
    fn ctx_with<'a>(&'a self, parts: &'a http::request::Parts) -> OperationContext<'a> {
        OperationContext::new(
            parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// `StartTelemetryQuerySession`, signed by `access_key_id`.
    async fn start_session_signed_by(
        &self,
        access_key_id: &str,
        session_name: Option<&str>,
    ) -> Result<StartTelemetryQuerySessionOutput, StartTelemetryQuerySessionError> {
        let parts = signed_request_parts_for(access_key_id);
        StartTelemetryQuerySession::new(Arc::clone(&self.engine))
            .handle(
                StartTelemetryQuerySessionInput {
                    session_name: session_name.map(str::to_string),
                },
                &self.ctx_with(&parts),
            )
            .await
    }

    /// `StartTelemetryQuerySession`, signed by the suite's own principal.
    async fn start_session(
        &self,
        session_name: Option<&str>,
    ) -> Result<StartTelemetryQuerySessionOutput, StartTelemetryQuerySessionError> {
        self.start_session_signed_by(ACCESS_KEY_ID, session_name)
            .await
    }

    /// `ListTelemetryQuerySessions`, signed by `access_key_id`.
    async fn list_sessions_signed_by(
        &self,
        access_key_id: &str,
        input: &ListTelemetryQuerySessionsInput,
    ) -> Result<ListTelemetryQuerySessionsOutput, ListTelemetryQuerySessionsError> {
        let parts = signed_request_parts_for(access_key_id);
        ListTelemetryQuerySessions::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx_with(&parts))
            .await
    }

    /// `ListTelemetryQuerySessions`, signed by the suite's own principal.
    async fn list_sessions(
        &self,
        input: &ListTelemetryQuerySessionsInput,
    ) -> Result<ListTelemetryQuerySessionsOutput, ListTelemetryQuerySessionsError> {
        self.list_sessions_signed_by(ACCESS_KEY_ID, input).await
    }
}

/// Whether `session_id` has the form the model fixes for a session id:
/// `@length(1..64)` and `^[a-z0-9-]+$` on `StartTelemetryQuerySessionResponse$sessionId`,
/// from `smithy_query(query="operation",operation="StartTelemetryQuerySession")`.
fn is_modelled_session_id(session_id: &str) -> bool {
    (1..=64).contains(&session_id.len())
        && session_id
            .chars()
            .all(|character| matches!(character, 'a'..='z' | '0'..='9' | '-'))
}

/// The names a start may carry, spanning the `sessionName` class the model admits —
/// `@length(1..128)` and
/// `^([a-zA-Z0-9_-]+|/aws/[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)?)$`, from
/// `smithy_query(query="shape",shape="StartTelemetryQuerySessionRequest")` — minus the
/// `/aws/` namespace the member's doc reserves for service integrations (asserted on its
/// own below), plus the start that names no session at all, which the member being
/// optional admits.
///
/// The rows differ along what a name can differ in: whether there is one, its length at
/// both published bounds, its case, and the punctuation its pattern admits.
fn session_name_cases() -> Vec<(String, Option<String>)> {
    [
        ("no name at all", None),
        ("the shortest name the model admits", Some("s".to_string())),
        ("a digits-only name", Some("20260101".to_string())),
        (
            "a name carrying the punctuation its pattern admits",
            Some("nightly_error-scan-7".to_string()),
        ),
        ("a mixed-case name", Some("NightlyErrorScan".to_string())),
        (
            "a name that merely mentions aws",
            Some("aws-nightly-scan".to_string()),
        ),
        ("the longest name the model admits", Some("s".repeat(128))),
    ]
    .into_iter()
    .map(|(case, name)| (case.to_string(), name))
    .collect()
}

/// The names reserved for service integrations, in both the forms the member's pattern
/// allows, so a caller may take none of them.
fn reserved_session_name_cases() -> Vec<(String, String)> {
    [
        (
            "the shortest reserved name the pattern allows",
            "/aws/a".to_string(),
        ),
        (
            "a one-segment reserved name",
            "/aws/incident-detection".to_string(),
        ),
        (
            "a two-segment reserved name",
            "/aws/incident-detection/triage".to_string(),
        ),
        (
            "the longest reserved name the model admits",
            format!("/aws/{}", "s".repeat(123)),
        ),
    ]
    .into_iter()
    .map(|(case, name)| (case.to_string(), name))
    .collect()
}

#[tokio::test]
async fn a_started_session_is_persisted_and_the_listing_reads_it_back() {
    let omni = Omni::new();

    // (case, the id the start minted, the name it was started under)
    let mut started: Vec<(String, String, Option<String>)> = Vec::new();
    for (case, session_name) in session_name_cases() {
        let out = omni
            .start_session(session_name.as_deref())
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: StartTelemetryQuerySession failed: {error:?}")
            });
        assert!(
            is_modelled_session_id(&out.session_id),
            "case {case:?}: the minted session id must have the form the model fixes, got {:?}",
            out.session_id
        );
        assert!(
            started.iter().all(|(_, id, _)| *id != out.session_id),
            "case {case:?}: every start mints its own session id, got the repeated {:?}",
            out.session_id
        );
        started.push((case, out.session_id, session_name));
    }

    let listed = omni
        .list_sessions(&ListTelemetryQuerySessionsInput::default())
        .await
        .unwrap_or_else(|error| panic!("ListTelemetryQuerySessions failed: {error:?}"));
    assert_eq!(
        listed.sessions.len(),
        started.len(),
        "the listing must report every started session, got {listed:?}"
    );
    assert_eq!(
        listed.next_token, None,
        "a listing that reported everything carries no resume token, got {:?}",
        listed.next_token
    );

    for (index, (case, session_id, session_name)) in started.iter().enumerate() {
        let summary = &listed.sessions[index];
        assert_eq!(
            &summary.session_id, session_id,
            "case {case:?}: the listing reports the sessions in start order, got {:?} at \
             position {index}",
            summary.session_id
        );
        assert_eq!(
            summary.session_name.as_deref(),
            session_name.as_deref(),
            "case {case:?}: the listing must report the name the start carried, got {:?}",
            summary.session_name
        );
        let created_at = summary
            .created_at
            .unwrap_or_else(|| panic!("case {case:?}: the summary must report createdAt"));
        let last_activity_at = summary
            .last_activity_at
            .unwrap_or_else(|| panic!("case {case:?}: the summary must report lastActivityAt"));
        assert_eq!(
            created_at, last_activity_at,
            "case {case:?}: starting a session is the first activity in it, so the two \
             instants are one until something moves it, got {created_at:?} and \
             {last_activity_at:?}"
        );
        assert!(
            created_at.secs() > 0,
            "case {case:?}: the session must be recorded at the instant it started, got \
             {created_at:?}"
        );
    }
}

#[tokio::test]
async fn a_session_name_reserved_for_service_integrations_is_refused_and_mints_nothing() {
    let omni = Omni::new();
    for (case, session_name) in reserved_session_name_cases() {
        // Over the wire, so the status, the error code AND the message are the facts a
        // client reads.
        let (status, body) = omni
            .over_the_wire(
                "StartTelemetryQuerySession",
                START_TELEMETRY_QUERY_SESSION_INPUT_SCHEMA,
                &StartTelemetryQuerySessionInput {
                    session_name: Some(session_name.clone()),
                },
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reserved session name must be refused with 400, got {status}"
        );
        let (code, message) = error_envelope(&case, &body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {code:?}"
        );
        assert_eq!(
            message, SESSION_NAME_NOT_VALID_MESSAGE,
            "case {case:?}: expected the refusal to name the parameter that was wrong, \
             got {message:?}"
        );
        // The sentence itself, spelled out once rather than only compared against the
        // constant the handler carries, so the WORDING is a checked fact.
        assert_eq!(
            message, "The specified session name is not valid.",
            "case {case:?}: expected the service's own wording, got {message:?}"
        );
    }

    // Nothing was recorded for any of them: the refusals left the caller with no session.
    let listed = omni
        .list_sessions(&ListTelemetryQuerySessionsInput::default())
        .await
        .unwrap_or_else(|error| panic!("ListTelemetryQuerySessions failed: {error:?}"));
    assert!(
        listed.sessions.is_empty(),
        "a refused start persists nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_session_name_the_caller_already_holds_is_refused_and_mints_nothing() {
    let omni = Omni::new();

    for (case, session_name) in session_name_cases() {
        let Some(session_name) = session_name else {
            // A start that names no session has no identity to collide with: two of them
            // both mint, which is the other half of this rule.
            let first = omni
                .start_session(None)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: the first start failed: {error:?}"));
            let second = omni
                .start_session(None)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: the second start failed: {error:?}"));
            assert_ne!(
                first.session_id, second.session_id,
                "case {case:?}: an unnamed start cannot conflict, so it mints another session"
            );
            continue;
        };

        let held = omni
            .start_session(Some(&session_name))
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the first start failed: {error:?}"));

        // Over the wire, so the status, the error code AND the message are the facts a
        // client reads.
        let (status, body) = omni
            .over_the_wire(
                "StartTelemetryQuerySession",
                START_TELEMETRY_QUERY_SESSION_INPUT_SCHEMA,
                &StartTelemetryQuerySessionInput {
                    session_name: Some(session_name.clone()),
                },
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a second start under a name the caller holds must be refused \
             with 409, got {status}"
        );
        let (code, message) = error_envelope(&case, &body);
        assert_eq!(
            code, "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {code:?}"
        );
        assert_eq!(
            message, SESSION_NAME_ALREADY_EXISTS_MESSAGE,
            "case {case:?}: expected the refusal to state that the name is taken, \
             got {message:?}"
        );
        // The sentence itself, spelled out once rather than only compared against the
        // constant the handler carries, so the WORDING is a checked fact.
        assert_eq!(
            message, "A session with this name already exists.",
            "case {case:?}: expected the service's own wording, got {message:?}"
        );

        // And nothing was minted: the caller still holds exactly the first session under
        // that name.
        let listed = omni
            .list_sessions(&ListTelemetryQuerySessionsInput::default())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing failed: {error:?}"));
        let holders: Vec<&str> = listed
            .sessions
            .iter()
            .filter(|summary| summary.session_name.as_deref() == Some(session_name.as_str()))
            .map(|summary| summary.session_id.as_str())
            .collect();
        assert_eq!(
            holders,
            vec![held.session_id.as_str()],
            "case {case:?}: the refused start must leave the one session the caller held, \
             got {holders:?}"
        );
    }
}

#[tokio::test]
async fn a_session_listing_reports_only_the_callers_own_sessions() {
    let omni = Omni::new();
    // The same name for both callers: a session name identifies a session among the ones
    // ONE caller holds, so it is free for another.
    let shared_name = "shared-session-name";

    let mine = omni
        .start_session(Some(shared_name))
        .await
        .unwrap_or_else(|error| panic!("the first caller's start failed: {error:?}"));
    let theirs = omni
        .start_session_signed_by(OTHER_ACCESS_KEY_ID, Some(shared_name))
        .await
        .unwrap_or_else(|error| {
            panic!("another caller must be able to hold a session under the same name: {error:?}")
        });
    assert_ne!(
        mine.session_id, theirs.session_id,
        "two callers' sessions are two sessions, got the same id {:?}",
        mine.session_id
    );

    for (case, access_key_id, expected) in [
        ("the caller that started the first", ACCESS_KEY_ID, &mine),
        (
            "the caller that started the second",
            OTHER_ACCESS_KEY_ID,
            &theirs,
        ),
    ] {
        let listed = omni
            .list_sessions_signed_by(access_key_id, &ListTelemetryQuerySessionsInput::default())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing failed: {error:?}"));
        let ids: Vec<&str> = listed
            .sessions
            .iter()
            .map(|summary| summary.session_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![expected.session_id.as_str()],
            "case {case:?}: a listing reports the caller's own sessions and no other's, \
             got {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_session_listing_pages_at_max_results_and_resumes_at_its_token() {
    let omni = Omni::new();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..5 {
        ids.push(
            omni.start_session(Some(&format!("paged-session-{index}")))
                .await
                .unwrap_or_else(|error| panic!("the start of session {index} failed: {error:?}"))
                .session_id,
        );
    }

    // The rows differ along the page bound: smaller than the set, a divisor of it, the
    // whole set, and larger than it.
    for (case, page_size) in [
        ("one at a time", 1),
        ("two at a time", 2),
        ("the whole set in one page", 5),
        ("a page larger than the set", 100),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        // Bounded by the number of sessions: one page per session is the most a correct
        // listing can take, so a token that never ends fails here rather than hanging.
        for step in 0..=ids.len() {
            let page = omni
                .list_sessions(&ListTelemetryQuerySessionsInput {
                    max_results: Some(page_size),
                    next_token: next_token.clone(),
                })
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: page {step} failed: {error:?}"));
            assert!(
                page.sessions.len() <= page_size as usize,
                "case {case:?}: a page must hold at most {page_size} sessions, got {}",
                page.sessions.len()
            );
            walked.extend(
                page.sessions
                    .iter()
                    .map(|summary| summary.session_id.clone()),
            );
            next_token = page.next_token;
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            next_token, None,
            "case {case:?}: the walk must end, got the further token {next_token:?}"
        );
        assert_eq!(
            walked, ids,
            "case {case:?}: the pages must walk every session once, in start order, got \
             {walked:?}"
        );
    }

    // A token this service did not mint for a session listing names no resume point.
    expect_modelled_error(
        "a token this service did not mint",
        omni.list_sessions(&ListTelemetryQuerySessionsInput {
            next_token: Some("not-a-token".to_string()),
            max_results: None,
        })
        .await,
        400,
        "ValidationException",
    );
}

#[tokio::test]
async fn the_query_session_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let session_name = "over-the-wire-session";

    let (status, body) = omni
        .over_the_wire(
            "StartTelemetryQuerySession",
            START_TELEMETRY_QUERY_SESSION_INPUT_SCHEMA,
            &StartTelemetryQuerySessionInput {
                session_name: Some(session_name.to_string()),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "StartTelemetryQuerySession over the wire: expected 200, got {status}: {}",
        String::from_utf8_lossy(&body)
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let started = StartTelemetryQuerySessionOutput::deserialize(&mut deserializer)
        .expect("the StartTelemetryQuerySession response deserializes");
    assert!(
        is_modelled_session_id(&started.session_id),
        "the minted session id must have the form the model fixes, got {:?}",
        started.session_id
    );

    let (status, body) = omni
        .over_the_wire(
            "ListTelemetryQuerySessions",
            LIST_TELEMETRY_QUERY_SESSIONS_INPUT_SCHEMA,
            &ListTelemetryQuerySessionsInput::default(),
        )
        .await;
    assert_eq!(
        status, 200,
        "ListTelemetryQuerySessions over the wire: expected 200, got {status}"
    );
    let mut deserializer = codec.create_deserializer(&body);
    let listed = ListTelemetryQuerySessionsOutput::deserialize(&mut deserializer)
        .expect("the ListTelemetryQuerySessions response deserializes");
    assert!(
        listed.sessions.iter().any(|summary| summary.session_id
            == started.session_id
            && summary.session_name.as_deref() == Some(session_name)),
        "the listing over the wire must report the session the start persisted, got \
         {listed:?}"
    );
}

/// Patch a view over the rpcv2Cbor wire, requiring the modelled success, and answer with
/// the record the operation reported.
async fn patched_over_the_wire(
    omni: &Omni,
    case: &str,
    input: &UpdateViewInput,
) -> UpdateViewOutput {
    let (status, body) = omni
        .over_the_wire("UpdateView", UPDATE_VIEW_INPUT_SCHEMA, input)
        .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: UpdateView over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    UpdateViewOutput::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the UpdateView response does not deserialize: {error:?}")
    })
}

/// The view the store holds under `name`, read back over the same wire.
async fn read_over_the_wire(omni: &Omni, case: &str, name: &str) -> GetViewOutput {
    let (status, body) = omni
        .over_the_wire(
            "GetView",
            GET_VIEW_INPUT_SCHEMA,
            &GetViewInput {
                name: name.to_string(),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: GetView over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    GetViewOutput::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the GetView response does not deserialize: {error:?}")
    })
}

/// One row of the partial-update table: the view the store starts with, the members the
/// patch carries, and what the view must hold once the patch has been applied.
struct ViewPatch {
    case: &'static str,
    suffix: &'static str,
    initial_definition: String,
    initial_description: Option<String>,
    patch_definition: Option<String>,
    patch_description: Option<String>,
    expected_definition: String,
    expected_description: Option<String>,
}

/// The patches the partial-update rule has to hold for.
///
/// The rows differ along WHICH subset of the two updatable members the request carries —
/// `description` alone, `definition` alone, both, neither — and along the FORM of the new
/// definition and the SIZE of the new description, because the rule is about the members
/// the request carries rather than about the values they take. `ViewDescription`'s
/// `@length(..1024)` is what the longest row is sized at.
fn view_patches() -> Vec<ViewPatch> {
    vec![
        ViewPatch {
            case: "the description alone, leaving the definition and the identity as they were",
            suffix: "patch-description-only",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("first".to_string()),
            patch_definition: None,
            patch_description: Some("second".to_string()),
            expected_definition: "SELECT * FROM logs".to_string(),
            expected_description: Some("second".to_string()),
        },
        ViewPatch {
            case: "the description alone onto a view that carried none",
            suffix: "patch-description-added",
            initial_definition: "SELECT 1".to_string(),
            initial_description: None,
            patch_definition: None,
            patch_description: Some("added".to_string()),
            expected_definition: "SELECT 1".to_string(),
            expected_description: Some("added".to_string()),
        },
        ViewPatch {
            case: "the longest description the model admits",
            suffix: "patch-description-longest",
            initial_definition: "SELECT 1".to_string(),
            initial_description: Some("first".to_string()),
            patch_definition: None,
            patch_description: Some("d".repeat(1024)),
            expected_definition: "SELECT 1".to_string(),
            expected_description: Some("d".repeat(1024)),
        },
        ViewPatch {
            case: "the definition alone, replaced by an aggregate query",
            suffix: "patch-definition-aggregate",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: Some(
                "SELECT service, count(*) AS errors FROM logs GROUP BY service".to_string(),
            ),
            patch_description: None,
            expected_definition: "SELECT service, count(*) AS errors FROM logs GROUP BY service"
                .to_string(),
            expected_description: Some("kept".to_string()),
        },
        ViewPatch {
            case: "the definition alone, replaced by a set operation",
            suffix: "patch-definition-set-operation",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: None,
            patch_definition: Some("SELECT id FROM logs UNION SELECT id FROM spans".to_string()),
            patch_description: None,
            expected_definition: "SELECT id FROM logs UNION SELECT id FROM spans".to_string(),
            expected_description: None,
        },
        ViewPatch {
            case: "the definition alone, replaced by a common table expression",
            suffix: "patch-definition-cte",
            initial_definition: "SELECT 1".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: Some(
                "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM \
                 errors"
                    .to_string(),
            ),
            patch_description: None,
            expected_definition:
                "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM \
                 errors"
                    .to_string(),
            expected_description: Some("kept".to_string()),
        },
        ViewPatch {
            case: "both members at once",
            suffix: "patch-both",
            initial_definition: "SELECT 1".to_string(),
            initial_description: Some("first".to_string()),
            patch_definition: Some("SELECT count(*) FROM spans".to_string()),
            patch_description: Some("second".to_string()),
            expected_definition: "SELECT count(*) FROM spans".to_string(),
            expected_description: Some("second".to_string()),
        },
        ViewPatch {
            case: "neither member, so the whole view comes through",
            suffix: "patch-neither",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: None,
            patch_description: None,
            expected_definition: "SELECT * FROM logs".to_string(),
            expected_description: Some("kept".to_string()),
        },
        ViewPatch {
            case: "the values the view already holds",
            suffix: "patch-unchanged",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: Some("SELECT * FROM logs".to_string()),
            patch_description: Some("kept".to_string()),
            expected_definition: "SELECT * FROM logs".to_string(),
            expected_description: Some("kept".to_string()),
        },
    ]
}

#[tokio::test]
async fn a_patch_replaces_the_members_it_carries_and_leaves_every_other_member_as_it_was() {
    let omni = Omni::new();

    for row in view_patches() {
        let case = row.case;
        let name = view_name(row.suffix);
        let created = omni
            .create_view(&CreateViewInput {
                description: row.initial_description.clone(),
                ..create_input(&name, &row.initial_definition)
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));

        let patched = patched_over_the_wire(
            &omni,
            case,
            &UpdateViewInput {
                name: name.clone(),
                definition: row.patch_definition.clone(),
                description: row.patch_description.clone(),
            },
        )
        .await;

        assert_eq!(
            (patched.definition.as_str(), patched.description.as_deref()),
            (
                row.expected_definition.as_str(),
                row.expected_description.as_deref()
            ),
            "case {case:?}: the patched view is {patched:?}"
        );
        // The members `UpdateView`'s input cannot even name: the identity, the ARN, the
        // ownership category and the instant the view was created.
        assert_eq!(
            (
                patched.name.as_str(),
                patched.arn.as_str(),
                patched.r#type.as_str(),
                patched.created_at
            ),
            (
                created.name.as_str(),
                created.arn.as_str(),
                VIEW_TYPE_USER,
                created.created_at
            ),
            "case {case:?}: a member the patch cannot name was changed, got {patched:?} from \
             {created:?}"
        );
        assert!(
            patched.updated_at.secs() >= patched.created_at.secs(),
            "case {case:?}: updatedAt {:?} precedes createdAt {:?}",
            patched.updated_at,
            patched.created_at
        );

        // The write reached the store the reader answers from, and left ONE view there.
        let read = read_over_the_wire(&omni, case, &name).await;
        assert_eq!(
            (
                read.name.as_str(),
                read.definition.as_str(),
                read.description.as_deref(),
                read.r#type.as_str(),
                read.arn.as_str(),
                read.created_at,
                read.updated_at
            ),
            (
                patched.name.as_str(),
                patched.definition.as_str(),
                patched.description.as_deref(),
                patched.r#type.as_str(),
                patched.arn.as_str(),
                patched.created_at,
                patched.updated_at
            ),
            "case {case:?}: GetView did not answer the record the patch persisted, got {read:?}"
        );
        let listed = omni
            .list_views(&ListViewsInput::default())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
        assert_eq!(
            listed
                .items
                .iter()
                .filter(|item| item.name == name)
                .count(),
            1,
            "case {case:?}: the patch must leave one view under its name, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_patched_definition_that_is_not_one_sql_query_is_refused_and_writes_nothing() {
    let omni = Omni::new();
    // The view every row starts from, and the description it starts with: a refused patch
    // has to leave both exactly as they are.
    let stored_definition = "SELECT service, count(*) AS errors FROM logs GROUP BY service";

    for (index, (case, candidate)) in NOT_ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("patch-refused-{index}"));
        omni.create_view(&CreateViewInput {
            description: Some("kept".to_string()),
            ..create_input(&name, stored_definition)
        })
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));

        let (status, body) = omni
            .over_the_wire(
                "UpdateView",
                UPDATE_VIEW_INPUT_SCHEMA,
                &UpdateViewInput {
                    name: name.clone(),
                    definition: Some((*candidate).to_string()),
                    // The same request asks for a new description too: a refused patch
                    // writes NEITHER member.
                    description: Some("second".to_string()),
                },
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: the wire must answer 400 ValidationException, got {status} {code:?}: \
             {message:?}"
        );
        assert!(
            message.starts_with(INVALID_VIEW_DEFINITION)
                || message == VIEW_DEFINITION_MUST_BE_A_SELECT_QUERY
                || message == VIEW_DEFINITION_MUST_BE_ONE_STATEMENT,
            "case {case:?}: the refusal must be one of the definition rule's three forms, got \
             {message:?}"
        );

        // A new definition is held to the SAME rule a created one is, so the create
        // refuses this text with the same status, code and message.
        let (create_status, create_body) = omni
            .over_the_wire(
                "CreateView",
                CREATE_VIEW_INPUT_SCHEMA,
                &create_input(&view_name(&format!("create-refused-{index}")), candidate),
            )
            .await;
        let (create_code, create_message) = error_envelope(case, &create_body);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                create_status,
                create_code.as_str(),
                create_message.as_str()
            ),
            "case {case:?}: UpdateView refused this definition differently from CreateView"
        );

        let read = read_over_the_wire(&omni, case, &name).await;
        assert_eq!(
            (read.definition.as_str(), read.description.as_deref()),
            (stored_definition, Some("kept")),
            "case {case:?}: the refused patch changed the view, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_patch_of_a_name_the_store_holds_nothing_under_is_the_modelled_404() {
    let omni = Omni::new();
    let live = view_name("patch-live");
    omni.create_view(&CreateViewInput {
        description: Some("kept".to_string()),
        ..create_input(&live, "SELECT 1")
    })
    .await
    .expect("the live view is created");

    // Rows span the `ViewName` class (`@length(6..256)`,
    // `^view\.[a-z0-9][a-z0-9_-]{0,250}$`, from
    // `smithy_query(query="shape",shape="ViewName")`): the shortest name the model
    // admits, one carrying the punctuation the pattern allows, one whose first character
    // is a digit, and the longest name the model admits.
    for (case, name) in [
        ("the shortest name the model admits", view_name("a")),
        ("a name with punctuation", view_name("no_such-view")),
        ("a name starting with a digit", view_name("0-errors")),
        (
            "the longest name the model admits",
            format!("view.{}", "v".repeat(251)),
        ),
    ] {
        // The template: the fixed "View not found: " and the name the request carried —
        // unquoted, with no trailing period, as every operation that reports a missing
        // view words it.
        let expected = format!("View not found: {name}");
        let (status, body) = omni
            .over_the_wire(
                "UpdateView",
                UPDATE_VIEW_INPUT_SCHEMA,
                &UpdateViewInput {
                    name: name.clone(),
                    definition: Some("SELECT count(*) FROM logs".to_string()),
                    description: Some("second".to_string()),
                },
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (404, "ResourceNotFoundException", expected.as_str()),
            "case {case:?}: the wire must answer 404 ResourceNotFoundException with \
             {expected:?}, got {status} {code:?}: {message:?}"
        );
    }

    // None of the refusals touched the view the store does hold.
    let read = read_over_the_wire(&omni, "the live view", &live).await;
    assert_eq!(
        (read.definition.as_str(), read.description.as_deref()),
        ("SELECT 1", Some("kept")),
        "a patch of a name the store holds nothing under changed the live view: {read:?}"
    );
}
