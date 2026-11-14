// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeView`.
//!
//! Every case goes over the wire, through the same `MultiServiceRouter` the production
//! router dispatches with, so the HTTP status, the restJson1 envelope and the stored
//! state are all exercised together.
//!
//! A `ViewToken` is minted where a view is SHOWN to a participant, which no call this
//! service's API carries, so these tests write the shown view into this service's store
//! the way the component that shows one does (`state::ShownView`) and then read it back
//! through the operation.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_connectparticipant_amazonconnectparticipantservicelambda as participant;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host the requests address.
const HOST: &str = "participant.connect.us-east-1.amazonaws.com";

/// A SigV4 credential scope naming this service's signing name, which is what the
/// router resolves a request's target service from.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/\
                             execute-api/aws4_request, SignedHeaders=host, Signature=00";

/// The region and instance the view ARNs below name, as a Connect view ARN carries them.
const REGION: &str = "us-east-1";
/// The instance a view belongs to; an id, not a value any assertion here depends on.
const INSTANCE_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One service mounted on one shared engine, plus that engine — the store the component
/// that shows a view writes into, and the one every handler reads.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(participant::EmulatedService::new(engine.clone()));
        Self { router, engine }
    }

    /// This service's store in the shared engine.
    fn store(
        &self,
    ) -> ::engine::ServiceState<
        participant::state::ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState,
    > {
        participant::state::service_state(&*self.engine).expect("the connectparticipant store")
    }

    /// Open a connection for `participant_token`, returning its `ConnectionToken` and the
    /// contact of the session that token authenticates.
    async fn connect(&self, participant_token: &str) -> (String, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{HOST}/participant/connection"))
            .header("host", HOST)
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .header("x-amz-bearer", participant_token)
            .body(Bytes::from(
                serde_json::to_vec(&json!({"Type": ["CONNECTION_CREDENTIALS"]}))
                    .expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body: Value =
            serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "CreateParticipantConnection for {participant_token:?}: got {body:?}"
        );
        let connection_token = body["ConnectionCredentials"]["ConnectionToken"]
            .as_str()
            .unwrap_or_else(|| panic!("no ConnectionToken: {body:?}"))
            .to_string();
        let contact_id = self
            .store()
            .lock()
            .participant_tokens
            .get(participant_token)
            .cloned()
            .unwrap_or_else(|| panic!("no session for {participant_token:?}"));
        (connection_token, contact_id)
    }

    /// `DescribeView`: `GET /participant/views/{ViewToken}` with the connection token in
    /// the `X-Amz-Bearer` header.
    async fn describe_view(&self, view_token: &str, connection_token: &str) -> (u16, Value) {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("https://{HOST}/participant/views/{view_token}"))
            .header("host", HOST)
            .header("authorization", AUTHORIZATION)
            .header("x-amz-bearer", connection_token)
            .body(Bytes::new())
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let decoded = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    /// Store `view` under `view_token` as a view shown in `contact_id`'s chat.
    fn show_view(&self, view_token: &str, contact_id: &str, view: participant::state::ShownView) {
        self.store()
            .lock()
            .shown_views
            .insert(view_token.to_string(), participant::state::ShownView {
                contact_id: contact_id.to_string(),
                ..view
            });
    }
}

/// The `__type` discriminator of a restJson1 error body.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// `arn:aws:connect:<region>:<account>:instance/<instance>/view/<id>`, the ARN form a
/// Connect view carries, built with the emulator's own ARN helper.
fn view_arn(view_id: &str) -> String {
    ::engine::emulator_arn(
        "connect",
        REGION,
        &format!("instance/{INSTANCE_ID}/view/{view_id}"),
    )
}

/// One stored view, and the token it is shown under.
struct Case {
    /// What the assertions name this row.
    name: &'static str,
    /// The token the view is reached by.
    token: &'static str,
    /// The view as it is stored.
    view: participant::state::ShownView,
}

/// The rows differ along every `View` member `DescribeView` reports: which content
/// members are set, how many actions the content carries, the version, and the shape of
/// the name and the template text.
fn cases() -> Vec<Case> {
    let view = |id: &str,
                name: &str,
                version: i32,
                input_schema: Option<&str>,
                template: Option<&str>,
                actions: &[&str]| participant::state::ShownView {
        contact_id: String::new(),
        id: id.to_string(),
        arn: view_arn(id),
        name: name.to_string(),
        version,
        input_schema: input_schema.map(str::to_string),
        template: template.map(str::to_string),
        actions: actions.iter().map(|action| action.to_string()).collect(),
    };
    vec![
        Case {
            name: "every content member set, one action",
            token: "token-full",
            view: view(
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "OrderLookup",
                1,
                Some(r#"{"type":"object","properties":{"OrderId":{"type":"string"}}}"#),
                Some(r#"{"templateType":"ListPicker","version":"1.0"}"#),
                &["Submit"],
            ),
        },
        Case {
            name: "no content member set at all",
            token: "token-empty-content",
            view: view(
                "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
                "Bare",
                1,
                None,
                None,
                &[],
            ),
        },
        Case {
            name: "several actions, in the order they were stored",
            token: "token-many-actions",
            view: view(
                "cccccccc-dddd-eeee-ffff-000000000000",
                "Survey",
                7,
                Some("{}"),
                Some("{\"templateType\":\"Form\"}\n"),
                &["Next", "Back", "Cancel", "Submit"],
            ),
        },
        Case {
            name: "an AWS-managed view id, a unicode name and a later version",
            token: "token-managed",
            view: view(
                // `View$Id` permits `_-:/$` beside alphanumerics, which is what an
                // AWS-managed view's id uses (`smithy_query(query="shape",
                // shape="ViewId")`: pattern `^[a-zA-Z0-9\_\-:\/$]+$`).
                "arn:aws:connect:::content/interactive/ListPicker:$LATEST",
                "Liste des choix — v2",
                42,
                Some(r#"{"type":"object"}"#),
                Some(r#"{"templateType":"ListPicker","version":"2.0"}"#),
                &["ActionSelected"],
            ),
        },
        Case {
            name: "a name at the modelled 255-character maximum",
            token: "token-long-name",
            // `ViewName` is len 1..255 (`smithy_query(query="shape", shape="ViewName")`).
            view: view(
                "dddddddd-eeee-ffff-0000-111111111111",
                &"x".repeat(255),
                3,
                None,
                Some("{}"),
                &[],
            ),
        },
    ]
}

#[tokio::test]
async fn a_shown_view_is_reported_to_the_participant_it_was_shown_to() {
    let harness = Harness::new();
    let (connection_token, contact_id) = harness.connect("participant-token-views").await;

    for case in cases() {
        harness.show_view(case.token, &contact_id, case.view);
    }

    for case in cases() {
        let label = case.name;
        let stored = case.view;
        let (status, body) = harness.describe_view(case.token, &connection_token).await;
        assert_eq!(status, 200, "case {label:?}: got {body:?}");

        let view = &body["View"];
        assert_eq!(
            view["Id"].as_str(),
            Some(stored.id.as_str()),
            "case {label:?}: expected the stored Id, got {view:?}"
        );
        assert_eq!(
            view["Arn"].as_str(),
            Some(stored.arn.as_str()),
            "case {label:?}: expected the stored Arn, got {view:?}"
        );
        assert_eq!(
            view["Name"].as_str(),
            Some(stored.name.as_str()),
            "case {label:?}: expected the stored Name, got {view:?}"
        );
        assert_eq!(
            view["Version"].as_i64(),
            Some(i64::from(stored.version)),
            "case {label:?}: expected the stored Version, got {view:?}"
        );

        let content = &view["Content"];
        assert_eq!(
            content["InputSchema"].as_str(),
            stored.input_schema.as_deref(),
            "case {label:?}: expected the stored InputSchema, got {content:?}"
        );
        assert_eq!(
            content["Template"].as_str(),
            stored.template.as_deref(),
            "case {label:?}: expected the stored Template, got {content:?}"
        );
        let reported: Vec<&str> = content["Actions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected an Actions list, got {content:?}"))
            .iter()
            .map(|action| action.as_str().unwrap_or_default())
            .collect();
        let expected: Vec<&str> = stored.actions.iter().map(String::as_str).collect();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the stored Actions, got {content:?}"
        );
    }
}

#[tokio::test]
async fn describing_a_view_leaves_what_a_later_caller_sees_unchanged() {
    let harness = Harness::new();
    let (connection_token, contact_id) = harness.connect("participant-token-reread").await;
    let case = cases()
        .into_iter()
        .next()
        .expect("at least one view case");
    harness.show_view(case.token, &contact_id, case.view);

    let (first_status, first) = harness.describe_view(case.token, &connection_token).await;
    assert_eq!(first_status, 200, "case first read: got {first:?}");
    let (second_status, second) = harness.describe_view(case.token, &connection_token).await;
    assert_eq!(second_status, 200, "case second read: got {second:?}");
    assert_eq!(
        first, second,
        "case repeated read: expected {first:?}, got {second:?}"
    );

    // The read consumed nothing: the view, the session and the connection token are all
    // still there for the next caller.
    let store = harness.store();
    let state = store.lock();
    assert!(
        state.shown_views.contains_key(case.token),
        "case repeated read: the shown view must still be stored, got {:?}",
        state.shown_views.keys().collect::<Vec<_>>()
    );
    assert!(
        state.connections.contains_key(&connection_token),
        "case repeated read: the connection token must still be bound, got {:?}",
        state.connections.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_token_that_names_no_view_of_the_calling_participant_is_access_denied() {
    let harness = Harness::new();
    let (connection_token, contact_id) = harness.connect("participant-token-denied").await;
    let (other_token, other_contact) = harness.connect("participant-token-other-chat").await;
    let case = cases()
        .into_iter()
        .next()
        .expect("at least one view case");
    let shown = case.token;
    harness.show_view(shown, &contact_id, case.view);
    // A view shown in a DIFFERENT chat, reachable only by that chat's participant.
    let elsewhere = cases()
        .into_iter()
        .nth(1)
        .expect("a second view case");
    harness.show_view(elsewhere.token, &other_contact, elsewhere.view);

    // A connection token that no longer authenticates anybody: the participant left.
    let gone = {
        let (token, _) = harness.connect("participant-token-left").await;
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{HOST}/participant/disconnect"))
            .header("host", HOST)
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .header("x-amz-bearer", token.clone())
            .body(Bytes::from_static(b"{}"))
            .expect("build the request");
        let response = harness.router.dispatch(request).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "DisconnectParticipant must succeed before the refusal cases"
        );
        token
    };

    // Rows differ in WHICH token fails to reach a view of the caller's own chat.
    let rows: &[(&str, &str, &str)] = &[
        ("an unknown view token", "no-such-view-token", &connection_token),
        ("a single-character view token", "x", &connection_token),
        (
            "a view token of another participant's chat",
            elsewhere.token,
            &connection_token,
        ),
        ("a shown view, read with an unknown connection token", shown, "bogus-connection-token"),
        (
            "a shown view, read with a token shaped like one we mint",
            shown,
            "00000000-0000-4000-8000-000000000000",
        ),
        (
            "a shown view, read with a disconnected participant's token",
            shown,
            &gone,
        ),
        (
            "another chat's view, read with that chat's own token but no view of it",
            "no-such-view-token",
            &other_token,
        ),
    ];
    for (label, view_token, connection) in rows {
        let (status, body) = harness.describe_view(view_token, connection).await;
        assert_eq!(status, 403, "case {label:?}: got {body:?}");
        assert_eq!(
            error_type(&body),
            "AccessDeniedException",
            "case {label:?}: got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some("Access denied"),
            "case {label:?}: got {body:?}"
        );
    }

    // The refusals told the caller nothing and took nothing away: its own view still
    // reads back.
    let (status, body) = harness.describe_view(shown, &connection_token).await;
    assert_eq!(status, 200, "case the caller's own view: got {body:?}");
}
