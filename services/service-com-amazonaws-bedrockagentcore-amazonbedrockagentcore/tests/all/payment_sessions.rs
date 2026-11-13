// SPDX-License-Identifier: CC0-1.0

//! The PAYMENTS plane's session lifecycle: `CreatePaymentSession`, `GetPaymentSession`,
//! `ListPaymentSessions` and `DeletePaymentSession`.
//!
//! RULE: a payment session is state this service holds under the payment manager that owns
//! it — the create mints its `paymentSessionId` and persists the user, the window and the
//! spending limits the request asked for, every read reports that record back, the delete
//! removes it, and a request naming a manager the account does not hold is denied before
//! anything is read or written.
//!
//! Each table's rows differ along what that rule generalises over: which optional members
//! the create carries (`limits`, `userId`, `agentName`, `clientToken`), the window's value
//! at both published bounds of `expiryTimeInMinutes` and past each, the shape of a limit's
//! decimal amount, which manager a session belongs to, which operation of the plane makes
//! the call, and whether the session the call addresses exists at all.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcorecontrol;

/// A manager of the modelled form that no create ever made, in another partition and region
/// so the case does not depend on where the held ones live.
const GHOST_MANAGER_ARN: &str =
    "arn:aws-us-gov:bedrock-agentcore:us-gov-west-1:000000000000:payment-manager/ghost-9z8y7x6w5v";

/// A `paymentSessionId` of the modelled form that addresses no session here
/// (`PaymentSessionId` is `^payment-session-[0-9a-zA-Z-]{15}$`, length exactly 31).
const GHOST_SESSION_ID: &str = "payment-session-0000000000ghost";

/// The two published bounds of `expiryTimeInMinutes` (`@range(min 15, max 480)`, source:
/// smithy_query query="operation" operation="CreatePaymentSession").
const MIN_EXPIRY_MINUTES: i64 = 15;
const MAX_EXPIRY_MINUTES: i64 = 480;

/// The header each optional `@httpHeader` member of the plane is carried in.
const USER_ID_HEADER: &str = "X-Amzn-Bedrock-AgentCore-Payments-User-Id";
const AGENT_NAME_HEADER: &str = "X-Amzn-Bedrock-AgentCore-Payments-Agent-Name";

/// A `clientToken` of the modelled length (33..256) and pattern.
const CLIENT_TOKEN: &str = "paymentsessiontoken00000000000001";
/// A second `clientToken`, so a case can tell a replay from a fresh call.
const OTHER_CLIENT_TOKEN: &str = "paymentsessiontoken00000000000002";

/// The role a payment manager runs under, in the form `RoleArn` publishes as its pattern
/// (source: smithy_query query="operation" operation="CreatePaymentManager" on the control
/// plane's model).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/AgentCorePayments";

/// The two payment managers a case opens sessions under, and the shared engine so a case can
/// age a stored session's clock without sleeping.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    /// The manager most cases work under.
    manager: String,
    /// A second manager, so a case can show a session belongs to ONE of them.
    other_manager: String,
}

/// The emulator, with request authentication off so a test signs nothing.
///
/// The AgentCore CONTROL plane is mounted beside this service because it OWNS payment
/// managers: a case that needs one creates a real manager through `CreatePaymentManager`
/// rather than naming an ARN nothing holds.
async fn emulator() -> Fixture {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(agentcorecontrol::EmulatedService::new(Arc::clone(&engine)));
    let manager = create_payment_manager(&router, "payments").await;
    let other_manager = create_payment_manager(&router, "otherpayments").await;
    Fixture {
        router,
        engine,
        manager,
        other_manager,
    }
}

/// Create one payment manager through the control plane and return the `paymentManagerArn`
/// it minted. `CreatePaymentManager` is `POST /payments/managers`, answering 202 (source:
/// smithy_query query="operation" operation="CreatePaymentManager" on the control plane's
/// model).
async fn create_payment_manager(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "/payments/managers",
        json!({"name": name, "authorizerType": "AWS_IAM", "roleArn": ROLE_ARN}),
        None,
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "the fixture payment manager must be created, got {status} with {created:?}"
    );
    created["paymentManagerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePaymentManager reported no ARN: {created:?}"))
        .to_string()
}

/// Send one request of the payments plane, with the optional user and agent headers when a
/// case carries them.
async fn send(
    router: &MultiServiceRouter,
    path: &str,
    body: Value,
    user_id: Option<&str>,
    agent_name: Option<&str>,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json");
    if let Some(user_id) = user_id {
        request = request.header(USER_ID_HEADER, user_id);
    }
    if let Some(agent_name) = agent_name {
        request = request.header(AGENT_NAME_HEADER, agent_name);
    }
    let request = request
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `CreatePaymentSession`: `POST /payments/createPaymentSession`.
async fn create_session(
    router: &MultiServiceRouter,
    body: Value,
    user_id: Option<&str>,
    agent_name: Option<&str>,
) -> (u16, Value) {
    send(
        router,
        "/payments/createPaymentSession",
        body,
        user_id,
        agent_name,
    )
    .await
}

/// `GetPaymentSession`: `POST /payments/getPaymentSession`.
async fn get_session(
    router: &MultiServiceRouter,
    manager_arn: &str,
    session_id: &str,
) -> (u16, Value) {
    send(
        router,
        "/payments/getPaymentSession",
        json!({"paymentManagerArn": manager_arn, "paymentSessionId": session_id}),
        None,
        None,
    )
    .await
}

/// `ListPaymentSessions`: `POST /payments/listPaymentSessions`.
async fn list_sessions(
    router: &MultiServiceRouter,
    body: Value,
    user_id: Option<&str>,
) -> (u16, Value) {
    send(
        router,
        "/payments/listPaymentSessions",
        body,
        user_id,
        None,
    )
    .await
}

/// `DeletePaymentSession`: `POST /payments/deletePaymentSession`, naming no user.
async fn delete_session(
    router: &MultiServiceRouter,
    manager_arn: &str,
    session_id: &str,
) -> (u16, Value) {
    delete_session_as(router, manager_arn, session_id, None).await
}

/// The same delete, carrying the `userId` of the request making it.
async fn delete_session_as(
    router: &MultiServiceRouter,
    manager_arn: &str,
    session_id: &str,
    user_id: Option<&str>,
) -> (u16, Value) {
    send(
        router,
        "/payments/deletePaymentSession",
        json!({"paymentManagerArn": manager_arn, "paymentSessionId": session_id}),
        user_id,
        None,
    )
    .await
}

/// The wire error code of a refusal (`__type` on `restJson1`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a refusal.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `id` is a `paymentSessionId` of the modelled form: the fixed prefix, then fifteen
/// characters of `[0-9a-zA-Z-]`, 31 in all.
fn is_modelled_session_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("payment-session-") else {
        return false;
    };
    suffix.len() == 15
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The spending limits of a session, as `SessionLimits` models them: one `maxSpendAmount`
/// carrying a decimal value and a currency (source: smithy_query query="shape"
/// shape="SessionLimits,Amount,Currency" — `Currency` admits USD).
fn limits(value: &str) -> Value {
    json!({"maxSpendAmount": {"value": value, "currency": "USD"}})
}

/// Move a stored session's creation instant `by` further into the past, so a case can reach
/// the far side of the window it was opened with without sleeping for it. This is the test
/// reaching into the store the handlers share, not an emulator behaviour.
fn age_session(engine: &engine::InMemoryEngine, session_id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let session = store
        .payment_sessions
        .get_mut(session_id)
        .expect("the payment session that was just created");
    session.created_at -= by;
    session.updated_at -= by;
}

/// How many payment sessions this emulator holds — the check that a refused or replayed
/// create wrote nothing new.
fn session_count(engine: &engine::InMemoryEngine) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store.payment_sessions.len()
}

/// One row of the create table: the case name, the manager the session is opened under, the
/// request body, and the two optional header values.
type CreateCase<'a> = (&'a str, &'a str, Value, Option<&'a str>, Option<&'a str>);

/// One row of the listing table: the case name, the manager listed, `maxResults`, the
/// `userId` the request carries, the ids expected on the FIRST page and whether a
/// `nextToken` must come with it.
type ListCase<'a> = (
    &'a str,
    &'a str,
    Option<i64>,
    Option<&'a str>,
    Vec<String>,
    bool,
);

/// A created payment session is reported from what was stored, and `GetPaymentSession`
/// reports the same record back — for every combination of the optional members
/// `CreatePaymentSessionRequest` models, both published bounds of the window, and several
/// shapes of the limit's decimal amount.
#[tokio::test]
async fn a_created_payment_session_round_trips_through_get_payment_session() {
    let Fixture {
        router,
        manager,
        other_manager,
        ..
    } = emulator().await;

    // `UserId` is length ..120, so 120 characters is its published bound.
    let longest_user_id = "u".repeat(120);
    // (case, manager, request body, userId header, agentName header)
    let cases: Vec<CreateCase<'_>> = vec![
        (
            "required-members-only-at-the-shortest-window",
            &manager,
            json!({
                "paymentManagerArn": manager,
                "expiryTimeInMinutes": MIN_EXPIRY_MINUTES,
            }),
            None,
            None,
        ),
        (
            "limits-and-both-headers-at-the-longest-window",
            &manager,
            json!({
                "paymentManagerArn": manager,
                "expiryTimeInMinutes": MAX_EXPIRY_MINUTES,
                "limits": limits("250.00"),
            }),
            Some("user-alice"),
            Some("checkout-agent"),
        ),
        (
            "a-long-decimal-amount-keeps-every-digit",
            &manager,
            json!({
                "paymentManagerArn": manager,
                "expiryTimeInMinutes": 60,
                "limits": limits("12345678901234567890.123456789"),
            }),
            Some("user-bob"),
            None,
        ),
        (
            "a-zero-limit-and-the-longest-user-id",
            &manager,
            json!({
                "paymentManagerArn": manager,
                "expiryTimeInMinutes": 120,
                "limits": limits("0"),
            }),
            // `UserId` is length ..120, so 120 characters is its published bound.
            Some(&longest_user_id),
            Some("agent"),
        ),
        (
            "another-manager-owns-its-own-sessions",
            &other_manager,
            json!({
                "paymentManagerArn": other_manager,
                "expiryTimeInMinutes": 45,
                "limits": limits("1.5"),
            }),
            Some("user-carol"),
            None,
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, manager, request, user_id, agent_name) in cases {
        let (status, created) = create_session(&router, request.clone(), user_id, agent_name).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreatePaymentSession, got {status} with \
             {created:?}"
        );
        let session = created["paymentSession"].clone();

        let session_id = session["paymentSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_session_id(&session_id),
            "case {case:?}: expected a paymentSessionId matching \
             ^payment-session-[0-9a-zA-Z-]{{15}}$, got {session_id:?}"
        );
        assert!(
            minted.insert(session_id.clone()),
            "case {case:?}: expected a fresh paymentSessionId, got the already minted \
             {session_id:?}"
        );

        assert_eq!(
            session["paymentManagerArn"].as_str().unwrap_or_default(),
            manager,
            "case {case:?}: expected the manager the request named, got {:?}",
            session["paymentManagerArn"]
        );
        // `PaymentSession$userId` is required while the request's own member is optional:
        // a session opened without a user reports none.
        let expected_user = user_id.unwrap_or_default();
        assert_eq!(
            session["userId"].as_str().unwrap_or_default(),
            expected_user,
            "case {case:?}: expected userId {expected_user:?}, got {:?}",
            session["userId"]
        );
        let expected_window = request["expiryTimeInMinutes"].as_i64().unwrap_or_default();
        assert_eq!(
            session["expiryTimeInMinutes"].as_i64().unwrap_or_default(),
            expected_window,
            "case {case:?}: expected expiryTimeInMinutes {expected_window}, got {:?}",
            session["expiryTimeInMinutes"]
        );
        assert_eq!(
            session["limits"],
            request.get("limits").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the limits the request set, got {:?}",
            session["limits"]
        );
        // Nothing has been charged against a session this call just opened, so the whole
        // maximum is still available — carried as the decimal string the limit was set
        // with, and absent altogether for a session with no limits.
        assert_eq!(
            session["availableLimits"]["availableSpendAmount"],
            request
                .get("limits")
                .map(|set| set["maxSpendAmount"].clone())
                .unwrap_or(Value::Null),
            "case {case:?}: expected the whole maximum to be available, got {:?}",
            session["availableLimits"]
        );
        assert_eq!(
            session["createdAt"], session["updatedAt"],
            "case {case:?}: expected a session nothing has changed to report one instant, got \
             createdAt {:?} and updatedAt {:?}",
            session["createdAt"], session["updatedAt"]
        );
        assert!(
            session["createdAt"].is_string(),
            "case {case:?}: expected a date-time createdAt, got {:?}",
            session["createdAt"]
        );

        // The read reports the SAME record, and reports it twice over: a read changes
        // nothing a later caller sees.
        for attempt in 1..=2 {
            let (status, read) = get_session(&router, manager, &session_id).await;
            assert_eq!(
                status, 200,
                "case {case:?} read {attempt}: expected HTTP 200 from GetPaymentSession, got \
                 {status} with {read:?}"
            );
            assert_eq!(
                read["paymentSession"], session,
                "case {case:?} read {attempt}: expected the created session back, got {:?}",
                read["paymentSession"]
            );
        }
    }
}

/// A window outside the range the model publishes is refused, and nothing is stored for it.
#[tokio::test]
async fn a_window_outside_the_modelled_range_is_refused() {
    let Fixture {
        router,
        engine,
        manager,
        ..
    } = emulator().await;

    // (case, expiryTimeInMinutes, whether the model admits it)
    let rows: [(&str, i64, bool); 6] = [
        ("one below the published minimum", MIN_EXPIRY_MINUTES - 1, false),
        ("the published minimum", MIN_EXPIRY_MINUTES, true),
        ("inside the range", 300, true),
        ("the published maximum", MAX_EXPIRY_MINUTES, true),
        ("one above the published maximum", MAX_EXPIRY_MINUTES + 1, false),
        ("far above the published maximum", 100_000, false),
    ];

    let mut stored = session_count(&engine);
    for (case, minutes, admitted) in rows {
        let (status, body) = create_session(
            &router,
            json!({"paymentManagerArn": manager, "expiryTimeInMinutes": minutes}),
            Some("user-alice"),
            None,
        )
        .await;
        let expected_status = if admitted { 201 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {case:?} ({minutes}): expected HTTP {expected_status}, got {status} with \
             {body:?}"
        );
        if admitted {
            stored += 1;
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} ({minutes}): expected a ValidationException, got {body:?}"
            );
        }
        let count = session_count(&engine);
        assert_eq!(
            count, stored,
            "case {case:?} ({minutes}): expected {stored} stored session(s), got {count}"
        );
    }
}

/// A replayed `clientToken` answers with the session it already opened instead of opening a
/// second one; a fresh token opens a new session, and the same token under another manager
/// opens that manager's own.
#[tokio::test]
async fn a_replayed_client_token_opens_no_second_session() {
    let Fixture {
        router,
        engine,
        manager,
        other_manager,
    } = emulator().await;

    let request = |manager: &str, token: Option<&str>| {
        let mut body = json!({"paymentManagerArn": manager, "expiryTimeInMinutes": 30});
        if let Some(token) = token {
            body["clientToken"] = json!(token);
        }
        body
    };

    let open = |body: Value| async { create_session(&router, body, Some("user-alice"), None).await };

    let (_, first) = open(request(&manager, Some(CLIENT_TOKEN))).await;
    let first_id = first["paymentSession"]["paymentSessionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, manager, token, whether the answer must be the session the first call opened)
    let rows: [(&str, &str, Option<&str>, bool); 4] = [
        ("the same token again", &manager, Some(CLIENT_TOKEN), true),
        (
            "the same token a third time",
            &manager,
            Some(CLIENT_TOKEN),
            true,
        ),
        ("another token", &manager, Some(OTHER_CLIENT_TOKEN), false),
        ("no token at all", &manager, None, false),
    ];
    let mut expected_count = 1;
    for (case, manager, token, replayed) in rows {
        let (status, body) = open(request(manager, token)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201, got {status} with {body:?}"
        );
        let id = body["paymentSession"]["paymentSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if replayed {
            assert_eq!(
                id, first_id,
                "case {case:?}: expected the session the token already opened ({first_id}), got \
                 {id}"
            );
        } else {
            assert_ne!(
                id, first_id,
                "case {case:?}: expected a session of its own, got the replayed {first_id}"
            );
            expected_count += 1;
        }
        let count = session_count(&engine);
        assert_eq!(
            count, expected_count,
            "case {case:?}: expected {expected_count} stored session(s), got {count}"
        );
    }

    // The same token under ANOTHER manager opens that manager's own session: a token is
    // answered only for the manager it was used under.
    let (status, other) = open(request(&other_manager, Some(CLIENT_TOKEN))).await;
    assert_eq!(
        status, 201,
        "case the same token under another manager: expected HTTP 201, got {status} with \
         {other:?}"
    );
    let other_id = other["paymentSession"]["paymentSessionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        other_id, first_id,
        "case the same token under another manager: expected a session of its own, got the \
         replayed {first_id}"
    );
    assert_eq!(
        other["paymentSession"]["paymentManagerArn"]
            .as_str()
            .unwrap_or_default(),
        other_manager,
        "case the same token under another manager: expected the session to belong to it, got \
         {:?}",
        other["paymentSession"]["paymentManagerArn"]
    );
}

/// A listing reports the sessions of the manager it names, newest first, one page at a
/// time, and narrows to one user when the request names one.
#[tokio::test]
async fn payment_sessions_are_listed_newest_first_and_paginate() {
    let Fixture {
        router,
        manager,
        other_manager,
        ..
    } = emulator().await;

    // Opened oldest first, each under a named user, so the listing's order and its user
    // scope are both observable.
    let mut opened: Vec<(String, &str)> = Vec::new();
    for (index, user) in ["user-alice", "user-bob", "user-alice"].into_iter().enumerate() {
        let (status, created) = create_session(
            &router,
            json!({
                "paymentManagerArn": manager,
                "expiryTimeInMinutes": 15 + index as i64,
            }),
            Some(user),
            None,
        )
        .await;
        assert_eq!(
            status, 201,
            "opening session {index}: expected HTTP 201, got {status} with {created:?}"
        );
        opened.push((
            created["paymentSession"]["paymentSessionId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            user,
        ));
    }
    // A session of the OTHER manager, which this manager's listing must never report.
    let (_, foreign) = create_session(
        &router,
        json!({"paymentManagerArn": other_manager, "expiryTimeInMinutes": 20}),
        Some("user-alice"),
        None,
    )
    .await;
    let foreign_id = foreign["paymentSession"]["paymentSessionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let newest_first: Vec<String> = opened.iter().rev().map(|(id, _)| id.clone()).collect();
    let alice: Vec<String> = opened
        .iter()
        .rev()
        .filter(|(_, user)| *user == "user-alice")
        .map(|(id, _)| id.clone())
        .collect();

    // (case, manager, maxResults, userId header, the ids expected on the FIRST page,
    //  whether a nextToken must come with it)
    let rows: Vec<ListCase<'_>> = vec![
        (
            "every session of the manager",
            &manager,
            None,
            None,
            newest_first.clone(),
            false,
        ),
        (
            "one page of two",
            &manager,
            Some(2),
            None,
            newest_first[..2].to_vec(),
            true,
        ),
        (
            "one page of one",
            &manager,
            Some(1),
            None,
            newest_first[..1].to_vec(),
            true,
        ),
        (
            "narrowed to one user",
            &manager,
            None,
            Some("user-alice"),
            alice.clone(),
            false,
        ),
        (
            "narrowed to a user with no sessions",
            &manager,
            None,
            Some("user-nobody"),
            Vec::new(),
            false,
        ),
        (
            "the other manager's own sessions",
            &other_manager,
            None,
            None,
            vec![foreign_id.clone()],
            false,
        ),
    ];

    for (case, listed_manager, max_results, user_id, expected, expects_token) in rows {
        let mut body = json!({"paymentManagerArn": listed_manager});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        let (status, listed) = list_sessions(&router, body.clone(), user_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListPaymentSessions, got {status} with \
             {listed:?}"
        );
        let page: Vec<String> = listed["paymentSessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["paymentSessionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            page, expected,
            "case {case:?}: expected the page {expected:?}, got {page:?}"
        );
        assert_eq!(
            listed["nextToken"].is_string(),
            expects_token,
            "case {case:?}: expected nextToken present={expects_token}, got {:?}",
            listed["nextToken"]
        );

        // Every page of a paginated listing walks the whole list the request described.
        if expects_token {
            let mut walked = page.clone();
            let mut token = listed["nextToken"].as_str().unwrap_or_default().to_string();
            for _ in 0..expected.len() + 4 {
                let mut next = body.clone();
                next["nextToken"] = json!(token);
                let (status, listed) = list_sessions(&router, next, user_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} paging: expected HTTP 200, got {status} with {listed:?}"
                );
                walked.extend(
                    listed["paymentSessions"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|summary| {
                            summary["paymentSessionId"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string()
                        }),
                );
                match listed["nextToken"].as_str() {
                    Some(next_token) => token = next_token.to_string(),
                    None => break,
                }
            }
            assert_eq!(
                walked, newest_first,
                "case {case:?} paging: expected the whole list {newest_first:?}, got {walked:?}"
            );
        }
    }

    // A summary carries the same identity the read of the one session reports.
    let (_, listed) = list_sessions(
        &router,
        json!({"paymentManagerArn": manager, "maxResults": 1}),
        None,
    )
    .await;
    let summary = listed["paymentSessions"][0].clone();
    let (_, read) = get_session(
        &router,
        &manager,
        summary["paymentSessionId"].as_str().unwrap_or_default(),
    )
    .await;
    let session = read["paymentSession"].clone();
    for member in [
        "paymentSessionId",
        "paymentManagerArn",
        "userId",
        "expiryTimeInMinutes",
        "createdAt",
        "updatedAt",
    ] {
        assert_eq!(
            summary[member], session[member],
            "case the summary of a listed session: expected {member} to match the read, got {:?} \
             and {:?}",
            summary[member], session[member]
        );
    }
}

/// A delete removes the session it addresses and reports the one status the model fixes for
/// a successful hard delete, whatever the session's own window had done; every later call
/// that addresses it is refused.
#[tokio::test]
async fn a_delete_removes_the_session_and_always_reports_deleted() {
    let Fixture {
        router,
        engine,
        manager,
        ..
    } = emulator().await;

    // `DeletePaymentSessionResponse$status` is documented as "the status of the deletion.
    // Always DELETED for successful hard delete", so the rows differ along how far the
    // session's window has run — inside it, one minute short of its end, and well past it —
    // and every one of them expects that same value.
    let rows: [(&str, Duration); 3] = [
        ("a session still inside its window", Duration::ZERO),
        (
            "a session one minute inside its window",
            Duration::from_secs(14 * 60),
        ),
        (
            "a session whose window the clock has run out",
            Duration::from_secs(16 * 60),
        ),
    ];

    for (case, aged_by) in rows {
        let (status, created) = create_session(
            &router,
            json!({
                "paymentManagerArn": manager,
                // The shortest window the model admits, so ageing the record past it is
                // the smallest step a case needs.
                "expiryTimeInMinutes": MIN_EXPIRY_MINUTES,
                "limits": limits("10.00"),
            }),
            Some("user-alice"),
            None,
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201, got {status} with {created:?}"
        );
        let session_id = created["paymentSession"]["paymentSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if aged_by > Duration::ZERO {
            age_session(&engine, &session_id, aged_by);
        }

        let before = session_count(&engine);
        let (status, deleted) = delete_session(&router, &manager, &session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeletePaymentSession, got {status} with \
             {deleted:?}"
        );
        assert_eq!(
            deleted["status"].as_str().unwrap_or_default(),
            "DELETED",
            "case {case:?}: expected the status a successful hard delete always reports, got \
             {:?}",
            deleted["status"]
        );
        let after = session_count(&engine);
        assert_eq!(
            after,
            before - 1,
            "case {case:?}: expected the session removed, got {after} stored of {before}"
        );

        // The session is gone for every caller: the read, the listing and a second delete.
        let (status, read) = get_session(&router, &manager, &session_id).await;
        assert_eq!(
            status, 404,
            "case {case:?} read after the delete: expected HTTP 404, got {status} with {read:?}"
        );
        let (status, again) = delete_session(&router, &manager, &session_id).await;
        assert_eq!(
            status, 404,
            "case {case:?} deleted twice: expected HTTP 404, got {status} with {again:?}"
        );
        let (_, listed) =
            list_sessions(&router, json!({"paymentManagerArn": manager}), None).await;
        let reported: Vec<String> = listed["paymentSessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["paymentSessionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !reported.contains(&session_id),
            "case {case:?}: expected the deleted session not to be listed, got {reported:?}"
        );
    }
}

/// A delete removes a session only for the user it belongs to: `userId` "must match the
/// session's userId", so a request naming another user is refused and the session stays.
#[tokio::test]
async fn a_delete_naming_another_users_session_is_refused() {
    let Fixture {
        router,
        engine,
        manager,
        ..
    } = emulator().await;

    // (case, the user the session is opened for, the user the delete names, whether the
    //  delete may remove it)
    let rows: [(&str, Option<&str>, Option<&str>, bool); 5] = [
        ("the same user", Some("user-alice"), Some("user-alice"), true),
        ("no user on the delete", Some("user-alice"), None, true),
        ("neither names a user", None, None, true),
        ("another user", Some("user-alice"), Some("user-bob"), false),
        ("a user where the session has none", None, Some("user-alice"), false),
    ];

    for (case, owner, deleter, permitted) in rows {
        let (status, created) = create_session(
            &router,
            json!({"paymentManagerArn": manager, "expiryTimeInMinutes": 30}),
            owner,
            None,
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201, got {status} with {created:?}"
        );
        let session_id = created["paymentSession"]["paymentSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let before = session_count(&engine);
        let (status, answer) = delete_session_as(&router, &manager, &session_id, deleter).await;
        if permitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            assert_eq!(
                session_count(&engine),
                before - 1,
                "case {case:?}: expected the session removed, got {} stored of {before}",
                session_count(&engine)
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            message(&answer),
            "userId must match the session's userId",
            "case {case:?}: expected the sentence stating the rule the member failed, got {:?}",
            message(&answer)
        );
        assert_eq!(
            answer["fieldList"][0]["name"].as_str().unwrap_or_default(),
            "userId",
            "case {case:?}: expected the refusal to name the member that failed, got {:?}",
            answer["fieldList"]
        );
        // The refused delete removed nothing, and the session is still readable.
        assert_eq!(
            session_count(&engine),
            before,
            "case {case:?}: expected the refusal to remove nothing, got {} stored of {before}",
            session_count(&engine)
        );
        let (status, read) = get_session(&router, &manager, &session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the session to be readable still, got {status} with {read:?}"
        );
    }
}

/// A `paymentSessionId` this manager holds no session for is refused with the plane's
/// not-found sentence, naming the session the request addressed — whichever operation
/// addresses it, and whether the id was never minted or belongs to ANOTHER manager's
/// session.
#[tokio::test]
async fn a_session_no_manager_of_the_request_holds_is_not_found() {
    let Fixture {
        router,
        engine,
        manager,
        other_manager,
    } = emulator().await;

    let (_, created) = create_session(
        &router,
        json!({"paymentManagerArn": other_manager, "expiryTimeInMinutes": 30}),
        Some("user-alice"),
        None,
    )
    .await;
    let foreign_id = created["paymentSession"]["paymentSessionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the id addressed under the FIRST manager)
    let ids: [(&str, &str); 2] = [
        ("an id no create ever minted", GHOST_SESSION_ID),
        ("another manager's session", &foreign_id),
    ];
    for (case, session_id) in ids {
        for (operation, answer) in [
            (
                "GetPaymentSession",
                get_session(&router, &manager, session_id).await,
            ),
            (
                "DeletePaymentSession",
                delete_session(&router, &manager, session_id).await,
            ),
        ] {
            let (status, refused) = answer;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected HTTP 404, got {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected a ResourceNotFoundException, got \
                 {refused:?}"
            );
            assert_eq!(
                message(&refused),
                format!("Payment session not found: {session_id}"),
                "case {case:?} on {operation}: expected the plane's not-found sentence naming \
                 the session the request addressed, got {:?}",
                message(&refused)
            );
        }
    }

    // The refused delete removed nothing: the other manager still holds its session.
    let count = session_count(&engine);
    assert_eq!(
        count, 1,
        "expected the refusals to remove nothing, got {count} stored session(s)"
    );
    let (status, still_there) = get_session(&router, &other_manager, &foreign_id).await;
    assert_eq!(
        status, 200,
        "expected the other manager's session to be readable still, got {status} with \
         {still_there:?}"
    );
}

/// Every operation of the plane resolves the payment manager against the plane that OWNS
/// it, and denies a manager the account does not hold before reading or writing anything.
#[tokio::test]
async fn a_payment_manager_the_account_does_not_hold_is_denied_on_every_operation() {
    let Fixture {
        router,
        engine,
        manager,
        ..
    } = emulator().await;

    let (_, created) = create_session(
        &router,
        json!({"paymentManagerArn": manager, "expiryTimeInMinutes": 30}),
        Some("user-alice"),
        None,
    )
    .await;
    let session_id = created["paymentSession"]["paymentSessionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (operation, the answer it gives for a manager nothing created)
    let answers = [
        (
            "CreatePaymentSession",
            create_session(
                &router,
                json!({"paymentManagerArn": GHOST_MANAGER_ARN, "expiryTimeInMinutes": 30}),
                Some("user-alice"),
                None,
            )
            .await,
        ),
        (
            "GetPaymentSession",
            get_session(&router, GHOST_MANAGER_ARN, &session_id).await,
        ),
        (
            "ListPaymentSessions",
            list_sessions(&router, json!({"paymentManagerArn": GHOST_MANAGER_ARN}), None).await,
        ),
        (
            "DeletePaymentSession",
            delete_session(&router, GHOST_MANAGER_ARN, &session_id).await,
        ),
    ];
    for (operation, (status, refused)) in answers {
        assert_eq!(
            status, 403,
            "case {operation} against a manager the account does not hold: expected HTTP 403, \
             got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {operation} against a manager the account does not hold: expected an \
             AccessDeniedException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            "Payment manager not found",
            "case {operation} against a manager the account does not hold: expected the plane's \
             denial sentence, got {:?}",
            message(&refused)
        );
    }

    // None of the four denials wrote or removed anything.
    let count = session_count(&engine);
    assert_eq!(
        count, 1,
        "expected the denials to leave the store alone, got {count} stored session(s)"
    );
    let (status, still_there) = get_session(&router, &manager, &session_id).await;
    assert_eq!(
        status, 200,
        "expected the held manager's session to be readable still, got {status} with \
         {still_there:?}"
    );
}
