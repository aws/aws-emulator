// SPDX-License-Identifier: CC0-1.0

//! The Classic load balancer DATA plane, end to end over the wire: what a client that sends
//! traffic to the address a load balancer PUBLISHED gets back.
//!
//! The rules under test:
//!
//! 1. **the address a load balancer publishes as its `DNSName` is an address this emulator
//!    answers on, and the load balancer answers there.** `DNSName` is the address a client
//!    dials, so a host in Elastic Load Balancing's own DNS zone — which resolves to nothing
//!    on the machine this emulator runs on — is no address at all; what is published is the
//!    `host:port` of the socket this emulator serves that load balancer's traffic on, while
//!    `CanonicalHostedZoneName` stays the name that zone carries.
//! 2. **a request is answered only by a registered instance the load balancer may send it
//!    to**: one its own health checks report `InService` and — while
//!    `CrossZoneLoadBalancing` is off — one in the zone of the node that took the request.
//!    With no such instance the load balancer answers the status it answers for having no
//!    capacity to serve the request; with one, it selects it, and the connection this
//!    emulator cannot make to a backend nothing runs is the status a load balancer answers
//!    for a registered instance it could not connect to.
//! 3. **a listener carrying a duration-based stickiness policy pins a client to the
//!    instance that was selected for it**: the answer to a cookie-less request carries the
//!    `AWSELB` cookie naming that instance, a request carrying it is served by that same
//!    instance and is handed no new cookie, and a cookie-less client keeps being spread
//!    across the instances the load balancer may send requests to.
//!
//! Every case drives the real wire path: the control plane through this service's own
//! awsQuery operations (`CreateLoadBalancer`, `ConfigureHealthCheck`,
//! `RegisterInstancesWithLoadBalancer`, `ModifyLoadBalancerAttributes`,
//! `CreateLBCookieStickinessPolicy`, `SetLoadBalancerPoliciesOfListener`), the backends
//! through Amazon EC2's own `RunInstances` on the same engine, and the traffic as an HTTP
//! request through the multi-service router carrying no AWS credential — exactly as a
//! client of the published address sends it.
//!
//! WHICH instance the load balancer selected is read where a client reads it: off the
//! `AWSELB` cookie the answer hands back, whose token names one instance of one load
//! balancer ([`instance_token`]).
//!
//! Most cases drive the plane through the router, which is the path the socket's own
//! forwarded leg takes; the last one DIALS the published `host:port` over TCP and speaks
//! HTTP/1.1 on it, which is what a client handed that address does, so the transport and the
//! region-exact addressing behind it are exercised too.
//!
//! The health checks the clock owes a registration are placed in the past rather than slept
//! for ([`age_registrations`]), exactly as this crate's health suites place them.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::service::stickiness::{
    instance_token, STICKINESS_COOKIE,
};
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::HEALTH_CHECK_SPAN;
use smithy_server_runtime::MultiServiceRouter;

use super::wire::{
    age_registrations, call, emulator_with_ec2, kid_text, launch_instance_in_zone, members,
    move_instance, param, parse, settle_ec2_transition,
};

/// The region every call below is signed for, unless a row names another.
const REGION: &str = "us-east-1";

/// The host a client addresses the emulator's own endpoint by, which is where the path
/// addressing of a load balancer's plane is read.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The status a load balancer with no registered instance it may send the request to
/// answers, from the Classic Load Balancer's own `HTTP 503: Service unavailable`.
const NO_INSTANCE: u16 = 503;

/// The status it answers once it HAS selected a registered instance and the connection to
/// that instance cannot be established, from its own `HTTP 504: Gateway Timeout`.
const INSTANCE_UNREACHABLE: u16 = 504;

/// The listener every load balancer below carries, and the backend port behind it.
const LISTENER_PORT: i32 = 80;
const INSTANCE_PORT: i32 = 8080;

/// The thresholds every health check below is configured with: two checks either way, so a
/// registration reaches a verdict after two checks have fallen due.
const HEALTHY_THRESHOLD: i32 = 2;
const UNHEALTHY_THRESHOLD: i32 = 2;

/// One answered data-plane request: the status, and the stickiness cookie the answer
/// carried.
struct Answered {
    status: u16,
    set_cookie: Option<String>,
}

impl Answered {
    /// The `AWSELB` token this answer minted, or `None` when it minted no cookie.
    fn minted_token(&self) -> Option<String> {
        let field = self.set_cookie.as_deref()?;
        let value = field
            .split(';')
            .next()?
            .trim()
            .strip_prefix(&format!("{STICKINESS_COOKIE}="))?;
        Some(value.to_string())
    }
}

/// Send one data-plane request: no credential, no body, and the `Cookie` header a client
/// that holds a pin carries back.
async fn send(
    router: &MultiServiceRouter,
    host: &str,
    target: &str,
    cookie: Option<&str>,
) -> Answered {
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(format!("http://{host}{target}"))
        .header("host", host);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("the data-plane request is well formed");
    let response = router.dispatch(request).await;
    Answered {
        status: response.status().as_u16(),
        set_cookie: response
            .headers()
            .get("set-cookie")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    }
}

/// Create one load balancer with an HTTP listener and answer the address it published.
async fn create_load_balancer(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    scheme: Option<&str>,
    protocol: &str,
    zones: &[&str],
) -> String {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("Listeners.member.1.Protocol", protocol),
        param(
            "Listeners.member.1.LoadBalancerPort",
            &LISTENER_PORT.to_string(),
        ),
        param(
            "Listeners.member.1.InstancePort",
            &INSTANCE_PORT.to_string(),
        ),
    ];
    if let Some(scheme) = scheme {
        params.push(param("Scheme", scheme));
    }
    for (index, zone) in zones.iter().enumerate() {
        params.push(param(
            &format!("AvailabilityZones.member.{}", index + 1),
            zone,
        ));
    }
    let created = call(router, region, "CreateLoadBalancer", &params).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateLoadBalancer must answer 200, got {}",
        created.body
    );
    let doc = parse(&created.body);
    kid_text(created.result(&doc), "DNSName")
        .unwrap_or_else(|| panic!("case {name:?}: no DNSName in {}", created.body))
}

/// Configure the health check every backend below is judged by.
async fn configure_health_check(router: &MultiServiceRouter, region: &str, name: &str) {
    let configured = call(
        router,
        region,
        "ConfigureHealthCheck",
        &[
            param("LoadBalancerName", name),
            param("HealthCheck.Target", &format!("HTTP:{INSTANCE_PORT}/ping")),
            param("HealthCheck.Interval", "5"),
            param("HealthCheck.Timeout", "2"),
            param(
                "HealthCheck.HealthyThreshold",
                &HEALTHY_THRESHOLD.to_string(),
            ),
            param(
                "HealthCheck.UnhealthyThreshold",
                &UNHEALTHY_THRESHOLD.to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "case {name:?}: ConfigureHealthCheck must answer 200, got {}",
        configured.body
    );
}

/// Register every id of `instance_ids` with the load balancer in ONE call.
async fn register(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    instance_ids: &[&str],
) {
    let mut params = vec![param("LoadBalancerName", name)];
    for (index, instance_id) in instance_ids.iter().enumerate() {
        params.push(param(
            &format!("Instances.member.{}.InstanceId", index + 1),
            instance_id,
        ));
    }
    let registered = call(
        router,
        region,
        "RegisterInstancesWithLoadBalancer",
        &params,
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "case {name:?}: registering {instance_ids:?} must answer 200, got {}",
        registered.body
    );
}

/// Turn `CrossZoneLoadBalancing` on or off through the attribute's own operation.
async fn set_cross_zone(router: &MultiServiceRouter, region: &str, name: &str, enabled: bool) {
    let modified = call(
        router,
        region,
        "ModifyLoadBalancerAttributes",
        &[
            param("LoadBalancerName", name),
            param(
                "LoadBalancerAttributes.CrossZoneLoadBalancing.Enabled",
                if enabled { "true" } else { "false" },
            ),
        ],
    )
    .await;
    assert_eq!(
        modified.status, 200,
        "case {name:?}: ModifyLoadBalancerAttributes must answer 200, got {}",
        modified.body
    );
}

/// Create a duration-based stickiness policy and enable it on the listener, which is the
/// pair of calls that attaches session stickiness to a port.
async fn attach_lb_cookie_policy(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    policy_name: &str,
    cookie_expiration_period: Option<&str>,
) {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("PolicyName", policy_name),
    ];
    if let Some(period) = cookie_expiration_period {
        params.push(param("CookieExpirationPeriod", period));
    }
    let created = call(router, region, "CreateLBCookieStickinessPolicy", &params).await;
    assert_eq!(
        created.status, 200,
        "case {policy_name:?}: CreateLBCookieStickinessPolicy must answer 200, got {}",
        created.body
    );
    enable_policies(router, region, name, &[policy_name]).await;
}

/// Enable exactly `policy_names` on the listener, through the operation that replaces the
/// set.
async fn enable_policies(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    policy_names: &[&str],
) {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("LoadBalancerPort", &LISTENER_PORT.to_string()),
    ];
    for (index, policy_name) in policy_names.iter().enumerate() {
        params.push(param(
            &format!("PolicyNames.member.{}", index + 1),
            policy_name,
        ));
    }
    let enabled = call(
        router,
        region,
        "SetLoadBalancerPoliciesOfListener",
        &params,
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "case {policy_names:?}: SetLoadBalancerPoliciesOfListener must answer 200, got {}",
        enabled.body
    );
    // The enabled set is read back where a client reads it: the listener's own
    // `PolicyNames`.
    let described = call(
        router,
        region,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    let doc = parse(&described.body);
    let result = described.result(&doc);
    let description = *members(result, "LoadBalancerDescriptions")
        .first()
        .unwrap_or_else(|| panic!("case {name:?}: no description in {}", described.body));
    let listener = *members(description, "ListenerDescriptions")
        .first()
        .unwrap_or_else(|| panic!("case {name:?}: no listener in {}", described.body));
    let reported = super::wire::member_texts(listener, "PolicyNames");
    assert_eq!(
        reported,
        policy_names
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<String>>(),
        "case {policy_names:?}: the listener must report the enabled set, got {reported:?}"
    );
}

/// What every stickiness and cross-zone case below is driven against: one load balancer
/// with one HTTP listener, placed in two zones, with one running backend registered in
/// each and both already past their healthy threshold.
struct Fixture {
    router: MultiServiceRouter,
    name: &'static str,
    dns: String,
    /// The backend in the FIRST of the load balancer's zones — the zone of the node that
    /// takes a request, which is the only zone a node serves while cross-zone load
    /// balancing is off.
    in_node_zone: String,
    /// And the backend in the other zone.
    in_other_zone: String,
}

impl Fixture {
    /// The `AWSELB` token naming each backend of this fixture.
    fn token(&self, instance_id: &str) -> String {
        instance_token(self.name, instance_id)
    }

    /// Send one cookie-less request to the published address.
    async fn get(&self) -> Answered {
        send(&self.router, &self.dns, "/", None).await
    }

    /// Send one request carrying the pin `token`.
    async fn get_pinned(&self, token: &str) -> Answered {
        send(
            &self.router,
            &self.dns,
            "/",
            Some(&format!("{STICKINESS_COOKIE}={token}")),
        )
        .await
    }
}

/// The two zones every fixture below is placed in: the node's own and one beside it.
const NODE_ZONE: &str = "us-east-1a";
const OTHER_ZONE: &str = "us-east-1b";

async fn fixture(name: &'static str, cross_zone: bool) -> (Fixture, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_ec2();
    let dns = create_load_balancer(
        &router,
        REGION,
        name,
        None,
        "HTTP",
        &[NODE_ZONE, OTHER_ZONE],
    )
    .await;
    configure_health_check(&router, REGION, name).await;
    if cross_zone {
        set_cross_zone(&router, REGION, name, true).await;
    }
    let in_node_zone = launch_instance_in_zone(&router, REGION, Some(NODE_ZONE), "node zone").await;
    let in_other_zone =
        launch_instance_in_zone(&router, REGION, Some(OTHER_ZONE), "other zone").await;
    settle_ec2_transition().await;
    register(&router, REGION, name, &[&in_node_zone, &in_other_zone]).await;
    // The checks both registrations owe, placed in the past so each has reached its
    // healthy threshold without the suite waiting for the cadence.
    age_registrations(
        &engine,
        REGION,
        name,
        HEALTH_CHECK_SPAN.saturating_mul(u32::try_from(HEALTHY_THRESHOLD).expect("a threshold")),
    );
    (
        Fixture {
            router,
            name,
            dns,
            in_node_zone,
            in_other_zone,
        },
        engine,
    )
}

/// RULE 1: the address a load balancer publishes is one this emulator answers on, the load
/// balancer answers there, and `CanonicalHostedZoneName` keeps the name Elastic Load
/// Balancing's own zone carries.
#[tokio::test]
async fn the_published_address_is_one_the_emulator_answers_on() {
    let (router, _engine) = emulator_with_ec2();
    // The rows span what the rule generalises over: the region (so the zone name a
    // hosted-zone name carries varies), the scheme — an `internal` load balancer publishes
    // an address too, because the client of one is inside this emulator — and the listener
    // protocol and port.
    let cases: &[(&str, &str, &str, Option<&str>, &str)] = &[
        ("the default scheme", "us-east-1", "web-elb", None, "HTTP"),
        (
            "an internal load balancer",
            "eu-west-1",
            "internal-elb",
            Some("internal"),
            "HTTP",
        ),
        (
            "a TCP listener",
            "ap-southeast-2",
            "tcp-elb",
            None,
            "TCP",
        ),
    ];
    for (case, region, name, scheme, protocol) in cases {
        let zone = format!("{region}a");
        let dns = create_load_balancer(&router, region, name, *scheme, protocol, &[&zone]).await;
        let address = dns.parse::<SocketAddr>();
        assert!(
            address.is_ok(),
            "case {case:?}: the published DNSName must be an address a client dials, got \
             {dns:?}"
        );
        assert!(
            !dns.contains("elb.amazonaws.com"),
            "case {case:?}: the published DNSName must not be a host in AWS's own zone, got \
             {dns:?}"
        );

        let described = call(
            &router,
            region,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", name)],
        )
        .await;
        let doc = parse(&described.body);
        let description = *members(described.result(&doc), "LoadBalancerDescriptions")
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: no description in {}", described.body));
        assert_eq!(
            kid_text(description, "DNSName").as_deref(),
            Some(dns.as_str()),
            "case {case:?}: a describe must report the address the create published, got {}",
            described.body
        );
        let hosted_zone_name = kid_text(description, "CanonicalHostedZoneName").unwrap_or_default();
        assert!(
            hosted_zone_name.ends_with(&format!(".{region}.elb.amazonaws.com")),
            "case {case:?}: CanonicalHostedZoneName is the name the hosted zone carries, got \
             {hosted_zone_name:?}"
        );

        // The load balancer ANSWERS there: nothing is registered yet, so what it answers is
        // its own "no instance to send this to" — not the front door's unknown-service
        // refusal, which is what an unserved address answers.
        let answered = send(&router, &dns, "/", None).await;
        assert_eq!(
            answered.status, NO_INSTANCE,
            "case {case:?}: the published address must be served by the load balancer, got \
             {}",
            answered.status
        );
        // And the same load balancer answers the path addressing of its own plane on the
        // emulator's own endpoint, which is how the socket hands a request it accepted to
        // the plane — the REGION included, because a Classic load balancer's name is unique
        // within one region only.
        let by_path = send(
            &router,
            EMULATOR_HOST,
            &format!("/_aws/elasticloadbalancing-classic/{region}/{name}/"),
            None,
        )
        .await;
        assert_eq!(
            by_path.status, NO_INSTANCE,
            "case {case:?}: the load balancer's own plane must answer its path addressing, \
             got {}",
            by_path.status
        );
        // A path naming ANOTHER region names no load balancer of this store, so this plane
        // does not answer it: a name is unique per region, and a request addressed to one
        // load balancer may never be served out of another's listeners.
        let wrong_region = send(
            &router,
            EMULATOR_HOST,
            &format!("/_aws/elasticloadbalancing-classic/me-south-1/{name}/"),
            None,
        )
        .await;
        assert!(
            wrong_region.status != NO_INSTANCE && wrong_region.status != INSTANCE_UNREACHABLE,
            "case {case:?}: no load balancer may answer for another region, got {}",
            wrong_region.status
        );
    }
}

/// RULE 1, the other half: a deleted load balancer answers nowhere — "the DNS name
/// associated with a deleted load balancer is no longer usable".
#[tokio::test]
async fn a_deleted_load_balancer_answers_on_no_address() {
    let (router, _engine) = emulator_with_ec2();
    let dns =
        create_load_balancer(&router, REGION, "going-elb", None, "HTTP", &[NODE_ZONE]).await;
    let served = send(&router, &dns, "/", None).await;
    assert_eq!(
        served.status, NO_INSTANCE,
        "case before the delete: the load balancer must answer on its own address, got {}",
        served.status
    );
    let deleted = call(
        &router,
        REGION,
        "DeleteLoadBalancer",
        &[param("LoadBalancerName", "going-elb")],
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: DeleteLoadBalancer must answer 200, got {}",
        deleted.body
    );
    let after = send(&router, &dns, "/", None).await;
    assert!(
        after.status != NO_INSTANCE && after.status != INSTANCE_UNREACHABLE,
        "case after the delete: no load balancer may answer on that address, got {}",
        after.status
    );
}

/// RULE 2: a request is answered only by a registered instance the load balancer may send
/// it to, and the verdict of that instance's own health checks is what decides it.
#[tokio::test]
async fn only_an_in_service_registered_instance_takes_a_request() {
    const NAME: &str = "health-gated-elb";
    let (router, engine) = emulator_with_ec2();
    let dns = create_load_balancer(&router, REGION, NAME, None, "HTTP", &[NODE_ZONE]).await;
    configure_health_check(&router, REGION, NAME).await;

    // Nothing registered: the load balancer has no instance to send the request to.
    let empty = send(&router, &dns, "/", None).await;
    assert_eq!(
        empty.status, NO_INSTANCE,
        "case no instance registered: expected {NO_INSTANCE}, got {}",
        empty.status
    );

    let serving = launch_instance_in_zone(&router, REGION, Some(NODE_ZONE), "serving").await;
    settle_ec2_transition().await;
    register(&router, REGION, NAME, &[&serving]).await;

    // Registered, and no check of it has fallen due yet: a registration still coming up
    // takes no traffic.
    let coming_up = send(&router, &dns, "/", None).await;
    assert_eq!(
        coming_up.status, NO_INSTANCE,
        "case the registration still coming up: expected {NO_INSTANCE}, got {}",
        coming_up.status
    );

    // Its healthy threshold reached: the load balancer selects it, and the connection to a
    // backend this emulator runs no server for cannot be established.
    let healthy_run =
        HEALTH_CHECK_SPAN.saturating_mul(u32::try_from(HEALTHY_THRESHOLD).expect("a threshold"));
    age_registrations(&engine, REGION, NAME, healthy_run);
    let in_service = send(&router, &dns, "/", None).await;
    assert_eq!(
        in_service.status, INSTANCE_UNREACHABLE,
        "case the backend in service: expected {INSTANCE_UNREACHABLE}, got {}",
        in_service.status
    );

    // The backend stopped, and its unhealthy threshold reached: it is out of service, so
    // the load balancer has nothing to send the request to again.
    move_instance(&router, REGION, "stopping", "StopInstances", &serving).await;
    age_registrations(
        &engine,
        REGION,
        NAME,
        HEALTH_CHECK_SPAN.saturating_mul(u32::try_from(UNHEALTHY_THRESHOLD).expect("a threshold")),
    );
    let out_of_service = send(&router, &dns, "/", None).await;
    assert_eq!(
        out_of_service.status, NO_INSTANCE,
        "case the backend out of service: expected {NO_INSTANCE}, got {}",
        out_of_service.status
    );
}

/// RULE 2, the placement half: `CrossZoneLoadBalancing` decides whether the node that took
/// the request may send it to an instance outside its own zone.
#[tokio::test]
async fn cross_zone_load_balancing_decides_which_zones_a_node_reaches() {
    const NAME: &str = "cross-zone-elb";
    // The pin is what names the instance the load balancer selected, so the policy is
    // attached first and every request below reads the choice off the cookie.
    let (fixture, _engine) = fixture(NAME, false).await;
    attach_lb_cookie_policy(&fixture.router, REGION, NAME, "pin", Some("300")).await;
    let node_token = fixture.token(&fixture.in_node_zone);
    let other_token = fixture.token(&fixture.in_other_zone);

    // Cross-zone load balancing OFF: every request is answered by the instance in the
    // node's own zone, however many requests are drawn.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for attempt in 0..24 {
        let answered = fixture.get().await;
        assert_eq!(
            answered.status, INSTANCE_UNREACHABLE,
            "case cross-zone off, request {attempt}: expected {INSTANCE_UNREACHABLE}, got {}",
            answered.status
        );
        seen.insert(
            answered
                .minted_token()
                .unwrap_or_else(|| panic!("case cross-zone off, request {attempt}: no cookie")),
        );
    }
    assert_eq!(
        seen,
        BTreeSet::from([node_token.clone()]),
        "case cross-zone off: only the instance in the node's own zone may take requests, \
         got {seen:?}"
    );

    // Cross-zone load balancing ON: the same node spreads requests to the instances in
    // every zone the load balancer is in.
    set_cross_zone(&fixture.router, REGION, NAME, true).await;
    let mut spread: BTreeSet<String> = BTreeSet::new();
    for _ in 0..40 {
        if let Some(token) = fixture.get().await.minted_token() {
            spread.insert(token);
        }
        if spread.len() == 2 {
            break;
        }
    }
    assert_eq!(
        spread,
        BTreeSet::from([node_token, other_token]),
        "case cross-zone on: both zones' instances must take requests, got {spread:?}"
    );
}

/// RULE 2, the placement half again: with cross-zone load balancing off, a load balancer
/// whose only in-service instance is in ANOTHER zone has nothing the node may send the
/// request to.
#[tokio::test]
async fn a_node_with_no_instance_in_its_own_zone_answers_no_capacity() {
    const NAME: &str = "other-zone-only-elb";
    let (fixture, _engine) = fixture(NAME, false).await;
    let deregistered = call(
        &fixture.router,
        REGION,
        "DeregisterInstancesFromLoadBalancer",
        &[
            param("LoadBalancerName", NAME),
            param("Instances.member.1.InstanceId", &fixture.in_node_zone),
        ],
    )
    .await;
    assert_eq!(
        deregistered.status, 200,
        "case deregistering the node-zone backend: expected 200, got {}",
        deregistered.body
    );
    let answered = fixture.get().await;
    assert_eq!(
        answered.status, NO_INSTANCE,
        "case only the other zone's backend registered: expected {NO_INSTANCE}, got {}",
        answered.status
    );
}

/// RULE 3: the listener's duration-based stickiness policy mints the `AWSELB` cookie
/// naming the instance a cookie-less request was selected for, and pins every request
/// carrying it to that same instance.
#[tokio::test]
async fn a_duration_based_policy_pins_a_client_to_the_instance_it_names() {
    const NAME: &str = "sticky-elb";
    let (fixture, _engine) = fixture(NAME, true).await;
    let tokens = BTreeSet::from([
        fixture.token(&fixture.in_node_zone),
        fixture.token(&fixture.in_other_zone),
    ]);

    // No policy attached: nothing pins the session, so no cookie is minted.
    let unpinned = fixture.get().await;
    assert_eq!(
        unpinned.status, INSTANCE_UNREACHABLE,
        "case no policy: expected {INSTANCE_UNREACHABLE}, got {}",
        unpinned.status
    );
    assert_eq!(
        unpinned.set_cookie, None,
        "case no policy: no stickiness cookie may be minted, got {:?}",
        unpinned.set_cookie
    );

    // The policy attached, with an expiration period: the answer carries the cookie, for
    // that many seconds, naming one of the instances the load balancer may send requests
    // to.
    attach_lb_cookie_policy(&fixture.router, REGION, NAME, "sticky-300", Some("300")).await;
    let minted = fixture.get().await;
    let field = minted
        .set_cookie
        .clone()
        .unwrap_or_else(|| panic!("case the policy attached: no Set-Cookie on {}", minted.status));
    assert_eq!(
        field,
        format!(
            "{STICKINESS_COOKIE}={}; Path=/; Max-Age=300",
            minted.minted_token().unwrap_or_default()
        ),
        "case the policy attached: the cookie must carry the pin, the path and the period, \
         got {field:?}"
    );
    let pinned_to = minted
        .minted_token()
        .unwrap_or_else(|| panic!("case the policy attached: no pin in {field:?}"));
    assert!(
        tokens.contains(&pinned_to),
        "case the policy attached: the pin must name a registered instance, got {pinned_to:?}"
    );

    // Every request carrying that pin is served by the instance it names, and is handed no
    // new cookie: the client already holds the one that pins it.
    for attempt in 0..16 {
        let answered = fixture.get_pinned(&pinned_to).await;
        assert_eq!(
            answered.status, INSTANCE_UNREACHABLE,
            "case pinned request {attempt}: expected {INSTANCE_UNREACHABLE}, got {}",
            answered.status
        );
        assert_eq!(
            answered.set_cookie, None,
            "case pinned request {attempt}: a held pin must not be replaced, got {:?}",
            answered.set_cookie
        );
    }

    // A pin naming no instance the load balancer may send requests to is no pin at all:
    // the session is re-pinned to an instance that can take it.
    for (case, carried) in [
        ("a pin naming no instance", "not-a-token".to_string()),
        (
            "a pin naming an unregistered instance",
            instance_token(NAME, "i-00000000000000bad"),
        ),
    ] {
        let repinned = fixture.get_pinned(&carried).await;
        let token = repinned
            .minted_token()
            .unwrap_or_else(|| panic!("case {case}: no cookie was minted"));
        assert!(
            tokens.contains(&token),
            "case {case}: the session must be re-pinned to a registered instance, got \
             {token:?}"
        );
    }

    // A cookie-less client is still spread across the instances the load balancer may send
    // requests to: each request of its own is drawn.
    let mut spread: BTreeSet<String> = BTreeSet::new();
    for _ in 0..40 {
        if let Some(token) = fixture.get().await.minted_token() {
            spread.insert(token);
        }
        if spread.len() == tokens.len() {
            break;
        }
    }
    assert_eq!(
        spread, tokens,
        "case a cookie-less client: requests must be spread across the registered \
         instances, got {spread:?}"
    );
}

/// RULE 3, the two other policy shapes: a duration-based policy at the documented default
/// period hands back a cookie with no lifetime of its own, and an
/// application-controlled policy mints none at all — it follows a cookie the application's
/// own response carries, and a load balancer with no reachable backend receives none.
#[tokio::test]
async fn the_cookie_follows_the_policy_the_listener_carries() {
    const NAME: &str = "policy-shapes-elb";
    let (fixture, _engine) = fixture(NAME, true).await;

    // The documented default: "the default value is 0, which indicates that the sticky
    // session should last for the duration of the browser session", so the cookie carries
    // no `Max-Age` and the browser drops it with the session.
    attach_lb_cookie_policy(&fixture.router, REGION, NAME, "session-pin", None).await;
    let minted = fixture.get().await;
    let field = minted
        .set_cookie
        .clone()
        .unwrap_or_else(|| panic!("case the browser-session policy: no Set-Cookie"));
    assert_eq!(
        field,
        format!(
            "{STICKINESS_COOKIE}={}; Path=/",
            minted.minted_token().unwrap_or_default()
        ),
        "case the browser-session policy: the cookie must carry no lifetime, got {field:?}"
    );

    // An application-controlled policy in its place: the pin a client already holds is
    // honoured, and no new cookie is minted for a cookie-less client.
    let created = call(
        &fixture.router,
        REGION,
        "CreateAppCookieStickinessPolicy",
        &[
            param("LoadBalancerName", NAME),
            param("PolicyName", "app-pin"),
            param("CookieName", "JSESSIONID"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the application policy: CreateAppCookieStickinessPolicy must answer 200, got {}",
        created.body
    );
    enable_policies(&fixture.router, REGION, NAME, &["app-pin"]).await;
    let unminted = fixture.get().await;
    assert_eq!(
        unminted.set_cookie, None,
        "case the application policy: no cookie may be minted without an application \
         response to follow, got {:?}",
        unminted.set_cookie
    );
    let held = fixture.token(&fixture.in_node_zone);
    let pinned = fixture.get_pinned(&held).await;
    assert_eq!(
        (pinned.status, pinned.set_cookie.clone()),
        (INSTANCE_UNREACHABLE, None),
        "case the application policy with a pin: the held pin must be honoured, got {:?}",
        pinned.set_cookie
    );
}

/// The configurations a listener cannot be given, which is what
/// `InvalidConfigurationRequestException` refuses: session stickiness on a listener that
/// reads no cookie, and two stickiness policies at once.
#[tokio::test]
async fn a_listener_refuses_a_policy_set_its_protocol_cannot_carry() {
    const NAME: &str = "tcp-listener-elb";
    let (router, _engine) = emulator_with_ec2();
    create_load_balancer(&router, REGION, NAME, None, "TCP", &[NODE_ZONE]).await;
    for (case, policy_name, params) in [
        (
            "duration-based stickiness",
            "tcp-pin",
            vec![param("CookieExpirationPeriod", "60")],
        ),
        (
            "application-controlled stickiness",
            "tcp-app-pin",
            vec![param("CookieName", "JSESSIONID")],
        ),
    ] {
        let mut created = vec![
            param("LoadBalancerName", NAME),
            param("PolicyName", policy_name),
        ];
        created.extend(params);
        let action = if policy_name == "tcp-pin" {
            "CreateLBCookieStickinessPolicy"
        } else {
            "CreateAppCookieStickinessPolicy"
        };
        let reply = call(&router, REGION, action, &created).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: creating the policy must answer 200, got {}",
            reply.body
        );
        let refused = call(
            &router,
            REGION,
            "SetLoadBalancerPoliciesOfListener",
            &[
                param("LoadBalancerName", NAME),
                param("LoadBalancerPort", &LISTENER_PORT.to_string()),
                param("PolicyNames.member.1", policy_name),
            ],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (409, "InvalidConfigurationRequest"),
            "case {case} on a TCP listener: expected a refused configuration, got {} {}",
            refused.status,
            refused.body
        );
    }

    // And a port naming a listener the load balancer does not have.
    let unknown_port = call(
        &router,
        REGION,
        "SetLoadBalancerPoliciesOfListener",
        &[
            param("LoadBalancerName", NAME),
            param("LoadBalancerPort", "8443"),
            param("PolicyNames.member.1", "tcp-pin"),
        ],
    )
    .await;
    assert_eq!(
        (unknown_port.status, unknown_port.error_code().as_str()),
        (400, "ListenerNotFound"),
        "case a port with no listener: expected ListenerNotFound, got {} {}",
        unknown_port.status,
        unknown_port.body
    );

    // And a policy the load balancer does not hold.
    let unknown_policy = call(
        &router,
        REGION,
        "SetLoadBalancerPoliciesOfListener",
        &[
            param("LoadBalancerName", NAME),
            param("LoadBalancerPort", &LISTENER_PORT.to_string()),
            param("PolicyNames.member.1", "no-such-policy"),
        ],
    )
    .await;
    assert_eq!(
        (unknown_policy.status, unknown_policy.error_code().as_str()),
        (400, "PolicyNotFound"),
        "case a policy the load balancer does not hold: expected PolicyNotFound, got {} {}",
        unknown_policy.status,
        unknown_policy.body
    );
}

/// RULE 1, end to end over a REAL connection: a client that DIALS the published `host:port`
/// — as a client handed that address does, over TCP, speaking HTTP/1.1, carrying no AWS
/// credential — is answered by the load balancer that published it.
///
/// Two load balancers of the SAME NAME in different regions are deployed, each with its own
/// registered backend: a Classic load balancer's name is unique within one region only, so
/// each socket must answer out of its OWN region's listeners, instances and policies. The
/// `AWSELB` cookie names the instance each one selected, which is how the test tells them
/// apart.
#[tokio::test]
async fn a_client_that_dials_the_published_address_reaches_that_load_balancer() {
    const NAME: &str = "same-name-elb";
    const OTHER_REGION: &str = "us-west-2";
    let (router, engine) = emulator_with_ec2();

    let mut published: Vec<(&str, String, String)> = Vec::new();
    for (region, zone) in [(REGION, NODE_ZONE), (OTHER_REGION, "us-west-2a")] {
        let dns = create_load_balancer(&router, region, NAME, None, "HTTP", &[zone]).await;
        configure_health_check(&router, region, NAME).await;
        let backend = launch_instance_in_zone(&router, region, Some(zone), region).await;
        settle_ec2_transition().await;
        register(&router, region, NAME, &[&backend]).await;
        age_registrations(
            &engine,
            region,
            NAME,
            HEALTH_CHECK_SPAN
                .saturating_mul(u32::try_from(HEALTHY_THRESHOLD).expect("a threshold")),
        );
        // The pin is what names the instance each load balancer selected.
        attach_lb_cookie_policy(&router, region, NAME, &format!("pin-{region}"), Some("60")).await;
        published.push((region, dns, backend));
    }
    assert_ne!(
        published[0].1, published[1].1,
        "case two regions: each load balancer must publish its own address, got {:?}",
        published
    );

    for (region, dns, backend) in &published {
        let dialled = dial(dns, "/", None).await;
        assert_eq!(
            dialled.status, INSTANCE_UNREACHABLE,
            "case dialling {region}'s published address: expected {INSTANCE_UNREACHABLE}, got \
             {}",
            dialled.status
        );
        let token = dialled
            .minted_token()
            .unwrap_or_else(|| panic!("case dialling {region}: no stickiness cookie"));
        assert_eq!(
            token,
            instance_token(NAME, backend),
            "case dialling {region}'s published address: the answer must come from that \
             region's own backend, got {token:?}"
        );
        // And a client that carries the pin back over its own connection is served by the
        // instance it names, with no new cookie minted.
        let pinned = dial(dns, "/", Some(&format!("{STICKINESS_COOKIE}={token}"))).await;
        assert_eq!(
            (pinned.status, pinned.set_cookie.clone()),
            (INSTANCE_UNREACHABLE, None),
            "case dialling {region} with the pin: the held pin must be honoured, got {:?}",
            pinned.set_cookie
        );
    }
}

/// Dial the published `host:port` over TCP and speak one HTTP/1.1 request on it, exactly as
/// a client handed that address does. The connection is closed by the answer
/// (`Connection: close`), so the response is read to end of stream rather than framed here.
///
/// The address dialled is this machine's own: the port is read off the address the load
/// balancer published and the host is the loopback literal, which is the only interface a
/// core-owned listener binds.
async fn dial(published: &str, target: &str, cookie: Option<&str>) -> Answered {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let port = published
        .parse::<SocketAddr>()
        .unwrap_or_else(|error| panic!("case dial {published:?}: not an address: {error}"))
        .port();
    let mut stream = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| panic!("case dial {published:?}: cannot connect: {error}"));
    let mut request = format!(
        "GET {target} HTTP/1.1\r\nHost: {published}\r\nConnection: close\r\n"
    );
    if let Some(cookie) = cookie {
        request.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case dial {published:?}: cannot write: {error}"));
    let mut answered = Vec::new();
    stream
        .read_to_end(&mut answered)
        .await
        .unwrap_or_else(|error| panic!("case dial {published:?}: cannot read: {error}"));
    let text = String::from_utf8_lossy(&answered).into_owned();
    let (head, _body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("case dial {published:?}: no status line in {text:?}"));
    let set_cookie = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.trim().to_string());
    Answered { status, set_cookie }
}
