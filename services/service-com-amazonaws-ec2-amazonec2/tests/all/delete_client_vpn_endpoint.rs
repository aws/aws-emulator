// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteClientVpnEndpoint`: the operation that begins taking ONE
//! stored Client VPN endpoint down, read back through `DescribeClientVpnEndpoints`.
//!
//! Driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A delete begins taking down exactly the endpoint the request names, whatever state that
//!    endpoint was reached in, answers it the state it is NOW in — `deleting`, which the clock
//!    owes `deleted` — and stamps it with the instant it was deleted at.
//! 2. An endpoint already on its way out keeps the state it is in, so a repeated delete
//!    reports that state rather than restarting the move or recording a second deletion
//!    instant.
//! 3. A request whose own form cannot be served is refused before anything is resolved or
//!    written: one that carries no endpoint id is `MissingParameter` in the ONE sentence this
//!    surface words such a refusal, whichever side of dispatch resolved the absence — the
//!    request boundary for a parameter the request omitted, the handler for one it sent empty
//!    — one carrying `DryRun` is answered the dry-run refusal, a question about the CALLER, so
//!    it is the same answer whatever endpoint the request names, and an id this account holds
//!    no endpoint for is refused by lookup rather than reported as deleted. Every endpoint is
//!    left as it was.
//! 4. The delete is scoped to the one endpoint it names: every other endpoint of the account
//!    keeps the state it held and carries no deletion instant.
//!
//! Every table row below is a distinct input along what those rules generalise over: the
//! state the endpoint is in when the delete reaches it, how many times it is deleted, which
//! endpoint of several is named, and — for the refusals — which part of the request cannot be
//! served.

use std::time::{Duration, Instant};

use ec2::ops::types::{ClientVpnEndpoint, ClientVpnEndpointStatus};
use ec2::state::{
    missing_required_parameter_message, ClientVpnTargetNetworkSpec as TargetNetworkSpec,
    ComAmazonawsEc2Amazonec2State as Ec2State, CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE,
    CLIENT_VPN_ENDPOINT_STATUS_DELETED, CLIENT_VPN_ENDPOINT_STATUS_DELETING,
    CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE, DRY_RUN_OPERATION, DRY_RUN_OPERATION_MESSAGE,
    DRY_RUN_OPERATION_STATUS, INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND, MISSING_PARAMETER,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, error_message};

/// The Region every call here is signed for. The Client VPN stores are per account rather
/// than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The actions this suite drives.
const DELETE: &str = "DeleteClientVpnEndpoint";
const DESCRIBE: &str = "DescribeClientVpnEndpoints";

/// The fifth state `ClientVpnEndpointStatusCode` declares —
/// `smithy_query(query="shape",shape="ClientVpnEndpointStatusCode")` lists `pending-associate
/// | available | deleting | deleted | pending`, and `pending` is the one this crate keeps no
/// constant for: "the Client VPN endpoint has been created with a Transit Gateway
/// configuration and is waiting for the Transit Gateway attachment to be accepted"
/// (`ClientVpnEndpointStatus$Code`).
const CLIENT_VPN_ENDPOINT_STATUS_PENDING: &str = "pending";

/// A well-formed endpoint id this account holds no endpoint for, and a value that is no
/// endpoint id at all: what the two lookup refusal rows name.
const ABSENT_ENDPOINT: &str = "cvpn-endpoint-00000000000000009";
const UNREADABLE_ENDPOINT: &str = "not-a-client-vpn-endpoint";

/// The member a request that names no endpoint left out, as the model declares it — what the
/// sentence EC2 answers such a request names.
const ENDPOINT_ID_MEMBER: &str = "ClientVpnEndpointId";

/// The deadline every poll below gives up at, and how often it looks again. Generous: what is
/// waited for is the CONDITION a clock-owed move settles into, so nothing here depends on how
/// fast the host runs — all the deadline bounds is a hang, and the interval is short enough
/// that it costs many looks rather than one.
const POLL_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// One form field.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One call of `action` carrying `params`, as `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// Write one Client VPN endpoint in the state `status` names, the way `CreateClientVpnEndpoint`
/// and the moves that follow it would leave one, and answer the id it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine, description: &str, status: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            vpc_id: Some("vpc-00000000000000001".to_string()),
            status: Some(ClientVpnEndpointStatus {
                code: Some(status.to_string()),
                message: None,
            }),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// Associate one target network with `endpoint_id`, the way `AssociateClientVpnTargetNetwork`
/// would — what makes the endpoint one a delete cannot be served for.
fn seed_target_network(engine: &engine::InMemoryEngine, endpoint_id: &str) {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_target_network(TargetNetworkSpec {
            client_vpn_endpoint_id: endpoint_id.to_string(),
            target_network_id: Some("subnet-00000000000000001".to_string()),
            vpc_id: Some("vpc-00000000000000001".to_string()),
            ..Default::default()
        })
        .expect("the store must take the target network association");
}

/// The body `DescribeClientVpnEndpoints` answers for exactly the endpoint `endpoint_id`, as
/// that call's own 200.
async fn described(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    let (status, body) = call(router, DESCRIBE, &[p("ClientVpnEndpointId.1", endpoint_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the endpoint {endpoint_id} must still be reported, got {status} with \
         body {body}"
    );
    body
}

/// The state and the deletion instant `DescribeClientVpnEndpoints` reports for one endpoint.
async fn reported_state(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> (Option<String>, Option<String>) {
    let body = described(case, router, endpoint_id).await;
    (element(&body, "code"), element(&body, "deletionTime"))
}

/// Look at the endpoint AGAIN until the describe reports `wanted`, or a deadline far past the
/// service's own transition span passes. Answers the state last seen.
async fn poll_until_status(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
    wanted: &str,
) -> Option<String> {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (held, _) = reported_state(case, router, endpoint_id).await;
        if held.as_deref() == Some(wanted) || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Rule 1: whatever state an endpoint is reached in, the delete answers it `deleting`, the
/// describe reports it `deleting` too, and it carries the instant it was deleted at.
#[tokio::test]
async fn a_delete_begins_taking_down_the_endpoint_whatever_state_it_was_in() {
    // Every state an endpoint can be reached in BEFORE one takes it down, from the states
    // `ClientVpnEndpointStatusCode` declares: the two a VPC-based endpoint moves through and
    // the one a Transit Gateway-based endpoint waits in.
    let states = [
        (
            "an endpoint with no target network associated yet",
            CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE,
        ),
        (
            "an endpoint whose target network has been associated",
            CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE,
        ),
        (
            "an endpoint waiting for a Transit Gateway attachment",
            CLIENT_VPN_ENDPOINT_STATUS_PENDING,
        ),
    ];

    for (case, held) in states {
        let (router, engine) = super::ec2_query::emulator();
        let endpoint = seed_endpoint(&engine, case, held);

        let (status, body) = call(&router, DELETE, &[p("ClientVpnEndpointId", &endpoint)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete is served, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(CLIENT_VPN_ENDPOINT_STATUS_DELETING),
            "case {case:?}: expected the delete to answer {CLIENT_VPN_ENDPOINT_STATUS_DELETING:?}, \
             got {body}"
        );

        let (reported, deleted_at) = reported_state(case, &router, &endpoint).await;
        assert_eq!(
            reported.as_deref(),
            Some(CLIENT_VPN_ENDPOINT_STATUS_DELETING),
            "case {case:?}: expected the describe to report \
             {CLIENT_VPN_ENDPOINT_STATUS_DELETING:?}, got {reported:?}"
        );
        let deleted_at = deleted_at.unwrap_or_default();
        assert!(
            as_timestamp(&deleted_at).is_some(),
            "case {case:?}: expected an ISO 8601 deletionTime on the endpoint, got \
             {deleted_at:?}"
        );
    }
}

/// Rule 2: a repeated delete reports the state the endpoint is in — `deleting` while it is on
/// its way out, `deleted` once the clock has settled it — and the deletion instant is stamped
/// once.
#[tokio::test]
async fn a_repeated_delete_reports_the_state_the_endpoint_is_in() {
    let case = "an endpoint deleted three times";
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, case, CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE);

    let (status, body) = call(&router, DELETE, &[p("ClientVpnEndpointId", &endpoint)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first delete is served, got {status} with body {body}"
    );
    let (_, first_instant) = reported_state(case, &router, &endpoint).await;
    let first_instant = first_instant.unwrap_or_default();

    // The SECOND delete, with the endpoint still on its way out: the state it is in, and the
    // same deletion instant the first one stamped.
    let (status, body) = call(&router, DELETE, &[p("ClientVpnEndpointId", &endpoint)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the repeated delete is served, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some(CLIENT_VPN_ENDPOINT_STATUS_DELETING),
        "case {case:?}: expected the repeat to report {CLIENT_VPN_ENDPOINT_STATUS_DELETING:?}, \
         got {body}"
    );
    let (_, repeated_instant) = reported_state(case, &router, &endpoint).await;
    assert_eq!(
        repeated_instant.as_deref(),
        Some(first_instant.as_str()),
        "case {case:?}: expected the deletion instant {first_instant:?} to be stamped once, got \
         {repeated_instant:?}"
    );

    // The move the clock owes the endpoint, looked for until it holds — and then a THIRD
    // delete, which reports that terminal state rather than moving it back.
    let settled = poll_until_status(case, &router, &endpoint, CLIENT_VPN_ENDPOINT_STATUS_DELETED)
        .await;
    assert_eq!(
        settled.as_deref(),
        Some(CLIENT_VPN_ENDPOINT_STATUS_DELETED),
        "case {case:?}: the clock owes the endpoint {CLIENT_VPN_ENDPOINT_STATUS_DELETED:?}, got \
         {settled:?}"
    );
    let (status, body) = call(&router, DELETE, &[p("ClientVpnEndpointId", &endpoint)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting a deleted endpoint is served, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some(CLIENT_VPN_ENDPOINT_STATUS_DELETED),
        "case {case:?}: expected the delete of a deleted endpoint to report \
         {CLIENT_VPN_ENDPOINT_STATUS_DELETED:?}, got {body}"
    );
    let (_, terminal_instant) = reported_state(case, &router, &endpoint).await;
    assert_eq!(
        terminal_instant.as_deref(),
        Some(first_instant.as_str()),
        "case {case:?}: expected the deletion instant {first_instant:?} to survive the settled \
         move, got {terminal_instant:?}"
    );
}

/// One refusal row: what cannot be served, the form fields, and the status, code and message
/// EC2 answers it.
struct RefusalRow {
    case: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: String,
    message: String,
}

/// Rule 3: a request whose own form cannot be served is refused, and nothing about any
/// endpoint changes.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let free = seed_endpoint(
        &engine,
        "the endpoint with no target network",
        CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE,
    );
    let attached = seed_endpoint(
        &engine,
        "the endpoint with a target network",
        CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE,
    );
    seed_target_network(&engine, &attached);

    let rows = vec![
        RefusalRow {
            case: "no endpoint id at all",
            params: Vec::new(),
            status: 400,
            code: MISSING_PARAMETER.to_string(),
            message: missing_required_parameter_message(ENDPOINT_ID_MEMBER),
        },
        RefusalRow {
            case: "an endpoint id the request sent empty",
            params: vec![p("ClientVpnEndpointId", "")],
            status: 400,
            code: MISSING_PARAMETER.to_string(),
            message: missing_required_parameter_message(ENDPOINT_ID_MEMBER),
        },
        RefusalRow {
            case: "a dry run of a delete that would be served",
            params: vec![p("ClientVpnEndpointId", &free), p("DryRun", "true")],
            status: DRY_RUN_OPERATION_STATUS,
            code: DRY_RUN_OPERATION.to_string(),
            message: DRY_RUN_OPERATION_MESSAGE.to_string(),
        },
        RefusalRow {
            case: "a dry run of a delete a target network forbids",
            params: vec![p("ClientVpnEndpointId", &attached), p("DryRun", "true")],
            status: DRY_RUN_OPERATION_STATUS,
            code: DRY_RUN_OPERATION.to_string(),
            message: DRY_RUN_OPERATION_MESSAGE.to_string(),
        },
        RefusalRow {
            case: "a dry run naming an endpoint the account does not hold",
            params: vec![
                p("ClientVpnEndpointId", ABSENT_ENDPOINT),
                p("DryRun", "true"),
            ],
            status: DRY_RUN_OPERATION_STATUS,
            code: DRY_RUN_OPERATION.to_string(),
            message: DRY_RUN_OPERATION_MESSAGE.to_string(),
        },
        RefusalRow {
            case: "an endpoint id the account holds no endpoint for",
            params: vec![p("ClientVpnEndpointId", ABSENT_ENDPOINT)],
            status: 400,
            code: INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND.to_string(),
            message: format!("Endpoint {ABSENT_ENDPOINT} does not exist"),
        },
        RefusalRow {
            case: "a value that is no endpoint id at all",
            params: vec![p("ClientVpnEndpointId", UNREADABLE_ENDPOINT)],
            status: 400,
            code: INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND.to_string(),
            message: format!("Endpoint {UNREADABLE_ENDPOINT} does not exist"),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = call(&router, DELETE, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = row.status
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.code,
            "case {case:?}: expected the code {expected:?}, got {code:?} in {body}",
            expected = row.code
        );
        let message = error_message(&body);
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the message {expected:?}, got {message:?}",
            expected = row.message
        );

        // Nothing the refusal touched: each endpoint still holds the state it was seeded in
        // and carries no deletion instant.
        for (endpoint, held) in [
            (&free, CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE),
            (&attached, CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE),
        ] {
            let (reported, deleted_at) = reported_state(case, &router, endpoint).await;
            assert_eq!(
                reported.as_deref(),
                Some(held),
                "case {case:?}: expected {endpoint} to still hold {held:?}, got {reported:?}"
            );
            assert_eq!(
                deleted_at, None,
                "case {case:?}: expected {endpoint} to carry no deletionTime, got {deleted_at:?}"
            );
        }
    }
}

/// Rule 4: the delete takes down the ONE endpoint it names, whichever of the account's
/// endpoints that is, and leaves every other one as it was.
#[tokio::test]
async fn a_delete_takes_down_only_the_endpoint_it_names() {
    // The same three endpoints each time, with the delete naming a different one of them: the
    // first, one in the middle, and the last.
    let seeded = [
        (
            "the first endpoint",
            CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE,
        ),
        ("the middle endpoint", CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE),
        ("the last endpoint", CLIENT_VPN_ENDPOINT_STATUS_PENDING),
    ];

    for named in 0..seeded.len() {
        let case = seeded[named].0;
        let (router, engine) = super::ec2_query::emulator();
        let endpoints: Vec<String> = seeded
            .iter()
            .map(|(description, held)| seed_endpoint(&engine, description, held))
            .collect();

        let (status, body) = call(
            &router,
            DELETE,
            &[p("ClientVpnEndpointId", &endpoints[named])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete is served, got {status} with body {body}"
        );

        for (index, endpoint) in endpoints.iter().enumerate() {
            let (reported, deleted_at) = reported_state(case, &router, endpoint).await;
            if index == named {
                assert_eq!(
                    reported.as_deref(),
                    Some(CLIENT_VPN_ENDPOINT_STATUS_DELETING),
                    "case {case:?}: expected the named endpoint {endpoint} to report \
                     {CLIENT_VPN_ENDPOINT_STATUS_DELETING:?}, got {reported:?}"
                );
                assert!(
                    deleted_at.is_some(),
                    "case {case:?}: expected the named endpoint {endpoint} to carry a \
                     deletionTime, got {deleted_at:?}"
                );
                continue;
            }
            let held = seeded[index].1;
            assert_eq!(
                reported.as_deref(),
                Some(held),
                "case {case:?}: expected the endpoint {endpoint} nobody named to still hold \
                 {held:?}, got {reported:?}"
            );
            assert_eq!(
                deleted_at, None,
                "case {case:?}: expected the endpoint {endpoint} nobody named to carry no \
                 deletionTime, got {deleted_at:?}"
            );
        }
    }
}
