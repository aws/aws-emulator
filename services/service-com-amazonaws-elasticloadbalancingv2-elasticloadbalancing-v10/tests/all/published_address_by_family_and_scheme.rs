// SPDX-License-Identifier: CC0-1.0

//! The address a load balancer PUBLISHES, dialled as a client that reads it does — for every
//! family and scheme whose listeners carry a client's own traffic — and the CloudWatch series
//! the request answered there is metered into.
//!
//! The rule under test: **`LoadBalancer$DNSName` is "the public DNS name of the load
//! balancer", the address a client is handed and sends its traffic to, so for every load
//! balancer whose listeners carry that traffic — the Application family's, which answer the
//! client's own request, and the Network family's, which hand the connection they accept to
//! what their target group selects — of EITHER scheme, the published value is an address a
//! client on THIS machine dials and is served at: a connection to it is accepted, the request
//! sent over it is answered out of that load balancer's own stored configuration, and the
//! answer is metered under that load balancer's own dimension.**
//!
//! `Scheme` decides which clients may reach a load balancer in a real network — an
//! internet-facing one's "nodes … have public IP addresses" while an internal one's "have only
//! private IP addresses", so "internal load balancers can route requests only from clients with
//! access to the VPC for the load balancer" — and in this emulator every such client reaches a
//! load balancer at the one address this emulator serves it on, so neither scheme publishes an
//! address a client cannot reach.
//!
//! The FORM each family publishes, that a later read reports the value the create answered, and
//! that no two load balancers publish the same one are `load_balancer_dns_name_form`'s table,
//! which spans two Regions and the Gateway family as well; what this suite holds is that the
//! value is SERVED and METERED.
//!
//! The rows span what the rule generalises over: both families whose listeners carry a client's
//! traffic, against both schemes `LoadBalancerSchemeEnum` names, with the request driven over a
//! REAL connection to the published value in each case. What each family's address must show
//! differs with what its listeners DO: an Application listener answers the client's request
//! itself, so its rows assert the status and the body its own configuration holds; a Network
//! listener hands the connection on to what its target group selects, so its rows assert that
//! the published address accepts the connection rather than an answer that would be no
//! listener's. A Gateway Load Balancer is not a row: its `GENEVE` listeners carry no request of
//! this kind and it gets no socket at all, which is `load_balancer_dns_name_form`'s table.
//!
//! The metering half is the same request's: `HTTP_Fixed_Response_Count` is Elastic Load
//! Balancing's published count of the requests an Application Load Balancer answered with a
//! `fixed-response` action, dimensioned by `LoadBalancer`, whose value is the `app/<name>/<id>`
//! resource path of that load balancer's own ARN — so a request a client sends to the published
//! address is observable in the AWS/ApplicationELB namespace, under the load balancer it was
//! sent to and under no other.
//!
//! The dial (`alb_desync_mitigation_and_forwarded_for::raw_exchange`), the move to `active`
//! (`alb_data_plane::settle`), the metric read (`alb_request_metrics::series_sum`) and the wire
//! harness (`provisioning_loop`) are the crate's own; nothing here restates them.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::settle;
use crate::alb_desync_mitigation_and_forwarded_for::raw_exchange;
use crate::alb_request_metrics::{load_balancer_dimension, series_sum, LOAD_BALANCER_DIMENSION};
use crate::provisioning_loop::{action, call, emulator, p};

/// The series one `fixed-response` answer is counted into — Elastic Load Balancing's published
/// count of the requests an Application Load Balancer answered with that action.
const FIXED_RESPONSE_COUNT: &str = "HTTP_Fixed_Response_Count";

/// The path every row asks for, which no rule matches: the listener's own default action is
/// what answers it, so the answer is the load balancer's own configuration and nothing else.
const ASKED_FOR: &str = "/warm/ok";

/// How long a dial or a datapoint is waited for. Generous on purpose: everything waited for
/// happens in this process, and the deadline is only here so a failure is a failure rather than
/// a hang — the loop returns the moment the datapoint is there, so a loaded host cannot decide
/// a verdict.
const BUDGET: Duration = Duration::from_secs(60);

/// How long the loop waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(10);

/// The two families whose listeners carry the traffic a client sends, which is what decides
/// both what the published address must show and how an ARN and a CloudWatch dimension name the
/// load balancer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    /// Listeners that answer the client's own request.
    Application,
    /// Listeners that hand the connection they accept to what their target group selects.
    Network,
}

impl Family {
    /// The `Type` a create names this family by (`LoadBalancerTypeEnum`).
    fn lb_type(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Network => "network",
        }
    }

    /// The listener protocol this family carries a client's traffic over.
    fn listener_protocol(self) -> &'static str {
        match self {
            Self::Application => "HTTP",
            Self::Network => "TCP",
        }
    }
}

/// One row: the case, the load balancer's `Name`, its family and scheme, and how many subnets
/// it is placed in.
struct Row {
    case: &'static str,
    name: &'static str,
    family: Family,
    scheme: &'static str,
    subnets: usize,
}

/// Both families against both schemes.
const ROWS: &[Row] = &[
    Row {
        case: "an internet-facing application load balancer",
        name: "published-app-public",
        family: Family::Application,
        scheme: "internet-facing",
        subnets: 2,
    },
    Row {
        case: "an internal application load balancer",
        name: "published-app-private",
        family: Family::Application,
        scheme: "internal",
        subnets: 2,
    },
    Row {
        case: "an internet-facing network load balancer",
        name: "published-net-public",
        family: Family::Network,
        scheme: "internet-facing",
        subnets: 1,
    },
    Row {
        case: "an internal network load balancer",
        name: "published-net-private",
        family: Family::Network,
        scheme: "internal",
        subnets: 3,
    },
];

/// The body the Application rows' listener answers with, which is its own: a row dialling one
/// address must be answered by the load balancer it dialled and by no other.
fn body_of(row: &Row) -> String {
    format!("answered-by-{}", row.name)
}

/// What one row's load balancer published: the ARN it was minted with and the address it
/// reports as its `DNSName`.
struct Published {
    arn: String,
    address: String,
}

/// Create the load balancer `row` describes, give it the listener its family carries traffic
/// over, and place the move to `active` the clock owes it.
async fn deploy(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> Published {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", row.name));
    form.push(p("Type", row.family.lb_type()));
    form.push(p("Scheme", row.scheme));
    for index in 1..=row.subnets {
        form.push(p(
            &format!("Subnets.member.{index}"),
            &format!("subnet-{index}{index}{index}{index}{index}{index}{index}{index}"),
        ));
    }
    let created = call(router, &form).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the create to be served, got {} body {}",
        row.case, created.status, created.body
    );
    let arn = created.text("LoadBalancerArn").unwrap_or_else(|| {
        panic!(
            "case {:?}: no LoadBalancerArn in {}",
            row.case, created.body
        )
    });
    let address = created
        .text("DNSName")
        .unwrap_or_else(|| panic!("case {:?}: no DNSName in {}", row.case, created.body));

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &arn));
    form.push(p("Protocol", row.family.listener_protocol()));
    form.push(p("Port", "80"));
    match row.family {
        // An Application listener answers the client's own request, so its default action is
        // the answer this row asserts.
        Family::Application => {
            form.push(p("DefaultActions.member.1.Type", "fixed-response"));
            form.push(p(
                "DefaultActions.member.1.FixedResponseConfig.StatusCode",
                "200",
            ));
            form.push(p(
                "DefaultActions.member.1.FixedResponseConfig.MessageBody",
                &body_of(row),
            ));
            form.push(p(
                "DefaultActions.member.1.FixedResponseConfig.ContentType",
                "text/plain",
            ));
        }
        // A Network listener hands the connection on to what its target group selects, so it
        // carries the `forward` that family's listeners are configured with.
        Family::Network => {
            let target_group = tcp_target_group(router, row).await;
            form.push(p("DefaultActions.member.1.Type", "forward"));
            form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
        }
    }
    let listener = call(router, &form).await;
    assert_eq!(
        listener.status, 200,
        "case {:?}: expected CreateListener to be served, got {} body {}",
        row.case, listener.status, listener.body
    );

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(engine, &arn, SystemTime::now() - Duration::from_secs(1));

    Published { arn, address }
}

/// The TCP target group a Network row's listener forwards to.
async fn tcp_target_group(router: &MultiServiceRouter, row: &Row) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.name));
    form.push(p("Protocol", "TCP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-12345678"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected CreateTargetGroup to be served, got {} body {}",
        row.case, reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body))
}

/// The published value as an address a client dials — or an explanation of how it is not one.
fn dialable(row: &Row, address: &str) -> SocketAddr {
    let dialable: SocketAddr = address.parse().unwrap_or_else(|error| {
        panic!(
            "case {:?}: expected an address of this emulator a client can dial, got \
             {address:?} ({error})",
            row.case
        )
    });
    assert!(
        dialable.ip().is_loopback(),
        "case {:?}: expected an address on an interface of this machine, got {address:?}",
        row.case
    );
    assert_ne!(
        dialable.port(),
        0,
        "case {:?}: expected the port the socket answers on, got {address:?}",
        row.case
    );
    dialable
}

/// Open a connection to the port of `address` and assert it is accepted: the published value is
/// an address this emulator SERVES, whatever the listeners behind it then do with the
/// connection.
///
/// Only the PORT is taken from the published value, exactly as the crate's own exchange does:
/// the core binds its listeners on this machine, so the dial is to loopback.
async fn accepts_a_connection(row: &Row, address: SocketAddr) {
    let dialed = tokio::time::timeout(
        BUDGET,
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, address.port())),
    )
    .await;
    assert!(
        matches!(dialed, Ok(Ok(_))),
        "case {:?}: expected {address} to accept a connection, got {dialed:?}",
        row.case
    );
}

/// The `Sum` of the fixed-response series under one load balancer's dimension, looked at again
/// until it reaches `wanted` or the budget ends — the datapoint is written while the answer is
/// being sent, so what a reader waits for is the value and not a span of time.
async fn fixed_responses_reaching(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    dimension: &str,
    wanted: f64,
) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let counted = series_sum(
            engine,
            FIXED_RESPONSE_COUNT,
            &[(LOAD_BALANCER_DIMENSION, dimension.to_string())],
        );
        if (counted - wanted).abs() < f64::EPSILON || Instant::now() >= deadline {
            assert!(
                (counted - wanted).abs() < f64::EPSILON,
                "case {case:?}: expected {FIXED_RESPONSE_COUNT} {wanted} under \
                 {LOAD_BALANCER_DIMENSION} {dimension:?}, got {counted}"
            );
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: the value a load balancer publishes as its `DNSName` is an address of THIS emulator —
/// for both families whose listeners carry a client's traffic and for both schemes — served at
/// that very address: a connection to it is accepted, the request over it is answered out of
/// that load balancer's own configuration, and the answer is metered under that load balancer's
/// own CloudWatch dimension and no other.
#[tokio::test]
async fn the_address_each_family_and_scheme_publishes_is_served_and_metered_by_this_emulator() {
    let (router, engine) = emulator();

    let mut deployed: Vec<Published> = Vec::new();
    for row in ROWS {
        deployed.push(deploy(&router, &engine, row).await);
    }

    // Nothing has been sent yet, so no load balancer has answered anything to count.
    for (row, published) in ROWS.iter().zip(deployed.iter()) {
        fixed_responses_reaching(
            &engine,
            row.case,
            &load_balancer_dimension(&published.arn),
            0.0,
        )
        .await;
    }

    // The request itself, over a real connection to the address each load balancer published.
    for (row, published) in ROWS.iter().zip(deployed.iter()) {
        let address = dialable(row, &published.address);
        match row.family {
            Family::Application => {
                let request = format!(
                    "GET {ASKED_FOR} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    published.address
                );
                let (status, raw) = raw_exchange(row.case, &published.address, &request).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the address it published to answer the listener's own \
                     action, got {status} in {raw:?}",
                    row.case
                );
                let answered = raw
                    .split_once("\r\n\r\n")
                    .map(|(_head, body)| body.to_string())
                    .unwrap_or_else(|| {
                        panic!("case {:?}: no head and body in {raw:?}", row.case)
                    });
                assert_eq!(
                    answered,
                    body_of(row),
                    "case {:?}: expected the body this load balancer's own listener holds, got \
                     {answered:?}",
                    row.case
                );
            }
            Family::Network => accepts_a_connection(row, address).await,
        }
    }

    // One request answered at the published address is one datapoint under that load
    // balancer's own dimension — and under no other, which is what pins the attribution: a
    // Network Load Balancer answered no request of its own, so it counts none.
    for (row, published) in ROWS.iter().zip(deployed.iter()) {
        let wanted = match row.family {
            Family::Application => 1.0,
            Family::Network => 0.0,
        };
        fixed_responses_reaching(
            &engine,
            row.case,
            &load_balancer_dimension(&published.arn),
            wanted,
        )
        .await;
    }

    // A second request to ONE of the addresses moves that load balancer's count and leaves
    // every other load balancer's where it was.
    let (row, published) = ROWS
        .iter()
        .zip(deployed.iter())
        .find(|(row, _)| row.family == Family::Application)
        .expect("the table holds an application load balancer");
    let request = format!(
        "GET {ASKED_FOR} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        published.address
    );
    let (status, raw) = raw_exchange(row.case, &published.address, &request).await;
    assert_eq!(
        status, 200,
        "case {:?} dialled a second time: expected 200, got {status} in {raw:?}",
        row.case
    );
    for (other, other_published) in ROWS.iter().zip(deployed.iter()) {
        let wanted = match (other.family, other.name == row.name) {
            (Family::Application, true) => 2.0,
            (Family::Application, false) => 1.0,
            (Family::Network, _) => 0.0,
        };
        fixed_responses_reaching(
            &engine,
            other.case,
            &load_balancer_dimension(&other_published.arn),
            wanted,
        )
        .await;
    }
}
