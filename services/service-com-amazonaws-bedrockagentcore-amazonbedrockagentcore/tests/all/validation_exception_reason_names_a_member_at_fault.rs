// SPDX-License-Identifier: CC0-1.0

//! What this service's modelled 400 `ValidationException` carries BESIDE its message.
//!
//! THE RULE: `reason` classifies which request MEMBER failed its constraints
//! (`ValidationExceptionReason` = CannotParse | FieldValidationFailed |
//! IdempotentParameterMismatchException | EventInOtherSession | ResourceConflict, source:
//! `smithy_query(query="shape", shape="ValidationExceptionReason")`) and `fieldList` names
//! those members, so a refusal that NO request member is at fault for — the resource the
//! request addresses is in a state that no longer permits the call — carries `message`
//! alone, while a refusal a member IS at fault for carries the classification with it. The
//! status (400), the error code (`ValidationException`) and the message are the same either
//! way.
//!
//! The rows span what the rule generalises over: the plane (the browser sessions plane and
//! the code interpreter sessions plane), how the session stopped being one the call can be
//! served in (a stop, and a time-to-live that ran out), which action or tool the call asked
//! for, and — for the other half of the rule — which member of which tool is at fault.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so what is asserted is the body a client parses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcorecontrol;

use super::harness::{age_browser_session, label};

/// The browser tool the service itself provides, held by every account without anything
/// creating it.
const SYSTEM_BROWSER: &str = "aws.browser.v1";

/// The code interpreter the service itself provides, held on the same terms.
const SYSTEM_INTERPRETER: &str = "aws.codeinterpreter.v1";

/// The header `InvokeBrowser` binds `sessionId` to (source: smithy_query
/// query="operation" operation="InvokeBrowser").
const BROWSER_SESSION_HEADER: &str = "x-amzn-browser-session-id";

/// The header `InvokeCodeInterpreter` binds `sessionId` to (source: smithy_query
/// query="operation" operation="InvokeCodeInterpreter").
const INTERPRETER_SESSION_HEADER: &str = "x-amzn-code-interpreter-session-id";

/// The modelled error code both halves of the rule answer with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The time-to-live every session here is started with, so a case that needs the far side
/// of it ages the stored session instead of waiting (`BrowserSessionTimeout` and
/// `CodeInterpreterSessionTimeout` are both range 1..28800).
const SESSION_TIMEOUT_SECONDS: u64 = 60;

/// One HTTP exchange: the status, the response headers and the body as received.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the body's
    /// `__type`.
    fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, with request authentication off so a test signs nothing, plus the shared
/// engine so a case can age a stored session's clock without sleeping. The AgentCore
/// CONTROL plane is mounted beside this service because it owns the browsers a session
/// runs on.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(agentcorecontrol::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Call {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(value.to_string())
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .dispatch(request.body(payload).expect("build the request"))
        .await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Start one browser session on the system browser and return its `sessionId`.
async fn start_browser_session(router: &MultiServiceRouter) -> String {
    let started = send(
        router,
        "PUT",
        &format!("/browsers/{}/sessions/start", label(SYSTEM_BROWSER)),
        &[],
        Some(json!({"sessionTimeoutSeconds": SESSION_TIMEOUT_SECONDS})),
    )
    .await;
    assert_eq!(
        started.status,
        200,
        "the fixture browser session must start, got {} with {:?}",
        started.status,
        started.json()
    );
    started
        .json()
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `StopBrowserSession`: `PUT /browsers/{browserIdentifier}/sessions/stop?sessionId=…`.
async fn stop_browser_session(router: &MultiServiceRouter, session_id: &str) {
    let stopped = send(
        router,
        "PUT",
        &format!(
            "/browsers/{}/sessions/stop?sessionId={}",
            label(SYSTEM_BROWSER),
            label(session_id)
        ),
        &[],
        None,
    )
    .await;
    assert_eq!(
        stopped.status,
        200,
        "the fixture stop must be served, got {} with {:?}",
        stopped.status,
        stopped.json()
    );
}

/// `InvokeBrowser`: `POST /browsers/{browserIdentifier}/sessions/invoke`, with the session
/// on its header and exactly one `BrowserAction` member in the body.
async fn invoke_browser(router: &MultiServiceRouter, session_id: &str, action: Value) -> Call {
    send(
        router,
        "POST",
        &format!("/browsers/{}/sessions/invoke", label(SYSTEM_BROWSER)),
        &[(BROWSER_SESSION_HEADER, session_id)],
        Some(json!({"action": action})),
    )
    .await
}

/// `GetBrowserSession`, so a case can show the refusal left the session as it found it.
async fn browser_session_status(router: &MultiServiceRouter, session_id: &str) -> String {
    let read = send(
        router,
        "GET",
        &format!(
            "/browsers/{}/sessions/get?sessionId={}",
            label(SYSTEM_BROWSER),
            label(session_id)
        ),
        &[],
        None,
    )
    .await;
    read.json()
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Start one code interpreter session on the system interpreter and return its `sessionId`.
async fn start_interpreter_session(router: &MultiServiceRouter) -> String {
    let started = send(
        router,
        "PUT",
        &format!(
            "/code-interpreters/{}/sessions/start",
            label(SYSTEM_INTERPRETER)
        ),
        &[],
        Some(json!({"sessionTimeoutSeconds": SESSION_TIMEOUT_SECONDS})),
    )
    .await;
    assert_eq!(
        started.status,
        200,
        "the fixture code interpreter session must start, got {} with {:?}",
        started.status,
        started.json()
    );
    started
        .json()
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `StopCodeInterpreterSession`.
async fn stop_interpreter_session(router: &MultiServiceRouter, session_id: &str) {
    let stopped = send(
        router,
        "PUT",
        &format!(
            "/code-interpreters/{}/sessions/stop?sessionId={}",
            label(SYSTEM_INTERPRETER),
            label(session_id)
        ),
        &[],
        Some(json!({})),
    )
    .await;
    assert_eq!(
        stopped.status,
        200,
        "the fixture stop must be served, got {} with {:?}",
        stopped.status,
        stopped.json()
    );
}

/// `InvokeCodeInterpreter`: `POST /code-interpreters/{id}/tools/invoke`.
async fn invoke_tool(
    router: &MultiServiceRouter,
    session_id: &str,
    tool: &str,
    arguments: Value,
) -> Call {
    let mut body = json!({"name": tool});
    if !arguments.is_null() {
        body["arguments"] = arguments;
    }
    send(
        router,
        "POST",
        &format!(
            "/code-interpreters/{}/tools/invoke",
            label(SYSTEM_INTERPRETER)
        ),
        &[(INTERPRETER_SESSION_HEADER, session_id)],
        Some(body),
    )
    .await
}

/// `GetCodeInterpreterSession`, for the same reason as its browser counterpart.
async fn interpreter_session_status(router: &MultiServiceRouter, session_id: &str) -> String {
    let read = send(
        router,
        "GET",
        &format!(
            "/code-interpreters/{}/sessions/get?sessionId={}",
            label(SYSTEM_INTERPRETER),
            label(session_id)
        ),
        &[],
        None,
    )
    .await;
    read.json()
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Move a stored code interpreter session's creation instant `by` further into the past, so
/// a case reaches the far side of a time-to-live without sleeping for it. This is the test
/// reaching into the store the handlers share, not an emulator behaviour.
fn age_interpreter_session(engine: &engine::InMemoryEngine, session_id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let session = store
        .code_interpreters
        .get_mut(SYSTEM_INTERPRETER)
        .and_then(|held| held.sessions.get_mut(session_id))
        .expect("the session that was just started");
    session.created_at -= by;
}

/// Which plane a row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    Browser,
    CodeInterpreter,
}

/// How a row's session stopped being one the call can be served in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    ByStop,
    ByTimeToLive,
}

/// The members a modelled `ValidationException` body carries beyond the JSON error
/// envelope's own `__type` discriminator, each paired with its value.
fn carried_members(body: &Value) -> Vec<(String, Value)> {
    body.as_object()
        .map(|members| {
            members
                .iter()
                .filter(|(name, _)| name.as_str() != "__type")
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// A refusal NO request member is at fault for carries `message` alone: the call addresses
/// a session that has stopped, every member of it is well formed, and the body therefore
/// carries no `reason` to classify a member by and no `fieldList` to name one in.
#[tokio::test]
async fn a_refusal_over_a_sessions_state_carries_the_message_alone() {
    let (router, engine) = emulator();

    // (case, plane, how the session ended, the action or tool the call asked for)
    let rows: Vec<(&str, Plane, Ended, &str, Value)> = vec![
        (
            "browser-stopped-screenshot",
            Plane::Browser,
            Ended::ByStop,
            "screenshot",
            json!({"screenshot": {}}),
        ),
        (
            "browser-stopped-mouse-click",
            Plane::Browser,
            Ended::ByStop,
            "mouseClick",
            json!({"mouseClick": {"x": 10, "y": 20}}),
        ),
        (
            "browser-past-its-time-to-live-key-type",
            Plane::Browser,
            Ended::ByTimeToLive,
            "keyType",
            json!({"keyType": {"text": "hello"}}),
        ),
        (
            "browser-past-its-time-to-live-mouse-scroll",
            Plane::Browser,
            Ended::ByTimeToLive,
            "mouseScroll",
            json!({"mouseScroll": {"x": 1, "y": 1, "deltaY": 120}}),
        ),
        (
            "interpreter-stopped-list-files",
            Plane::CodeInterpreter,
            Ended::ByStop,
            "listFiles",
            json!({"path": ""}),
        ),
        (
            "interpreter-stopped-execute-code",
            Plane::CodeInterpreter,
            Ended::ByStop,
            "executeCode",
            json!({"code": "1 + 1", "language": "javascript"}),
        ),
        (
            "interpreter-past-its-time-to-live-read-files",
            Plane::CodeInterpreter,
            Ended::ByTimeToLive,
            "readFiles",
            json!({"paths": ["notes.txt"]}),
        ),
        (
            "interpreter-past-its-time-to-live-write-files",
            Plane::CodeInterpreter,
            Ended::ByTimeToLive,
            "writeFiles",
            json!({"content": [{"path": "notes.txt", "text": "hi"}]}),
        ),
    ];

    let past_the_timeout = Duration::from_secs(SESSION_TIMEOUT_SECONDS + 1);

    for (case, plane, ended, tool, arguments) in rows {
        let (refused, session_id, expected_message, expected_status) = match plane {
            Plane::Browser => {
                let session_id = start_browser_session(&router).await;
                match ended {
                    Ended::ByStop => stop_browser_session(&router, &session_id).await,
                    Ended::ByTimeToLive => age_browser_session(
                        &engine,
                        SYSTEM_BROWSER,
                        &session_id,
                        past_the_timeout,
                    ),
                }
                let refused = invoke_browser(&router, &session_id, arguments).await;
                let expected = format!(
                    "Browser session {session_id} is not active. Start a new session or \
                     provide a valid session ID"
                );
                (refused, session_id, expected, "TERMINATED".to_string())
            }
            Plane::CodeInterpreter => {
                let session_id = start_interpreter_session(&router).await;
                match ended {
                    Ended::ByStop => stop_interpreter_session(&router, &session_id).await,
                    Ended::ByTimeToLive => {
                        age_interpreter_session(&engine, &session_id, past_the_timeout)
                    }
                }
                let refused = invoke_tool(&router, &session_id, tool, arguments).await;
                let expected = format!("Code interpreter session {session_id} is not active");
                (refused, session_id, expected, "TERMINATED".to_string())
            }
        };

        let body = refused.json();
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {body:?}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?} with {body:?}",
            refused.error_code()
        );
        let carried = carried_members(&body);
        assert_eq!(
            carried,
            vec![("message".to_string(), json!(expected_message))],
            "case {case:?}: expected the message alone, got {carried:?}"
        );

        // The refusal left the session exactly as it found it.
        let status = match plane {
            Plane::Browser => browser_session_status(&router, &session_id).await,
            Plane::CodeInterpreter => interpreter_session_status(&router, &session_id).await,
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected the session to read back {expected_status}, got {status:?}"
        );
    }
}

/// The other half of the rule: a refusal a request MEMBER is at fault for still classifies
/// it, so the same operation on a LIVE session answers the same 400 code with `reason`
/// carrying the classification.
#[tokio::test]
async fn a_refusal_a_member_is_at_fault_for_still_classifies_it() {
    let (router, _engine) = emulator();

    // Rows differ along which tool was invoked and which of its arguments is at fault:
    // one the tool cannot work without, and one it cannot work with as given.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("execute-code-with-no-code", "executeCode", json!({"language": "javascript"})),
        ("execute-code-with-no-language", "executeCode", json!({"code": "1"})),
        ("execute-code-with-no-arguments-at-all", "executeCode", Value::Null),
        ("read-files-with-no-paths", "readFiles", json!({})),
        ("remove-files-with-no-paths", "removeFiles", json!({})),
        ("write-files-with-an-empty-content-list", "writeFiles", json!({"content": []})),
    ];

    for (case, tool, arguments) in rows {
        // A live session, so the only thing at fault is the argument the row names.
        let session_id = start_interpreter_session(&router).await;
        let refused = invoke_tool(&router, &session_id, tool, arguments).await;
        let body = refused.json();

        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {body:?}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?} with {body:?}",
            refused.error_code()
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("FieldValidationFailed"),
            "case {case:?}: expected the member-level classification, got {body:?}"
        );
        let message = body.get("message").and_then(Value::as_str).unwrap_or("");
        assert!(
            message.contains(tool),
            "case {case:?}: expected the message to name the tool {tool:?}, got {message:?}"
        );
    }
}
