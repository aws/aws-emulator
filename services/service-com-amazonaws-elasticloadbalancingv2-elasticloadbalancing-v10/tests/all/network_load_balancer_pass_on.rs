// SPDX-License-Identifier: CC0-1.0

//! A NETWORK Load Balancer's own traffic, end to end over the wire: what a client that
//! sends a request to the name such a load balancer publishes gets back.
//!
//! The rule under test: **a load balancer answers a request addressed to the name it
//! publishes out of the listener the request reached, and a listener that HANDS THE
//! CONNECTION ON — a `forward` to an `alb` target group, "alb - Register a single
//! Application Load Balancer as a target", whose registration holds that load balancer's
//! ARN — is answered by the load balancer the chosen registration names, out of ITS own
//! listener.** A Network Load Balancer's listeners carry the TCP protocols and no rules of
//! their own, so passing the connection on is all one does; what answers is therefore the
//! Application Load Balancer behind it, deciding the request on its own rules exactly as it
//! would for a client that addressed it directly.
//!
//! The rows span what the rule generalises over: a path a rule of the load balancer behind
//! the chain matches (two spellings of it) and one no rule matches, which its listener's
//! default action answers; a chain that leads back to a load balancer already in it and a
//! chain of distinct load balancers longer than the walk this plane follows, neither of
//! which is followed any further; and a registration naming a load balancer this service
//! does not hold, for which no connection can be made. Every status and body a row expects
//! is the Application Load Balancer's own, written through the control plane's own
//! operations and reachable by nothing else in the chain.
//!
//! The wire harness (`emulator`, `action`, `call`, `p`) comes from `provisioning_loop`, and
//! the data-plane request builder and the clock-owed placement from `alb_data_plane`;
//! nothing here restates them. The clock-owed moves (each load balancer's to `active`, each
//! registration's to `healthy`) are placed on either side of now in the store rather than
//! slept for, exactly as that suite places them.

use std::time::{Duration, SystemTime};

use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{send, settle, Answered};
use crate::provisioning_loop::{action, call, emulator, p};

/// The body and status the Application Load Balancer's RULE answers with, and the body and
/// status its listener's DEFAULT action answers with. Two different nonces: what proves the
/// answer came from the load balancer at the end of the chain is that the two never agree.
const MATCHED_BODY: &str = "matched-9c41";
const MATCHED_STATUS: u16 = 200;
const DEFAULT_BODY: &str = "default-2ab7";
const DEFAULT_STATUS: u16 = 404;

/// The path pattern the Application Load Balancer's rule is written for.
const MATCHED_PATTERN: &str = "/orders/*";

/// The status a `forward` whose target cannot be CONNECTED to earns, which is what a chain
/// the plane will not follow any further and a registration naming no load balancer of this
/// store both answer.
const UNREACHABLE_STATUS: u16 = 502;

/// A load-balancer ARN of the right FORM naming no load balancer this service holds: what a
/// registration left behind by a load balancer that is gone looks like.
const ABSENT_LOAD_BALANCER: &str =
    "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/gone/0123456789abcdef";

/// How many links a chain of DISTINCT load balancers is built with: one more than the walk
/// this plane follows, so the last hop of it is the one the bound refuses. The bound itself
/// is the data plane's own (`MAX_PASSED_ON_HOPS`), which is not published, so the row states
/// the length it needs rather than reading it — a bound raised past this length fails this
/// row instead of passing it quietly.
const PAST_THE_HOP_BOUND: usize = 5;

/// Create a load balancer of `lb_type` and answer `(ARN, published DNS name)`.
async fn create_load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    lb_type: &str,
) -> (String, String) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Scheme", "internal"));
    form.push(p("Type", lb_type));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {lb_type} load balancer {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let arn = reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body));
    let dns_name = reply
        .text("DNSName")
        .unwrap_or_else(|| panic!("case create {name}: no DNSName in {}", reply.body));
    (arn, dns_name)
}

/// Create an `alb` target group holding `target`, and answer its ARN. A target group of a
/// Network Load Balancer carries a TCP protocol, and an `alb` one takes "a single
/// Application Load Balancer" by ARN.
async fn create_alb_target_group(
    router: &MultiServiceRouter,
    name: &str,
    target: &str,
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("TargetType", "alb"));
    form.push(p("Protocol", "TCP"));
    form.push(p("Port", "80"));
    form.push(p("HealthCheckProtocol", "HTTP"));
    form.push(p("VpcId", "vpc-12345678"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create target group {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case create {name}: no TargetGroupArn in {}", reply.body));

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("Targets.member.1.Id", target));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case register {target} with {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    arn
}

/// Create the TCP listener of a Network Load Balancer whose default action forwards to
/// `tg_arn` — the listener that hands the connection on.
async fn create_tcp_listener(router: &MultiServiceRouter, lb_arn: &str, tg_arn: &str) {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "TCP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create the TCP listener of {lb_arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
}

/// Create the Application Load Balancer every row's chain ends at: one HTTP:80 listener
/// whose default action is the fixed response above, and one rule for [`MATCHED_PATTERN`].
async fn create_answering_application(router: &MultiServiceRouter) -> String {
    let (arn, _) = create_load_balancer(router, "pass-on-alb", "application").await;
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        &DEFAULT_STATUS.to_string(),
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.MessageBody",
        DEFAULT_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create the application listener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let listener = reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create the application listener: no ARN in {}", reply.body));

    let mut form = action("CreateRule");
    form.push(p("ListenerArn", &listener));
    form.push(p("Priority", "10"));
    form.push(p("Conditions.member.1.Field", "path-pattern"));
    form.push(p("Conditions.member.1.Values.member.1", MATCHED_PATTERN));
    form.push(p("Actions.member.1.Type", "fixed-response"));
    form.push(p(
        "Actions.member.1.FixedResponseConfig.StatusCode",
        &MATCHED_STATUS.to_string(),
    ));
    form.push(p(
        "Actions.member.1.FixedResponseConfig.MessageBody",
        MATCHED_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create the matching rule: expected 200, got {} body {}",
        reply.status, reply.body
    );
    arn
}

#[tokio::test]
async fn a_network_load_balancer_is_answered_by_the_load_balancer_it_forwards_to() {
    let (router, engine) = emulator();
    let application = create_answering_application(&router).await;

    // The chain under test: a Network Load Balancer whose TCP listener forwards to an `alb`
    // target group holding the Application Load Balancer above.
    let (network, network_dns) = create_load_balancer(&router, "pass-on-nlb", "network").await;
    let group = create_alb_target_group(&router, "pass-on-targets", &application).await;
    create_tcp_listener(&router, &network, &group).await;

    // A chain that leads back to a load balancer already in it: the Network Load Balancer's
    // own ARN registered with the group its own listener forwards to.
    let (cyclic, cyclic_dns) = create_load_balancer(&router, "pass-on-cycle", "network").await;
    let cyclic_group = create_alb_target_group(&router, "pass-on-cyclic", &cyclic).await;
    create_tcp_listener(&router, &cyclic, &cyclic_group).await;

    // A registration naming a load balancer this service does not hold.
    let (absent, absent_dns) = create_load_balancer(&router, "pass-on-absent", "network").await;
    let absent_group =
        create_alb_target_group(&router, "pass-on-missing", ABSENT_LOAD_BALANCER).await;
    create_tcp_listener(&router, &absent, &absent_group).await;

    // A chain of DISTINCT load balancers longer than the walk this plane follows: each one
    // passes the request to the next, so what ends it is the bound rather than a revisit.
    let mut chain: Vec<String> = Vec::new();
    let mut chain_dns = String::new();
    for link in 0..=PAST_THE_HOP_BOUND {
        let (arn, dns) =
            create_load_balancer(&router, &format!("pass-on-link-{link}"), "network").await;
        if link == 0 {
            chain_dns = dns;
        } else {
            // The PREVIOUS link forwards to this one, which is what makes the chain.
            let group = create_alb_target_group(
                &router,
                &format!("pass-on-link-tg-{link}"),
                &arn,
            )
            .await;
            let previous = chain.last().expect("the previous link");
            create_tcp_listener(&router, previous, &group).await;
        }
        chain.push(arn);
    }

    // The clock-owed moves, placed rather than slept for: every load balancer is active and
    // every registration is healthy.
    let past = SystemTime::now() - Duration::from_secs(1);
    for arn in [&application, &network, &cyclic, &absent]
        .into_iter()
        .chain(chain.iter())
    {
        settle(&engine, arn, past);
    }

    // The rows: the host each request is addressed to — the name a load balancer publishes —
    // the path asked for, and the answer the chain behind that name produces.
    let rows: &[(&str, &str, &str, u16, &str)] = &[
        (
            "a path the rule of the load balancer behind the chain matches",
            &network_dns,
            "/orders/42",
            MATCHED_STATUS,
            MATCHED_BODY,
        ),
        (
            "a deeper path under the same matched prefix",
            &network_dns,
            "/orders/42/items",
            MATCHED_STATUS,
            MATCHED_BODY,
        ),
        (
            "a path no rule matches, answered by that listener's default action",
            &network_dns,
            "/health",
            DEFAULT_STATUS,
            DEFAULT_BODY,
        ),
        (
            "a chain that leads back to a load balancer already in it",
            &cyclic_dns,
            "/orders/42",
            UNREACHABLE_STATUS,
            "",
        ),
        (
            "a chain of distinct load balancers longer than the walk this plane follows",
            &chain_dns,
            "/orders/42",
            UNREACHABLE_STATUS,
            "",
        ),
        (
            "a registration naming a load balancer this service does not hold",
            &absent_dns,
            "/orders/42",
            UNREACHABLE_STATUS,
            "",
        ),
    ];

    for (case, host, path, status, body) in rows {
        let answered: Answered = send(&router, "GET", host, path, &[]).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected HTTP {status} from {host}{path}, got {} with body {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.body, *body,
            "case {case:?}: expected the body the answering listener's action carries, got \
             {:?}",
            answered.body
        );
    }
}
