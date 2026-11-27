// SPDX-License-Identifier: CC0-1.0

//! How a stream session's placement resolves, end to end over the same router the
//! emulator serves.
//!
//! The rule under test: `StartStreamSession` persists a session in `ACTIVATING`
//! status — "Amazon GameLift Streams creates the stream session resource ... and
//! begins searching for available stream capacity to run the stream" — and the
//! placement then resolves against the capacity the hosting stream group holds.
//! "If Amazon GameLift Streams is successful in finding capacity for the stream,
//! the stream session status changes to ACTIVE status"; "if Amazon GameLift Streams
//! was not successful in finding capacity ..., the stream session status changes to
//! ERROR status and StartStreamSession returns a StatusReason of placementTimeout".
//! A session that is placed carries NO `StatusReason` — that member is "a short
//! description of the reason the stream session is in ERROR status or TERMINATED
//! status" — and a session whose `SignalRequest` carries no WebRTC offer the stream
//! server could answer fails with the modelled `invalidSignalRequest` instead.
//! `internalError` ("An internal service error occurred") is a service fault and is
//! never the outcome of a placement a request and the stored capacity decide.
//!
//! The tables span that class. The first differs along WHICH capacity member a
//! location is configured through and how large it is (always-on, on-demand, a
//! maximum, a combination, and none at all), and along how many streams are asked
//! for against it; the second differs along the FORM of the signal request (a JSON
//! session description, a bare SDP offer, the documented no-client sentinel, and
//! four kinds of value that are no offer at all). The third follows a placed
//! session through the operations a live stream gates.
//!
//! The last table is the other half of what a placement settles: the lifecycle
//! instants a session reports. A session reports each of them only once the event it
//! records has happened to it — `ApplicationStartedAt` once its placement found
//! capacity to launch the application on, and `StreamingStartedAt` and
//! `StreamingEndedAt` once a client connection starts and ends streaming, which
//! needs the stream server's answer to the `SignalRequest` and so happens nowhere in
//! this emulator. Its rows differ along which of those events a session reached:
//! none yet, a placement that launched the application, a placement that found no
//! capacity, a request that was no offer, and a launched stream that was then
//! terminated.
//!
//! Every assertion reads a RESPONSE BODY, through `GetStreamSession`, so a
//! placement that moved nothing in state fails here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_read_projection::{poll_until, stream_group_fully_active};
use super::stream_group_capacity::group_with_locations;
use super::stream_workflow::{
    call, emulator, ready_application, text, PAST_TRANSITION, REGION,
};

/// A complete-enough WebRTC offer: the JSON session description a browser hands a
/// backend, whose `sdp` opens with SDP's single version line (`v=0`) and carries a
/// media line.
const OFFER: &str = concat!(
    r#"{"type":"offer","sdp":"v=0\r\no=- 4611731400430051336 2 IN IP4 127.0.0.1\r\n"#,
    r#"s=-\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=recvonly\r\n"}"#
);

/// Start one stream session and answer the body the start itself reported.
async fn started_session(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    application: &str,
    signal_request: &str,
) -> Value {
    let (status, started) = call(
        router,
        "POST",
        &format!("/streamgroups/{group}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": signal_request,
            "ApplicationIdentifier": application,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected StartStreamSession 201, got {status} {started}"
    );
    assert_eq!(
        started["Status"], "ACTIVATING",
        "case {case:?}: a start begins the search for capacity, got {started}"
    );
    started
}

/// The session id a stream-session body's ARN ends in.
fn session_id_of(body: &Value, case: &str) -> String {
    text(body, "/Arn", case)
        .rsplit('/')
        .next()
        .expect("a stream session ARN ends in the session id")
        .to_string()
}

/// Start one stream session and answer its id. `signal_request` is the offer it
/// carries; `locations` is left out of the request when empty.
async fn start_session(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    application: &str,
    signal_request: &str,
) -> String {
    let started = started_session(router, case, group, application, signal_request).await;
    session_id_of(&started, case)
}

/// Read one stream session back and answer its whole body.
async fn read_session(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    session: &str,
) -> Value {
    let (status, got) = call(
        router,
        "GET",
        &format!("/streamgroups/{group}/streamsessions/{session}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetStreamSession 200, got {status} {got}"
    );
    got
}

/// The `(Status, StatusReason)` pair a stream session reads back as, with an absent
/// reason reported as `None`.
async fn status_of(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    session: &str,
) -> (String, Option<String>) {
    let got = read_session(router, case, group, session).await;
    assert!(
        got.get("SignalResponse").is_none(),
        "case {case:?}: the WebRTC answer comes from a stream server, and none ran, \
         so SignalResponse must be absent, got {got}"
    );
    (
        text(&got, "/Status", case),
        got.get("StatusReason")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// One row of the capacity table: the case, the location configuration the group is
/// created with, how many streams are asked for, and how many of them the location
/// can run at once.
struct CapacityCase {
    case: &'static str,
    configuration: Value,
    asked_for: usize,
    capacity: usize,
}

/// A placement finds capacity while the location it runs in has a free stream, and
/// times out once every stream the location can run is taken.
#[tokio::test]
async fn a_placement_resolves_against_the_capacity_of_its_location() {
    // The capacity members come from `smithy_query(query="shape",
    // shape="LocationConfiguration")`: `AlwaysOnCapacity` is "minimum streaming
    // capacity which is allocated to you and is never released back to the
    // service", `OnDemandCapacity` is "the streaming capacity that Amazon GameLift
    // Streams can allocate in response to stream requests", and `MaximumCapacity`
    // is "the maximum capacity that the service can allocate for you". The rows
    // span each of them alone, a combination, a size well above one stream, and a
    // location configured with no capacity at all.
    let cases = [
        CapacityCase {
            case: "one always-on stream",
            configuration: json!({"LocationName": REGION, "AlwaysOnCapacity": 1}),
            asked_for: 2,
            capacity: 1,
        },
        CapacityCase {
            case: "two on-demand streams",
            configuration: json!({"LocationName": REGION, "OnDemandCapacity": 2}),
            asked_for: 3,
            capacity: 2,
        },
        CapacityCase {
            case: "a maximum of three streams",
            configuration: json!({"LocationName": REGION, "MaximumCapacity": 3}),
            asked_for: 4,
            capacity: 3,
        },
        CapacityCase {
            case: "always-on beside a larger maximum",
            configuration: json!({
                "LocationName": REGION,
                "AlwaysOnCapacity": 1,
                "MaximumCapacity": 4,
            }),
            asked_for: 5,
            capacity: 4,
        },
        CapacityCase {
            case: "no capacity at all",
            configuration: json!({"LocationName": REGION, "MaximumCapacity": 0}),
            asked_for: 1,
            capacity: 0,
        },
    ];

    for CapacityCase {
        case,
        configuration,
        asked_for,
        capacity,
    } in cases
    {
        let router = emulator();
        let (app_id, app_arn) = ready_application(&router, case).await;
        let (group_id, _, _) = group_with_locations(
            &router,
            case,
            &app_arn,
            std::slice::from_ref(&configuration),
        )
        .await;
        std::thread::sleep(PAST_TRANSITION);

        let mut sessions = Vec::new();
        for _ in 0..asked_for {
            sessions.push(start_session(&router, case, &group_id, &app_id, OFFER).await);
        }
        // The placements come due one TRANSITION after each start; nothing sleeps
        // for them in the emulator, so one wait settles them all.
        std::thread::sleep(PAST_TRANSITION);

        for (index, session) in sessions.iter().enumerate() {
            let row = format!("{case}: stream {} of {asked_for}", index + 1);
            let (status, reason) = status_of(&router, &row, &group_id, session).await;
            // Capacity goes to the streams that asked for it first.
            let expected = if index < capacity {
                ("ACTIVE".to_string(), None)
            } else {
                (
                    "ERROR".to_string(),
                    Some("placementTimeout".to_string()),
                )
            };
            assert_eq!(
                (status.clone(), reason.clone()),
                expected,
                "case {row:?}: a location with {capacity} stream(s) of capacity \
                 expected {expected:?}, got {status:?} {reason:?}"
            );
        }

        // A read reports what the clock already owed and moves nothing: the same
        // session reads back the same way, with the same LastUpdatedAt.
        let first = read_session(&router, case, &group_id, &sessions[0]).await;
        let again = read_session(&router, case, &group_id, &sessions[0]).await;
        assert_eq!(
            (first["Status"].clone(), first["LastUpdatedAt"].clone()),
            (again["Status"].clone(), again["LastUpdatedAt"].clone()),
            "case {case:?}: a read must not move the session, got {first} then {again}"
        );
    }
}

/// A placement fails with `invalidSignalRequest` exactly when the request carries no
/// WebRTC offer the stream server could answer — and with no reason at all when it
/// does.
#[tokio::test]
async fn a_signal_request_that_carries_no_offer_fails_the_placement() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "signal-request").await;
    // Room for every row of the table, so capacity is never what decides a row.
    let configuration = json!({"LocationName": REGION, "MaximumCapacity": 12});
    let (group_id, _, _) = group_with_locations(
        &router,
        "signal-request-group",
        &app_arn,
        std::slice::from_ref(&configuration),
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);

    // (case, SignalRequest, the StatusReason the placement ends with, or None when
    // the placement succeeds). `SignalRequest` is "a WebRTC ICE offer string to use
    // when initializing a WebRTC connection. Typically, the offer is a very long
    // JSON string", and the service "also supports setting the field to
    // NO_CLIENT_CONNECTION", so the accepted forms are the JSON session
    // description, a bare SDP offer, and that sentinel.
    let cases: [(&str, &str, Option<&str>); 8] = [
        ("a JSON session description", OFFER, None),
        (
            "a JSON description that names no type",
            r#"{"sdp":"v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n"}"#,
            None,
        ),
        (
            "a bare SDP offer",
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=video 9 RTP/SAVPF 96\r\n",
            None,
        ),
        ("the no-client sentinel", "NO_CLIENT_CONNECTION", None),
        (
            "a WebRTC answer rather than an offer",
            r#"{"type":"answer","sdp":"v=0\r\nm=video 9 RTP/SAVPF 96\r\n"}"#,
            Some("invalidSignalRequest"),
        ),
        (
            "a session description with no SDP in it",
            r#"{"type":"offer"}"#,
            Some("invalidSignalRequest"),
        ),
        (
            "a word that is no session description",
            "offer",
            Some("invalidSignalRequest"),
        ),
        (
            "an SDP body of an unknown version",
            "v=1\r\no=- 0 0 IN IP4 127.0.0.1\r\n",
            Some("invalidSignalRequest"),
        ),
    ];

    let mut started = Vec::new();
    for (case, signal_request, expected_reason) in cases {
        let session = start_session(&router, case, &group_id, &app_id, signal_request).await;
        started.push((case, session, expected_reason));
    }
    std::thread::sleep(PAST_TRANSITION);

    for (case, session, expected_reason) in started {
        let (status, reason) = status_of(&router, case, &group_id, &session).await;
        let expected = match expected_reason {
            None => ("ACTIVE".to_string(), None),
            Some(reason) => ("ERROR".to_string(), Some(reason.to_string())),
        };
        assert_eq!(
            (status.clone(), reason.clone()),
            expected,
            "case {case:?}: expected {expected:?}, got {status:?} {reason:?}"
        );
    }
}

/// A placed session is a live stream: it is reported as `ACTIVE` by both readers, it
/// gets past the status gate of the operations that need a live stream, and
/// terminating it hands its capacity back to the location.
#[tokio::test]
async fn a_placed_session_is_a_live_stream() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "live-stream").await;
    // One stream of capacity, so the capacity the placed session holds — and the
    // capacity its termination releases — is observable.
    let configuration = json!({"LocationName": REGION, "AlwaysOnCapacity": 1});
    let (group_id, _, _) = group_with_locations(
        &router,
        "live-stream-group",
        &app_arn,
        std::slice::from_ref(&configuration),
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);

    let case = "a placed session";
    let session_id = start_session(&router, case, &group_id, &app_id, OFFER).await;
    std::thread::sleep(PAST_TRANSITION);
    let (status, reason) = status_of(&router, case, &group_id, &session_id).await;
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("ACTIVE", None),
        "case {case:?}: expected a placed stream, got {status:?} {reason:?}"
    );

    // The list reader is a stub of its own (`ListStreamSessions` still answers 501),
    // so the single reader is what the placement is read back through here.

    // The status gate of `CreateStreamSessionAdminShell` — "The stream session must
    // be in one of the following statuses: ACTIVE, CONNECTED,
    // PENDING_CLIENT_RECONNECTION, or RECONNECTING" — now admits the session, and
    // the call grants the terminal session it documents: a Systems Manager session
    // id, that session's websocket endpoint, and an authentication token.
    let access = format!("/streamgroups/{group_id}/streamsessions/{session_id}/access");
    let (shell_status, granted) = call(&router, "POST", &access, None).await;
    assert_eq!(
        shell_status, 200,
        "case {case:?}: expected CreateStreamSessionAdminShell 200 on an eligible \
         session, got {shell_status} {granted}"
    );
    let shell_session_id = text(&granted, "/SessionId", case);
    assert!(
        shell_session_id.starts_with(&format!("{session_id}-")),
        "case {case:?}: the session id must name the stream session the terminal is \
         into, got {shell_session_id:?}"
    );
    let shell_url = text(&granted, "/StreamUrl", case);
    assert_eq!(
        shell_url,
        format!(
            "wss://ssmmessages.{REGION}.amazonaws.com/v1/data-channel/\
             {shell_session_id}?role=publish_subscribe"
        ),
        "case {case:?}: expected the Systems Manager data channel of this session, \
         got {shell_url:?}"
    );
    let token = text(&granted, "/TokenValue", case);
    assert!(
        !token.is_empty() && token.len() <= 1024,
        "case {case:?}: TokenValue is modelled len 0..1024 and authenticates the \
         channel, got {} characters",
        token.len()
    );

    // "The StreamUrl is valid for 60 seconds. After it expires, call this operation
    // again to get a new URL": a second call grants a DIFFERENT terminal session
    // rather than replaying the first.
    let (again_status, again) = call(&router, "POST", &access, None).await;
    assert_eq!(
        again_status, 200,
        "case {case:?}: expected a second grant, got {again_status} {again}"
    );
    assert_ne!(
        text(&again, "/SessionId", case),
        shell_session_id,
        "case {case:?}: each call grants its own terminal session, got {again}"
    );
    assert_ne!(
        text(&again, "/TokenValue", case),
        token,
        "case {case:?}: each grant carries its own token, got {again}"
    );

    // And the grant does not move the stream session itself.
    let (status, reason) = status_of(&router, case, &group_id, &session_id).await;
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("ACTIVE", None),
        "case {case:?}: a terminal session must not move the stream, got {status:?} \
         {reason:?}"
    );

    // While the stream runs it holds the location's one stream, so the next start
    // finds no capacity.
    let crowded = "a second stream against one of capacity";
    let second = start_session(&router, crowded, &group_id, &app_id, OFFER).await;
    std::thread::sleep(PAST_TRANSITION);
    let (status, reason) = status_of(&router, crowded, &group_id, &second).await;
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("ERROR", Some("placementTimeout")),
        "case {crowded:?}: expected the modelled capacity failure, got {status:?} \
         {reason:?}"
    );

    // Terminating the placed stream releases its capacity: once it is TERMINATED,
    // the next placement finds the location's stream free again.
    let (terminate_status, terminate_body) = call(
        &router,
        "DELETE",
        &format!("/streamgroups/{group_id}/streamsessions/{session_id}"),
        None,
    )
    .await;
    assert_eq!(
        terminate_status, 204,
        "case {case:?}: expected TerminateStreamSession 204, got {terminate_status} \
         {terminate_body}"
    );
    std::thread::sleep(PAST_TRANSITION);
    let (status, reason) = status_of(&router, case, &group_id, &session_id).await;
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("TERMINATED", Some("apiTerminated")),
        "case {case:?}: expected the stopped stream, got {status:?} {reason:?}"
    );

    let released = "a stream placed on the released capacity";
    let third = start_session(&router, released, &group_id, &app_id, OFFER).await;
    std::thread::sleep(PAST_TRANSITION);
    let (status, reason) = status_of(&router, released, &group_id, &third).await;
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("ACTIVE", None),
        "case {released:?}: expected the freed stream to be placed, got {status:?} \
         {reason:?}"
    );
}

/// The instants a stream session reports for its own lifecycle, from
/// `smithy_query(query="operation", operation="GetStreamSession")`:
/// `ApplicationStartedAt` is "when the application launched for this stream session",
/// `StreamingStartedAt` "when streaming started for this stream session, which is
/// when a client connected", and `StreamingEndedAt` "when streaming ended for this
/// stream session".
const LIFECYCLE_INSTANTS: [&str; 3] = [
    "ApplicationStartedAt",
    "StreamingStartedAt",
    "StreamingEndedAt",
];

/// The two instants that only a client connection can open and close, and which this
/// emulator therefore never reports: a connection needs the stream server's answer to
/// the `SignalRequest`, and no stream server runs here.
const CLIENT_STREAM_INSTANTS: [&str; 2] = ["StreamingStartedAt", "StreamingEndedAt"];

/// One instant of a session body, as the seconds since the epoch the wire carries, or
/// `None` for a member the body does not report.
fn instant(body: &Value, member: &str) -> Option<f64> {
    body.get(member).map(|at| {
        at.as_f64()
            .unwrap_or_else(|| panic!("{member} is a timestamp, got {at}"))
    })
}

/// Assert `body` reports none of the instants in `members`.
fn assert_absent(body: &Value, members: &[&str], case: &str) {
    for member in members {
        assert!(
            instant(body, member).is_none(),
            "case {case:?}: nothing has happened to report {member} for, so it must be \
             absent, got {body}"
        );
    }
}

/// One row: the case, the `SignalRequest` the start carries, and whether its
/// placement finds capacity to launch the application on.
struct LifecycleCase {
    case: &'static str,
    signal_request: &'static str,
    launched: bool,
}

/// A session reports an event's instant once that event has happened to it, and not
/// before: the application launch at the placement that ran it, and the streaming
/// instants not at all while no client can connect.
#[tokio::test]
async fn a_session_reports_the_instants_of_the_events_that_happened_to_it() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "lifecycle-instants").await;
    // Two streams of capacity, so the two rows that are placed find it and the row
    // after them does not.
    let configuration = json!({"LocationName": REGION, "AlwaysOnCapacity": 2});
    let (group_id, _, _) = group_with_locations(
        &router,
        "lifecycle-instants-group",
        &app_arn,
        std::slice::from_ref(&configuration),
    )
    .await;
    stream_group_fully_active(&router, &group_id, "lifecycle-instants").await;

    let rows = [
        LifecycleCase {
            case: "a placement that launched the application",
            signal_request: OFFER,
            launched: true,
        },
        LifecycleCase {
            // The documented no-client sentinel starts a stream all the same, so the
            // application launches for it too.
            case: "a placement from the no-client sentinel",
            signal_request: "NO_CLIENT_CONNECTION",
            launched: true,
        },
        LifecycleCase {
            case: "a placement the location had no capacity for",
            signal_request: OFFER,
            launched: false,
        },
        LifecycleCase {
            case: "a request that carried no offer to place",
            signal_request: r#"{"type":"answer","sdp":"v=0\r\nm=video 9 RTP/SAVPF 96\r\n"}"#,
            launched: false,
        },
    ];

    let mut started = Vec::new();
    for LifecycleCase {
        case,
        signal_request,
        launched,
    } in rows
    {
        // The start's own body: the session "is starting and preparing to stream", so
        // nothing has launched or streamed for it yet.
        let body = started_session(&router, case, &group_id, &app_id, signal_request).await;
        assert_absent(&body, &LIFECYCLE_INSTANTS, case);
        started.push((case, session_id_of(&body, case), launched));
    }

    let mut placed_launch = Vec::new();
    for (case, session, launched) in &started {
        let path = format!("/streamgroups/{group_id}/streamsessions/{session}");
        // Wait for the CONDITION the placement settles into, never for a fixed time.
        let settled = poll_until(&router, &path, case, |body| {
            matches!(body["Status"].as_str(), Some("ACTIVE" | "ERROR"))
        })
        .await;
        // A client connection is what starts and ends streaming, and none can be made
        // here, whatever the placement did.
        assert_absent(&settled, &CLIENT_STREAM_INSTANTS, case);

        if *launched {
            assert_eq!(
                settled["Status"], "ACTIVE",
                "case {case:?}: expected a placed stream, got {settled}"
            );
            let launched_at = instant(&settled, "ApplicationStartedAt").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a placement that found capacity launched the \
                     application, so ApplicationStartedAt must be reported, got {settled}"
                )
            });
            // The launch happened at the placement, which is the last thing that moved
            // the session, and never before the session existed.
            assert_eq!(
                Some(launched_at),
                instant(&settled, "LastUpdatedAt"),
                "case {case:?}: the application launched when the placement completed, \
                 got {settled}"
            );
            let created_at = instant(&settled, "CreatedAt")
                .unwrap_or_else(|| panic!("case {case:?}: a created session, got {settled}"));
            assert!(
                launched_at >= created_at,
                "case {case:?}: expected the launch at or after the start {created_at}, got \
                 {launched_at}"
            );
            placed_launch.push((case, session.clone(), launched_at));
        } else {
            assert_eq!(
                settled["Status"], "ERROR",
                "case {case:?}: expected a placement that found nothing, got {settled}"
            );
            assert_absent(&settled, &["ApplicationStartedAt"], case);
        }

        // Reading a session reports the same instants again: nothing about them moves
        // because somebody looked.
        let again = poll_until(&router, &path, case, |_| true).await;
        for member in LIFECYCLE_INSTANTS {
            assert_eq!(
                instant(&again, member),
                instant(&settled, member),
                "case {case:?}: {member} must not move between two reads, got {again}"
            );
        }
    }

    // A launched stream that is then terminated keeps the instant its application
    // launched at — the launch happened — and still reports no streaming instants,
    // because no client ever connected to the stream it ran.
    let (case, session, launched_at) = placed_launch
        .first()
        .expect("the table places at least one stream")
        .clone();
    let path = format!("/streamgroups/{group_id}/streamsessions/{session}");
    let (status, body) = call(&router, "DELETE", &path, None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected TerminateStreamSession 204, got {status} {body}"
    );
    let stopped = poll_until(&router, &path, case, |body| {
        body["Status"] == json!("TERMINATED")
    })
    .await;
    assert_eq!(
        instant(&stopped, "ApplicationStartedAt"),
        Some(launched_at),
        "case {case:?}: a stopped stream keeps the instant its application launched at, \
         got {stopped}"
    );
    assert_absent(&stopped, &CLIENT_STREAM_INSTANTS, case);
}
