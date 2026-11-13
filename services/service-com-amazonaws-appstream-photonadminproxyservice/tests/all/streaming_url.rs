// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's streaming-URL issuance over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a streaming URL is issued only for a fleet of the stack that is STREAMING: while the
//!   stack's fleet is in any other state the call is refused with
//!   `OperationNotPermittedException` (400) carrying AWS's sentence for that class, and
//!   no URL is issued;
//! * the stack and the fleet have to exist and be associated: a stack or fleet this
//!   request cannot reach is refused with `ResourceNotFoundException` (400);
//! * `StartFleet` is what makes a fleet streaming, and it does so on the clock — a fleet
//!   reports `STARTING` straight after the call and `RUNNING` once the span has passed,
//!   at which point the same `CreateStreamingURL` request succeeds;
//! * a successful call answers a URL that leads back to this emulator, valid for exactly
//!   the `Validity` the request asked for (60 seconds by default), together with the
//!   instant it expires;
//! * issuing a URL dedicates ONE streaming session per `(stack, fleet, user)` identity,
//!   which a read of that stack's fleet reports as pending and unconnected until the URL
//!   stops verifying, at which point the clock expires it — and a refused call dedicates
//!   none;
//! * a `Validity` outside the documented range is refused and issues nothing.

use std::time::Duration;

use serde_json::{json, Value};

use crate::refusal_messages::{not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The stack and fleet every case below streams from.
const STACK: &str = "streaming-stack";
const FLEET: &str = "streaming-fleet";

/// The user a streaming URL is issued for (`UserId`, len 2..32, pattern `^[\w+=,.@-]*$`).
const USER: &str = "stream-user1";

/// The states a fleet can be in without streaming, as `FLEET_STATE_VALUES`
/// (`src/ops/types/shared_types.rs`) names them. `STOPPED` is where a created fleet
/// starts; `STARTING` is where `StartFleet` puts it until its span has passed.
const FLEET_STATE_CREATED: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::FLEET_STATE_CREATED;
const FLEET_STATE_STARTING: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::FLEET_STATE_STARTING;
const FLEET_STATE_RUNNING: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::FLEET_STATE_RUNNING;

/// The span the clock owes a starting fleet before it is streaming, read from the crate so
/// the test tracks the constant rather than a copy of its value.
const FLEET_START_SPAN: Duration =
    service_com_amazonaws_appstream_photonadminproxyservice::state::FLEET_START_SPAN;

/// The documented bounds of `Validity` ("Specify a value between 1 and 604800 seconds")
/// and the documented default ("The default is 60 seconds").
const VALIDITY_MIN: i64 =
    service_com_amazonaws_appstream_photonadminproxyservice::state::STREAMING_URL_MIN_VALIDITY_SECONDS;
const VALIDITY_MAX: i64 =
    service_com_amazonaws_appstream_photonadminproxyservice::state::STREAMING_URL_MAX_VALIDITY_SECONDS;
const VALIDITY_DEFAULT: i64 =
    service_com_amazonaws_appstream_photonadminproxyservice::state::STREAMING_URL_DEFAULT_VALIDITY_SECONDS;

/// AWS's sentence for a streaming URL asked for against a fleet that is not streaming, as
/// the crate states it: the fleet name is its only variable part.
fn not_running_sentence(fleet: &str) -> String {
    service_com_amazonaws_appstream_photonadminproxyservice::state::fleet_not_running_message(fleet)
}

/// Create a stack, a fleet and the association between them, which is the setup every
/// case below starts from.
///
/// Shared with the suite that follows the URL this setup makes issuable
/// (`tests/all/streaming_session_resolution.rs`).
pub(crate) async fn stack_with_fleet(emulator: &Emulator, case: &str, stack: &str, fleet: &str) {
    stack_with_fleet_lasting(emulator, case, stack, fleet, None).await;
}

/// The same setup, for a fleet that publishes a maximum user duration.
async fn stack_with_fleet_lasting(
    emulator: &Emulator,
    case: &str,
    stack: &str,
    fleet: &str,
    max_user_duration: Option<i64>,
) {
    emulator.ok(case, "CreateStack", json!({ "Name": stack })).await;
    let mut create = json!({
        "Name": fleet,
        "InstanceType": "stream.standard.small",
        "FleetType": "ELASTIC",
        "Platform": "WINDOWS_SERVER_2019",
        "MaxConcurrentSessions": 5,
        "VpcConfig": { "SubnetIds": ["subnet-00000000", "subnet-11111111"] },
    });
    if let Some(max_user_duration) = max_user_duration {
        create["MaxUserDurationInSeconds"] = json!(max_user_duration);
    }
    emulator.ok(case, "CreateFleet", create).await;
    emulator
        .ok(
            case,
            "AssociateFleet",
            json!({ "FleetName": fleet, "StackName": stack }),
        )
        .await;
}

/// The `State` a `DescribeFleets` reports for one fleet.
async fn fleet_state(emulator: &Emulator, case: &str, fleet: &str) -> String {
    let payload = emulator
        .ok(case, "DescribeFleets", json!({ "Names": [fleet] }))
        .await;
    payload
        .pointer("/Fleets/0/State")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the fleet's State, got {payload}"))
        .to_string()
}

/// Start the fleet and wait for the state the clock owes it, asserting that the transient
/// state is observable first.
///
/// Shared with `tests/all/streaming_session_resolution.rs`, so both suites make a fleet
/// streaming the same way.
pub(crate) async fn start_and_await_running(emulator: &Emulator, case: &str, fleet: &str) {
    emulator.ok(case, "StartFleet", json!({ "Name": fleet })).await;
    let starting = fleet_state(emulator, case, fleet).await;
    assert_eq!(
        starting, FLEET_STATE_STARTING,
        "case {case:?}: a caller reading straight after the start must still see the \
         transient state, got {starting:?}"
    );

    // The transition is due on the clock; nothing here hurries it.
    let deadline = std::time::Instant::now() + FLEET_START_SPAN * 4;
    loop {
        let state = fleet_state(emulator, case, fleet).await;
        if state == FLEET_STATE_RUNNING {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the fleet must reach {FLEET_STATE_RUNNING} within the span the \
             start owes it, still {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A `CreateStreamingURL` request for the stack, fleet and user above, carrying `extra`.
fn streaming_request(stack: &str, fleet: &str, user: &str, extra: &Value) -> Value {
    let mut request = json!({ "StackName": stack, "FleetName": fleet, "UserId": user });
    for (member, value) in extra.as_object().expect("the row carries members") {
        request[member] = value.clone();
    }
    request
}

/// The value of a query field of a URL, decoded no further than the raw text.
///
/// Shared with the other suites in this target (`tests/all/applications.rs`), so every
/// suite reads a field of a signed URL the same way.
pub(crate) fn query_field(url: &str, name: &str) -> Option<String> {
    url.split(['?', '&'])
        .filter_map(|item| item.split_once('='))
        .find(|(field, _)| *field == name)
        .map(|(_, value)| value.to_string())
}

/// Now, as the wire reports an instant: seconds since the epoch.
fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// The fleet-state gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fleet_that_is_not_streaming_refuses_a_streaming_url() {
    // Rows: what the request carries beside the identity of the stack, fleet and user.
    // The gate is on the fleet's state, so every row is refused with the same sentence
    // whatever else it asks for.
    for (case, extra) in [
        ("the required members only", json!({})),
        ("an application to launch", json!({ "ApplicationId": "app" })),
        ("a session context", json!({ "SessionContext": "ctx" })),
        ("a validity of its own", json!({ "Validity": 300 })),
        (
            "every optional member",
            json!({ "ApplicationId": "app", "SessionContext": "ctx", "Validity": VALIDITY_MAX }),
        ),
    ] {
        let emulator = Emulator::new();
        stack_with_fleet(&emulator, case, STACK, FLEET).await;

        // A created fleet is stopped, which is where the gate first bites.
        let state = fleet_state(&emulator, case, FLEET).await;
        assert_eq!(
            state, FLEET_STATE_CREATED,
            "case {case:?}: a created fleet is not streaming yet, got {state:?}"
        );

        let (status, payload) = emulator
            .call(
                "CreateStreamingURL",
                streaming_request(STACK, FLEET, USER, &extra),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a stopped fleet must refuse the call, got {status} {payload}"
        );
        assert!(
            payload.get("StreamingURL").is_none(),
            "case {case:?}: a refused call must issue no URL, got {payload}"
        );
        let message = refusal(
            &emulator,
            case,
            "CreateStreamingURL",
            streaming_request(STACK, FLEET, USER, &extra),
            400,
            "OperationNotPermittedException",
        )
        .await;
        assert_eq!(
            message,
            not_running_sentence(FLEET),
            "case {case:?}: expected AWS's sentence for a fleet that is not running, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_starting_fleet_still_refuses_a_streaming_url() {
    let case = "while the fleet is starting";
    let emulator = Emulator::new();
    stack_with_fleet(&emulator, case, STACK, FLEET).await;
    emulator.ok(case, "StartFleet", json!({ "Name": FLEET })).await;

    let state = fleet_state(&emulator, case, FLEET).await;
    assert_eq!(
        state, FLEET_STATE_STARTING,
        "case {case:?}: a fleet whose instances are coming up is not streaming yet, got \
         {state:?}"
    );
    let message = refusal(
        &emulator,
        case,
        "CreateStreamingURL",
        streaming_request(STACK, FLEET, USER, &json!({})),
        400,
        "OperationNotPermittedException",
    )
    .await;
    assert_eq!(
        message,
        not_running_sentence(FLEET),
        "case {case:?}: expected AWS's sentence for a fleet that is not running, got \
         {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Resources the request cannot reach
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_stack_or_fleet_the_request_cannot_reach_is_not_found() {
    let case = "setup";
    let emulator = Emulator::new();
    stack_with_fleet(&emulator, case, STACK, FLEET).await;
    // A second fleet, created but NOT associated with the stack: it exists, and it is
    // still not a fleet of that stack.
    emulator
        .ok(
            case,
            "CreateFleet",
            json!({ "Name": "unassociated", "InstanceType": "stream.standard.small" }),
        )
        .await;

    // Rows: which side of the request names something it cannot reach, which kind of
    // resource the refusal is about, and the name its sentence carries.
    for (case, stack, fleet, kind, missing) in [
        ("a stack nothing was created under", "absent-stack", FLEET, "stack", "absent-stack"),
        ("a fleet nothing was created under", STACK, "absent-fleet", "fleet", "absent-fleet"),
        (
            "a fleet the stack is not associated with",
            STACK,
            "unassociated",
            "fleet",
            "unassociated",
        ),
    ] {
        let message = refusal(
            &emulator,
            case,
            "CreateStreamingURL",
            streaming_request(stack, fleet, USER, &json!({})),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            not_found_sentence(kind, missing),
            "case {case:?}: expected AWS's sentence for the resource the request named, got \
             {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Issuing the URL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_running_fleet_issues_a_url_valid_for_the_validity_asked_for() {
    let case = "setup";
    let emulator = Emulator::new();
    stack_with_fleet(&emulator, case, STACK, FLEET).await;
    start_and_await_running(&emulator, case, FLEET).await;

    // Rows: what the request asks of the URL — the default validity, both published
    // bounds, a value between them, and the optional members that travel in the URL.
    for (case, extra, expected_validity) in [
        ("no validity, so the documented default", json!({}), VALIDITY_DEFAULT),
        ("the shortest validity allowed", json!({ "Validity": VALIDITY_MIN }), VALIDITY_MIN),
        ("the longest validity allowed", json!({ "Validity": VALIDITY_MAX }), VALIDITY_MAX),
        ("a validity between the bounds", json!({ "Validity": 1800 }), 1800),
        (
            "an application and a session context",
            json!({ "ApplicationId": "Desktop", "SessionContext": "ctx-1", "Validity": 120 }),
            120,
        ),
    ] {
        let before = now_epoch_seconds();
        let payload = emulator
            .ok(
                case,
                "CreateStreamingURL",
                streaming_request(STACK, FLEET, USER, &extra),
            )
            .await;
        let url = payload
            .get("StreamingURL")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a StreamingURL, got {payload}"))
            .to_string();
        assert!(
            !url.is_empty(),
            "case {case:?}: the issued URL must not be empty, got {payload}"
        );
        assert!(
            url.contains(STACK) && url.contains(FLEET) && url.contains(USER),
            "case {case:?}: the URL must address the session it was issued for, got {url:?}"
        );
        assert!(
            url.contains("X-Amz-Signature="),
            "case {case:?}: the URL must be signed so it leads back to this emulator, got \
             {url:?}"
        );
        assert_eq!(
            query_field(&url, "X-Amz-Expires"),
            Some(expected_validity.to_string()),
            "case {case:?}: the URL must be valid for the validity asked for, got {url:?}"
        );

        let expires = payload
            .get("Expires")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Expires instant, got {payload}"));
        assert!(
            expires > before,
            "case {case:?}: the URL must expire in the future, got {expires} against {before}"
        );
        // The expiry is the validity the request asked for, to the second the signature
        // was computed at, so it cannot be further out than the call plus that validity.
        let ceiling = now_epoch_seconds() + expected_validity as f64;
        assert!(
            expires <= ceiling,
            "case {case:?}: the URL must expire after the validity asked for, got {expires} \
             against {ceiling}"
        );
    }
}

#[tokio::test]
async fn a_validity_outside_the_documented_range_is_refused() {
    let case = "setup";
    let emulator = Emulator::new();
    stack_with_fleet(&emulator, case, STACK, FLEET).await;
    start_and_await_running(&emulator, case, FLEET).await;

    // Rows: values one past each published bound, and values no span can be read from.
    for validity in [
        VALIDITY_MIN - 1,
        VALIDITY_MAX + 1,
        -1,
        i64::from(i32::MIN),
        i64::MAX,
    ] {
        let case = format!("Validity {validity}");
        assert_error(
            &emulator,
            &case,
            "CreateStreamingURL",
            streaming_request(STACK, FLEET, USER, &json!({ "Validity": validity })),
            400,
            "ValidationException",
        )
        .await;
    }

    // The positive control: between the bounds the same request is answered with a URL.
    let payload = emulator
        .ok(
            "positive control",
            "CreateStreamingURL",
            streaming_request(STACK, FLEET, USER, &json!({ "Validity": VALIDITY_MAX })),
        )
        .await;
    assert!(
        payload.get("StreamingURL").is_some(),
        "case \"positive control\": a validity inside the range must be answered with a \
         URL, got {payload}"
    );
}

// ---------------------------------------------------------------------------
// The session the URL is issued for
// ---------------------------------------------------------------------------

/// The sessions a `DescribeSessions` reports, and the token it carries.
async fn describe_sessions(
    emulator: &Emulator,
    case: &str,
    request: Value,
) -> (Vec<Value>, Option<Value>) {
    let payload = emulator.ok(case, "DescribeSessions", request).await;
    let sessions = payload
        .get("Sessions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Sessions list, got {payload}"))
        .clone();
    (sessions, payload.get("NextToken").cloned())
}

/// Every session of one stack's fleet, in the order the read reports them.
async fn sessions_of(emulator: &Emulator, case: &str, stack: &str, fleet: &str) -> Vec<Value> {
    let (sessions, next_token) = describe_sessions(
        emulator,
        case,
        json!({ "StackName": stack, "FleetName": fleet }),
    )
    .await;
    assert!(
        next_token.is_none(),
        "case {case:?}: every session here fits in one page, got {next_token:?}"
    );
    sessions
}

#[tokio::test]
async fn a_refused_call_dedicates_no_session() {
    // Rows: the state the fleet is in when the call is refused. Neither state issues a
    // URL, so neither leaves a session behind for the user it was asked for.
    for (case, start) in [("a stopped fleet", false), ("a starting fleet", true)] {
        let emulator = Emulator::new();
        stack_with_fleet(&emulator, case, STACK, FLEET).await;
        if start {
            emulator.ok(case, "StartFleet", json!({ "Name": FLEET })).await;
        }

        let before = sessions_of(&emulator, case, STACK, FLEET).await;
        assert!(
            before.is_empty(),
            "case {case:?}: nothing has streamed yet, so no session must be reported, got \
             {before:?}"
        );
        refusal(
            &emulator,
            case,
            "CreateStreamingURL",
            streaming_request(STACK, FLEET, USER, &json!({})),
            400,
            "OperationNotPermittedException",
        )
        .await;

        let (for_user, _) = describe_sessions(
            &emulator,
            case,
            json!({
                "StackName": STACK,
                "FleetName": FLEET,
                "UserId": USER,
                "AuthenticationType": "API",
            }),
        )
        .await;
        assert!(
            for_user.is_empty(),
            "case {case:?}: a refused call must dedicate no session for that user, got \
             {for_user:?}"
        );
        assert!(
            sessions_of(&emulator, case, STACK, FLEET).await.is_empty(),
            "case {case:?}: a refused call must dedicate no session at all"
        );
    }
}

#[tokio::test]
async fn an_issued_url_dedicates_one_session_per_user_of_the_fleet() {
    let case = "setup";
    let max_user_duration = 3600_i64;
    let emulator = Emulator::new();
    stack_with_fleet_lasting(&emulator, case, STACK, FLEET, Some(max_user_duration)).await;
    start_and_await_running(&emulator, case, FLEET).await;

    // Rows: the users a URL is issued for. Each gets its own session of this stack's
    // fleet, and a URL issued again for one of them re-dedicates that user's session
    // rather than adding a second.
    let users = ["stream-user1", "second.user+tag@example"];
    for user in users {
        let case = format!("first URL for {user}");
        emulator
            .ok(
                &case,
                "CreateStreamingURL",
                streaming_request(STACK, FLEET, user, &json!({})),
            )
            .await;
        let (reported, _) = describe_sessions(
            &emulator,
            &case,
            json!({
                "StackName": STACK,
                "FleetName": FLEET,
                "UserId": user,
                "AuthenticationType": "API",
            }),
        )
        .await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the URL must dedicate exactly one session for that user, got \
             {reported:?}"
        );
        let session = &reported[0];
        let start_time = session
            .get("StartTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a StartTime, got {session}"));
        assert!(
            session
                .get("Id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the session must carry an identifier, got {session}"
        );
        assert_eq!(
            session.get("UserId").and_then(Value::as_str),
            Some(user),
            "case {case:?}: the session must name the user it was dedicated for, got {session}"
        );
        assert_eq!(
            session.get("StackName").and_then(Value::as_str),
            Some(STACK),
            "case {case:?}: the session must name its stack, got {session}"
        );
        assert_eq!(
            session.get("FleetName").and_then(Value::as_str),
            Some(FLEET),
            "case {case:?}: the session must name its fleet, got {session}"
        );
        assert_eq!(
            session.get("State").and_then(Value::as_str),
            Some("PENDING"),
            "case {case:?}: nobody has opened the URL, so the session is pending, got {session}"
        );
        assert_eq!(
            session.get("ConnectionState").and_then(Value::as_str),
            Some("NOT_CONNECTED"),
            "case {case:?}: no user is connected to the session, got {session}"
        );
        assert_eq!(
            session.get("AuthenticationType").and_then(Value::as_str),
            Some("API"),
            "case {case:?}: a session a streaming URL authenticates is an API session, got \
             {session}"
        );
        assert_eq!(
            session.get("InstanceId"),
            None,
            "case {case:?}: no instance is hosting the session, got {session}"
        );
        // "This time is based on the MaxUserDurationinSeconds value": the fleet's own
        // maximum, counted from the instant the session was dedicated.
        let max_expiration = session
            .get("MaxExpirationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a MaxExpirationTime, got {session}"));
        assert!(
            (max_expiration - (start_time + max_user_duration as f64)).abs() < 1.0,
            "case {case:?}: the session must expire the fleet's maximum user duration \
             after it started, got {max_expiration} against {start_time}"
        );

        // A second URL for the SAME user re-dedicates that session.
        emulator
            .ok(
                &case,
                "CreateStreamingURL",
                streaming_request(STACK, FLEET, user, &json!({ "Validity": 120 })),
            )
            .await;
        let (again, _) = describe_sessions(
            &emulator,
            &case,
            json!({
                "StackName": STACK,
                "FleetName": FLEET,
                "UserId": user,
                "AuthenticationType": "API",
            }),
        )
        .await;
        assert_eq!(
            again.len(),
            1,
            "case {case:?}: a user streams one session of a fleet at a time, got {again:?}"
        );
    }

    // Both users' sessions are reported for the fleet, and only for that fleet's stack.
    let all = sessions_of(&emulator, "both users", STACK, FLEET).await;
    assert_eq!(
        all.len(),
        users.len(),
        "case \"both users\": every dedicated session must be reported, got {all:?}"
    );
    let (elsewhere, _) = describe_sessions(
        &emulator,
        "another fleet",
        json!({ "StackName": STACK, "FleetName": "other-fleet" }),
    )
    .await;
    assert!(
        elsewhere.is_empty(),
        "case \"another fleet\": a read of another fleet reports no session of this one, \
         got {elsewhere:?}"
    );

    // A read for another authentication type reports none of them: a streaming URL
    // authenticates its user through the API.
    for authentication_type in ["SAML", "AWS_AD", "USERPOOL"] {
        let case = format!("AuthenticationType {authentication_type}");
        let (reported, _) = describe_sessions(
            &emulator,
            &case,
            json!({
                "StackName": STACK,
                "FleetName": FLEET,
                "AuthenticationType": authentication_type,
            }),
        )
        .await;
        assert!(
            reported.is_empty(),
            "case {case:?}: no session of this fleet is authenticated that way, got \
             {reported:?}"
        );
    }

    // Rows: the page size the read asks for. Following the token walks every session
    // exactly once, whatever the size.
    for limit in [Some(1_i64), Some(2), Some(50), None] {
        let case = format!("Limit {limit:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({ "StackName": STACK, "FleetName": FLEET });
            if let Some(limit) = limit {
                request["Limit"] = json!(limit);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = token.clone();
            }
            let (sessions, token) = describe_sessions(&emulator, &case, request).await;
            if let Some(limit) = limit {
                assert!(
                    i64::try_from(sessions.len()).expect("a page fits in an i64") <= limit,
                    "case {case:?}: a page must hold at most {limit} sessions, got {}",
                    sessions.len()
                );
            }
            seen.extend(sessions.iter().map(|session| {
                session
                    .get("UserId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a UserId, got {session}"))
                    .to_string()
            }));
            match token {
                Some(token) => next_token = Some(token),
                None => break,
            }
        }
        let mut expected: Vec<String> = users.iter().map(|user| (*user).to_string()).collect();
        expected.sort();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: following NextToken must report every session exactly once, got \
             {seen:?}"
        );
    }

    // A page size no session can be reported in, and a token this service never minted,
    // are refused rather than answered with an empty page. The bounds are AWS's own: at
    // least one session, and at most the documented maximum of 50.
    for limit in [0_i64, -1, 51, i64::from(i32::MAX)] {
        let case = format!("Limit {limit}");
        assert_error(
            &emulator,
            &case,
            "DescribeSessions",
            json!({ "StackName": STACK, "FleetName": FLEET, "Limit": limit }),
            400,
            "ValidationException",
        )
        .await;
    }
    for token in ["stream-user1", "abc", "zzzz"] {
        let case = format!("NextToken {token:?}");
        assert_error(
            &emulator,
            &case,
            "DescribeSessions",
            json!({ "StackName": STACK, "FleetName": FLEET, "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }
}

#[tokio::test]
async fn a_session_whose_url_stopped_verifying_is_expired() {
    let case = "a one-second URL";
    let emulator = Emulator::new();
    stack_with_fleet(&emulator, case, STACK, FLEET).await;
    start_and_await_running(&emulator, case, FLEET).await;
    emulator
        .ok(
            case,
            "CreateStreamingURL",
            streaming_request(STACK, FLEET, USER, &json!({ "Validity": VALIDITY_MIN })),
        )
        .await;

    let pending = sessions_of(&emulator, case, STACK, FLEET).await;
    assert_eq!(
        pending.len(),
        1,
        "case {case:?}: the URL must dedicate a session, got {pending:?}"
    );
    assert_eq!(
        pending[0].get("State").and_then(Value::as_str),
        Some("PENDING"),
        "case {case:?}: the session is pending while its URL still verifies, got {:?}",
        pending[0]
    );

    // The URL is temporary: once it stops verifying, the session nobody connected to is
    // expired — a transition the clock owes, which the read materialises.
    tokio::time::sleep(Duration::from_secs(VALIDITY_MIN.unsigned_abs()) + Duration::from_millis(200))
        .await;
    let expired = sessions_of(&emulator, case, STACK, FLEET).await;
    assert_eq!(
        expired.len(),
        1,
        "case {case:?}: the session is still reported after it expires, got {expired:?}"
    );
    assert_eq!(
        expired[0].get("State").and_then(Value::as_str),
        Some("EXPIRED"),
        "case {case:?}: a session whose URL stopped verifying is expired, got {:?}",
        expired[0]
    );
    // Reading again reports the same thing: the transition was due on the clock, not on
    // the call that observed it.
    let again = sessions_of(&emulator, case, STACK, FLEET).await;
    assert_eq!(
        again, expired,
        "case {case:?}: a read must not change what a later caller sees, got {again:?}"
    );
}

#[tokio::test]
async fn a_read_that_names_a_user_without_the_authentication_type_is_refused() {
    let case = "UserId without AuthenticationType";
    let emulator = Emulator::new();
    stack_with_fleet(&emulator, case, STACK, FLEET).await;

    assert_error(
        &emulator,
        case,
        "DescribeSessions",
        json!({ "StackName": STACK, "FleetName": FLEET, "UserId": USER }),
        400,
        "InvalidParameterCombinationException",
    )
    .await;

    // The positive control: the same read naming the authentication type is answered.
    let (sessions, _) = describe_sessions(
        &emulator,
        case,
        json!({
            "StackName": STACK,
            "FleetName": FLEET,
            "UserId": USER,
            "AuthenticationType": "API",
        }),
    )
    .await;
    assert!(
        sessions.is_empty(),
        "case {case:?}: nothing has streamed yet, so the read reports no session, got \
         {sessions:?}"
    );
}
