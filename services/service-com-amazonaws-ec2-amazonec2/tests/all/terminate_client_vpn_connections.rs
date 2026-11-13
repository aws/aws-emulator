// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `TerminateClientVpnConnections`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=TerminateClientVpnConnections`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation TERMINATES THE CONNECTIONS THE STORE HOLDS for
//! the endpoint the request names: the one connection its `ConnectionId` names, or every
//! ACTIVE connection its `Username` names, each moved out of `active` into `terminating`
//! (the clock owing it `terminated` a transition span later) and reported with the status it
//! held before the call beside the one it holds after it — refusing, and writing nothing, a
//! request that names no endpoint, an endpoint this account holds none of, both selectors or
//! neither, a connection the endpoint does not hold, or a user it holds no active connection
//! for.
//!
//! Every table row is a distinct input along what that generalises over: which selector names
//! the connections, how many connections one user holds, whether a connection carries a user
//! at all, which endpoint holds it, how far through its termination it already is, and which
//! form of unresolvable value the request names. The recorded case — a terminate naming an
//! endpoint id this account holds no endpoint for — is ONE row of the refusal table.

use std::time::{Duration, Instant, SystemTime};

use ec2::ops::types::ClientVpnEndpoint;
use ec2::state::{
    ClientVpnConnectionSpec, ComAmazonawsEc2Amazonec2State as Ec2State,
    CLIENT_VPN_CONNECTION_STATUS_ACTIVE, CLIENT_VPN_CONNECTION_STATUS_TERMINATED,
    CLIENT_VPN_CONNECTION_STATUS_TERMINATING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items, Param};

/// The Region every call here is signed for. The Client VPN stores are per account rather
/// than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "TerminateClientVpnConnections";

/// EC2's answer to an authorized dry run, the status it carries it on, and its wording.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// How long a poll below is given before it reports that a connection never left
/// `terminating` — generously past [`ec2::state::TRANSITION_DELAY`], so a slow machine fails
/// nothing, while a status the loop's guard never matches still ends the test instead of
/// hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(10);

/// How long a poll waits between calls, as a client's own waiter does.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> Param {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One Client VPN endpoint, written the way `CreateClientVpnEndpoint` would, answering the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine, description: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            transport_protocol: Some("udp".to_string()),
            vpn_port: Some(443),
            ..Default::default()
        })
        .unwrap_or_else(|| panic!("the store must take the Client VPN endpoint {description:?}"))
}

/// One client connection of `endpoint_id`, written the way the endpoint's own VPN data plane
/// records one, answering the `ConnectionId` it was minted with.
fn seed_connection(
    engine: &engine::InMemoryEngine,
    endpoint_id: &str,
    username: Option<&str>,
    client_ip: &str,
) -> String {
    let connection = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_connection(ClientVpnConnectionSpec {
            client_vpn_endpoint_id: endpoint_id.to_string(),
            username: username.map(str::to_string),
            common_name: Some(username.unwrap_or("client.example.com").to_string()),
            client_ip: Some(client_ip.to_string()),
            established_at: Some(SystemTime::now()),
        })
        .unwrap_or_else(|| panic!("the store must take a connection of {endpoint_id}"));
    connection
        .connection_id
        .unwrap_or_else(|| panic!("a stored connection of {endpoint_id} must carry an id"))
}

/// The `ClientVpnConnectionStatusCode` the STORE holds for `connection_id`, which is what a
/// later caller would be told — read without any call, so an assertion on it is about
/// persisted state rather than about a response.
fn stored_status(engine: &engine::InMemoryEngine, connection_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_connections
        .get(connection_id)
        .map(|record| ec2::state::client_vpn_connection_status_code(record).to_string())
}

/// The instant the store recorded as the end of `connection_id`
/// (`ClientVpnConnection$ConnectionEndTime`), or `None` while it carries none.
fn stored_end_time(engine: &engine::InMemoryEngine, connection_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_connections
        .get(connection_id)
        .and_then(|record| record.view.connection_end_time.clone())
}

/// The status of every connection the store holds, by connection id — the "nothing else
/// moved" assertions compare two of these.
fn stored_statuses(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_connections
        .iter()
        .map(|(connection_id, record)| {
            (
                connection_id.clone(),
                ec2::state::client_vpn_connection_status_code(record).to_string(),
            )
        })
        .collect()
}

/// One terminate call carrying `params`, as its HTTP status and XML body.
async fn terminate(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// One reported termination: the connection, the status it held before the call and the one
/// it holds after it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    connection_id: String,
    previous: String,
    current: String,
}

/// The `connectionStatuses` a successful answer carries, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "connectionStatuses")
        .into_iter()
        .map(|item| Reported {
            connection_id: child_text(item, "connectionId").unwrap_or_default(),
            previous: child(item, "previousStatus")
                .and_then(|status| child_text(status, "code"))
                .unwrap_or_default(),
            current: child(item, "currentStatus")
                .and_then(|status| child_text(status, "code"))
                .unwrap_or_default(),
        })
        .collect()
}

/// The envelope's own `name` member: `None` when the answer carries none.
fn envelope_member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    child_text(parsed.root_element(), name)
}

/// One terminate call that has to be served, answering its body.
async fn served(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = terminate(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// Terminate `connection_id` again and again, exactly as a client's own waiter does, until
/// the answer stops reporting it `terminating`, and answer the status it then held.
///
/// The call is idempotent — a connection that is no longer active is reported unchanged — so
/// polling with it neither moves the connection a second time nor hides the move the clock
/// owes it.
async fn poll_until_past_terminating(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
    connection_id: &str,
) -> String {
    let params = vec![
        p("ClientVpnEndpointId", endpoint_id),
        p("ConnectionId", connection_id),
    ];
    let current = |body: &str| {
        reported(case, body)
            .first()
            .map(|status| status.current.clone())
            .unwrap_or_default()
    };
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut status = current(&served(case, router, &params).await);
    while status == CLIENT_VPN_CONNECTION_STATUS_TERMINATING && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        status = current(&served(case, router, &params).await);
    }
    status
}

// ---------------------------------------------------------------------------
// Naming ONE connection
// ---------------------------------------------------------------------------

/// One seeded endpoint and the connections on it, for the tables below: a user with three
/// connections, a user with one, and a connection authenticated by certificate that carries
/// no user at all.
struct Seeded {
    endpoint_id: String,
    /// The other endpoint of the same account, holding a connection of the same user — what
    /// makes every assertion below about the endpoint the request named.
    other_endpoint_id: String,
    /// (username, connection id) for each connection of `endpoint_id`, in seeding order.
    connections: Vec<(Option<String>, String)>,
    /// The connection the other endpoint holds for the user with three connections here.
    other_endpoint_connection: String,
}

/// The username rows below select on: one holding several connections, one holding a single
/// connection, and one the endpoint has never seen.
const BUSY_USER: &str = "alice";
const QUIET_USER: &str = "bob";
const UNKNOWN_USER: &str = "carol";

/// Seed the two endpoints and their connections.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let endpoint_id = seed_endpoint(engine, "the endpoint under test");
    let other_endpoint_id = seed_endpoint(engine, "a second endpoint of the same account");
    let connections = vec![
        (
            Some(BUSY_USER.to_string()),
            seed_connection(engine, &endpoint_id, Some(BUSY_USER), "10.100.0.10"),
        ),
        (
            Some(BUSY_USER.to_string()),
            seed_connection(engine, &endpoint_id, Some(BUSY_USER), "10.100.0.11"),
        ),
        (
            Some(BUSY_USER.to_string()),
            seed_connection(engine, &endpoint_id, Some(BUSY_USER), "10.100.0.12"),
        ),
        (
            Some(QUIET_USER.to_string()),
            seed_connection(engine, &endpoint_id, Some(QUIET_USER), "10.100.0.20"),
        ),
        (
            None,
            seed_connection(engine, &endpoint_id, None, "10.100.0.30"),
        ),
    ];
    let other_endpoint_connection =
        seed_connection(engine, &other_endpoint_id, Some(BUSY_USER), "10.100.0.40");
    Seeded {
        endpoint_id,
        other_endpoint_id,
        connections,
        other_endpoint_connection,
    }
}

/// RULE: a terminate naming ONE connection moves that connection out of `active` into
/// `terminating`, reports both statuses, and leaves every other connection — of the same
/// user, of another user, and of another endpoint — exactly as it was.
#[tokio::test]
async fn naming_one_connection_terminates_that_connection_only() {
    // Rows differing along what the named connection is: one of several a user holds, the
    // only one a user holds, and one carrying no user at all.
    let rows: [(&str, usize); 4] = [
        ("one of the three connections a single user holds", 0),
        ("the last of that user's connections", 2),
        ("the only connection of another user", 3),
        (
            "a connection authenticated by certificate, which carries no user",
            4,
        ),
    ];

    for (case, index) in rows {
        let (router, engine) = emulator();
        let seeded = seed(&engine);
        let (username, connection_id) = seeded.connections[index].clone();
        let before = stored_statuses(&engine);

        let body = served(
            case,
            &router,
            &[
                p("ClientVpnEndpointId", &seeded.endpoint_id),
                p("ConnectionId", &connection_id),
            ],
        )
        .await;

        assert_eq!(
            envelope_member(case, &body, "clientVpnEndpointId").as_deref(),
            Some(seeded.endpoint_id.as_str()),
            "case {case:?}: expected the endpoint the request named to be reported, \
             got body {body}"
        );
        assert_eq!(
            envelope_member(case, &body, "username"),
            username,
            "case {case:?}: expected the user who established the connection to be \
             reported, got body {body}"
        );
        assert_eq!(
            reported(case, &body),
            vec![Reported {
                connection_id: connection_id.clone(),
                previous: CLIENT_VPN_CONNECTION_STATUS_ACTIVE.to_string(),
                current: CLIENT_VPN_CONNECTION_STATUS_TERMINATING.to_string(),
            }],
            "case {case:?}: expected the one named connection to be reported moving out of \
             {CLIENT_VPN_CONNECTION_STATUS_ACTIVE}, got body {body}"
        );

        // The STORE holds that move, and holds it for that connection alone.
        assert_eq!(
            stored_status(&engine, &connection_id).as_deref(),
            Some(CLIENT_VPN_CONNECTION_STATUS_TERMINATING),
            "case {case:?}: expected the store to hold {connection_id} \
             {CLIENT_VPN_CONNECTION_STATUS_TERMINATING}"
        );
        assert!(
            stored_end_time(&engine, &connection_id).is_some(),
            "case {case:?}: expected the store to record when {connection_id} ended"
        );
        let after = stored_statuses(&engine);
        let moved: Vec<(String, String)> = before
            .iter()
            .zip(after.iter())
            .filter(|(was, now)| was != now)
            .map(|(_, now)| now.clone())
            .collect();
        assert_eq!(
            moved,
            vec![(
                connection_id.clone(),
                CLIENT_VPN_CONNECTION_STATUS_TERMINATING.to_string()
            )],
            "case {case:?}: expected exactly one connection to have moved, got {after:?} \
             from {before:?}"
        );
    }
}

/// RULE: a connection id that names a connection of ANOTHER endpoint is not a connection of
/// the endpoint the request named — the selector resolves against that endpoint's own
/// connections.
#[tokio::test]
async fn a_connection_of_another_endpoint_is_not_this_endpoints_connection() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let case = "a connection id belonging to the account's other endpoint";
    let before = stored_statuses(&engine);

    let (status, body) = terminate(
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.endpoint_id),
            p("ConnectionId", &seeded.other_endpoint_connection),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidClientVpnConnection.IdNotFound",
        "case {case:?}: got body {body}"
    );
    assert_eq!(
        stored_statuses(&engine),
        before,
        "case {case:?}: a refused terminate must leave every connection as it was"
    );

    // Named on the endpoint that DOES hold it, the very same id is served.
    let served_case = "that same connection, on the endpoint that holds it";
    let body = served(
        served_case,
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.other_endpoint_id),
            p("ConnectionId", &seeded.other_endpoint_connection),
        ],
    )
    .await;
    assert_eq!(
        reported(served_case, &body)
            .first()
            .map(|status| status.current.clone())
            .unwrap_or_default(),
        CLIENT_VPN_CONNECTION_STATUS_TERMINATING,
        "case {served_case:?}: expected the connection to be terminated, got body {body}"
    );
}

// ---------------------------------------------------------------------------
// Naming a USER
// ---------------------------------------------------------------------------

/// RULE: a terminate naming a USER moves every ACTIVE connection that user holds on that
/// endpoint, however many that is, reports one status per connection and echoes the user —
/// leaving the other users' connections and the other endpoint's alone.
#[tokio::test]
async fn naming_a_user_terminates_every_active_connection_of_that_user() {
    // Rows differing along how many active connections the named user holds on the endpoint.
    let rows: [(&str, &str, usize); 2] = [
        ("a user holding three connections", BUSY_USER, 3),
        ("a user holding a single connection", QUIET_USER, 1),
    ];

    for (case, username, expected) in rows {
        let (router, engine) = emulator();
        let seeded = seed(&engine);
        let selected: Vec<String> = seeded
            .connections
            .iter()
            .filter(|(user, _)| user.as_deref() == Some(username))
            .map(|(_, connection_id)| connection_id.clone())
            .collect();
        assert_eq!(
            selected.len(),
            expected,
            "case {case:?}: the seeding must give {username} {expected} connection(s), \
             got {selected:?}"
        );

        let body = served(
            case,
            &router,
            &[
                p("ClientVpnEndpointId", &seeded.endpoint_id),
                p("Username", username),
            ],
        )
        .await;

        assert_eq!(
            envelope_member(case, &body, "username").as_deref(),
            Some(username),
            "case {case:?}: expected the user the request named to be reported, \
             got body {body}"
        );
        let expected_statuses: Vec<Reported> = selected
            .iter()
            .map(|connection_id| Reported {
                connection_id: connection_id.clone(),
                previous: CLIENT_VPN_CONNECTION_STATUS_ACTIVE.to_string(),
                current: CLIENT_VPN_CONNECTION_STATUS_TERMINATING.to_string(),
            })
            .collect();
        assert_eq!(
            reported(case, &body),
            expected_statuses,
            "case {case:?}: expected one status per active connection of {username}, \
             got body {body}"
        );

        // Every connection of that user moved, and nothing else did — including the
        // connection the SAME user holds on the account's other endpoint.
        for connection_id in &selected {
            assert_eq!(
                stored_status(&engine, connection_id).as_deref(),
                Some(CLIENT_VPN_CONNECTION_STATUS_TERMINATING),
                "case {case:?}: expected the store to hold {connection_id} \
                 {CLIENT_VPN_CONNECTION_STATUS_TERMINATING}"
            );
        }
        for (user, connection_id) in &seeded.connections {
            if user.as_deref() == Some(username) {
                continue;
            }
            assert_eq!(
                stored_status(&engine, connection_id).as_deref(),
                Some(CLIENT_VPN_CONNECTION_STATUS_ACTIVE),
                "case {case:?}: expected the connection {connection_id} of {user:?} to be \
                 left {CLIENT_VPN_CONNECTION_STATUS_ACTIVE}"
            );
        }
        assert_eq!(
            stored_status(&engine, &seeded.other_endpoint_connection).as_deref(),
            Some(CLIENT_VPN_CONNECTION_STATUS_ACTIVE),
            "case {case:?}: expected the other endpoint's connection for {username} to be \
             left {CLIENT_VPN_CONNECTION_STATUS_ACTIVE}"
        );
    }
}

/// RULE: once a user's connections are no longer active, the endpoint holds no active
/// connection for that user — so naming the user again is refused rather than reporting a
/// move nothing made.
#[tokio::test]
async fn a_user_whose_connections_are_already_terminating_has_none_to_terminate() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let case = "the same user twice";

    let first = served(
        case,
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.endpoint_id),
            p("Username", QUIET_USER),
        ],
    )
    .await;
    assert_eq!(
        reported(case, &first).len(),
        1,
        "case {case:?}: expected the user's one connection to be terminated, got body {first}"
    );
    let before = stored_statuses(&engine);

    let (status, body) = terminate(
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.endpoint_id),
            p("Username", QUIET_USER),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 on the second call, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidClientVpnConnection.UserNotFound",
        "case {case:?}: got body {body}"
    );
    assert_eq!(
        stored_statuses(&engine),
        before,
        "case {case:?}: the refused second call must leave every connection as it was"
    );
}

// ---------------------------------------------------------------------------
// The move the clock owes
// ---------------------------------------------------------------------------

/// RULE: `terminating` is transient — the clock owes the connection `terminated` a transition
/// span later, with nothing called in between to make the move happen — and terminating a
/// connection that is no longer active reports it UNCHANGED rather than moving it back.
#[tokio::test]
async fn a_terminating_connection_reaches_terminated_and_stays_there() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let case = "a connection waited out";
    let connection_id = seeded.connections[0].1.clone();

    let body = served(
        case,
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.endpoint_id),
            p("ConnectionId", &connection_id),
        ],
    )
    .await;
    assert_eq!(
        reported(case, &body)
            .first()
            .map(|status| status.current.clone())
            .unwrap_or_default(),
        CLIENT_VPN_CONNECTION_STATUS_TERMINATING,
        "case {case:?}: expected the call to answer the transient status, got body {body}"
    );
    let ended_at = stored_end_time(&engine, &connection_id);
    assert!(
        ended_at.is_some(),
        "case {case:?}: expected the store to record when {connection_id} ended"
    );

    let settled =
        poll_until_past_terminating(case, &router, &seeded.endpoint_id, &connection_id).await;
    assert_eq!(
        settled, CLIENT_VPN_CONNECTION_STATUS_TERMINATED,
        "case {case:?}: expected the poll to end at \
         {CLIENT_VPN_CONNECTION_STATUS_TERMINATED}, got {settled:?}"
    );
    assert_eq!(
        stored_status(&engine, &connection_id).as_deref(),
        Some(CLIENT_VPN_CONNECTION_STATUS_TERMINATED),
        "case {case:?}: expected the store to hold the terminal status the answer reported"
    );

    // A further terminate of the same connection reports it unchanged: both statuses are the
    // terminal one, and the instant it ended is the one the first call recorded.
    let repeat = served(
        case,
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.endpoint_id),
            p("ConnectionId", &connection_id),
        ],
    )
    .await;
    assert_eq!(
        reported(case, &repeat),
        vec![Reported {
            connection_id: connection_id.clone(),
            previous: CLIENT_VPN_CONNECTION_STATUS_TERMINATED.to_string(),
            current: CLIENT_VPN_CONNECTION_STATUS_TERMINATED.to_string(),
        }],
        "case {case:?}: expected a terminated connection to be reported unchanged, \
         got body {repeat}"
    );
    assert_eq!(
        stored_end_time(&engine, &connection_id),
        ended_at,
        "case {case:?}: expected the recorded end of {connection_id} to be left alone"
    );
}

// ---------------------------------------------------------------------------
// What the operation refuses
// ---------------------------------------------------------------------------

/// A well-formed Client VPN endpoint id this account holds no endpoint for — the form the
/// recorded case names.
const ABSENT_ENDPOINT: &str = "cvpn-endpoint-0123456789abcdef0";

/// RULE: a request the store cannot serve is refused with the code and the wording EC2
/// answers for that reading of it, and writes nothing.
#[tokio::test]
async fn an_unservable_request_is_refused_and_writes_nothing() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let endpoint = seeded.endpoint_id.as_str();
    let connection = seeded.connections[0].1.as_str();
    let before = stored_statuses(&engine);

    // (case, the request's members, the code and the message the refusal carries)
    let rows: Vec<(&str, Vec<Param>, &str, String)> = vec![
        (
            "an endpoint id the request sent empty, which names no endpoint",
            vec![p("ClientVpnEndpointId", ""), p("ConnectionId", connection)],
            "MissingParameter",
            "The request must contain the parameter clientVpnEndpointId".to_string(),
        ),
        (
            "an endpoint id the account holds no endpoint for",
            vec![
                p("ClientVpnEndpointId", ABSENT_ENDPOINT),
                p("ConnectionId", connection),
            ],
            "InvalidClientVpnEndpointId.NotFound",
            format!("Endpoint {ABSENT_ENDPOINT} does not exist"),
        ),
        (
            "a value that is not an endpoint id at all",
            vec![
                p("ClientVpnEndpointId", "not-an-endpoint"),
                p("Username", BUSY_USER),
            ],
            "InvalidClientVpnEndpointId.NotFound",
            "Endpoint not-an-endpoint does not exist".to_string(),
        ),
        (
            "neither a connection nor a user",
            vec![p("ClientVpnEndpointId", endpoint)],
            "MissingParameter",
            "The request must contain the parameter connectionId or username".to_string(),
        ),
        (
            "an empty connection id, which names no connection",
            vec![p("ClientVpnEndpointId", endpoint), p("ConnectionId", "")],
            "MissingParameter",
            "The request must contain the parameter connectionId or username".to_string(),
        ),
        (
            "both a connection and a user",
            vec![
                p("ClientVpnEndpointId", endpoint),
                p("ConnectionId", connection),
                p("Username", BUSY_USER),
            ],
            "InvalidParameterCombination",
            "The parameter connectionId cannot be used with the parameter username".to_string(),
        ),
        (
            "a connection id the endpoint holds no connection for",
            vec![
                p("ClientVpnEndpointId", endpoint),
                p("ConnectionId", "cvpn-connection-0fedcba987654321f"),
            ],
            "InvalidClientVpnConnection.IdNotFound",
            "Connection cvpn-connection-0fedcba987654321f does not exist".to_string(),
        ),
        (
            "a value that is not a connection id at all",
            vec![
                p("ClientVpnEndpointId", endpoint),
                p("ConnectionId", "not-a-connection"),
            ],
            "InvalidClientVpnConnection.IdNotFound",
            "Connection not-a-connection does not exist".to_string(),
        ),
        (
            "a user the endpoint has never seen",
            vec![p("ClientVpnEndpointId", endpoint), p("Username", UNKNOWN_USER)],
            "InvalidClientVpnConnection.UserNotFound",
            format!("User {UNKNOWN_USER} does not exist"),
        ),
        (
            "a user whose name is the id of a connection, which is no user",
            vec![p("ClientVpnEndpointId", endpoint), p("Username", connection)],
            "InvalidClientVpnConnection.UserNotFound",
            format!("User {connection} does not exist"),
        ),
    ];

    for (case, params, code, message) in rows {
        let (status, body) = terminate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected EC2's own wording, got body {body}"
        );
        assert_eq!(
            stored_statuses(&engine),
            before,
            "case {case:?}: a refused terminate must leave every connection as it was"
        );
    }
}

/// RULE: `DryRun` "checks whether you have the required permissions for the action, without
/// actually making the request" — so a readable dry run is answered `DryRunOperation` and
/// terminates nothing, whichever selector it carries and whether or not that selector names
/// anything this account holds.
#[tokio::test]
async fn a_dry_run_terminates_nothing() {
    let (router, engine) = emulator();
    let seeded = seed(&engine);
    let before = stored_statuses(&engine);

    // Rows differing along which selector the flag rides on and whether it resolves.
    let rows: Vec<(&str, Vec<Param>)> = vec![
        (
            "a dry run naming a connection the endpoint holds",
            vec![
                p("ClientVpnEndpointId", &seeded.endpoint_id),
                p("ConnectionId", &seeded.connections[0].1),
            ],
        ),
        (
            "a dry run naming a user holding connections",
            vec![
                p("ClientVpnEndpointId", &seeded.endpoint_id),
                p("Username", BUSY_USER),
            ],
        ),
        (
            "a dry run naming an endpoint the account does not hold",
            vec![
                p("ClientVpnEndpointId", ABSENT_ENDPOINT),
                p("Username", BUSY_USER),
            ],
        ),
    ];

    for (case, members) in rows {
        let mut params = members.clone();
        params.push(p("DryRun", "true"));
        let (status, body) = terminate(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_MESSAGE,
            "case {case:?}: expected EC2's own dry-run wording, got body {body}"
        );
        assert_eq!(
            stored_statuses(&engine),
            before,
            "case {case:?}: a dry run must leave every connection as it was"
        );
    }

    // `DryRun=false` is not a dry run: the same request is served and does terminate.
    let case = "the same request without the flag";
    let body = served(
        case,
        &router,
        &[
            p("ClientVpnEndpointId", &seeded.endpoint_id),
            p("ConnectionId", &seeded.connections[0].1),
            p("DryRun", "false"),
        ],
    )
    .await;
    assert_eq!(
        reported(case, &body)
            .first()
            .map(|status| status.current.clone())
            .unwrap_or_default(),
        CLIENT_VPN_CONNECTION_STATUS_TERMINATING,
        "case {case:?}: expected the connection to be terminated, got body {body}"
    );
}
