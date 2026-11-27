// SPDX-License-Identifier: CC0-1.0

//! What `CreateTargetGroup`, `RegisterTargets`, `DescribeLoadBalancers` and
//! `DeleteTargetGroup` ACCEPT and REFUSE, over the wire, for the rules that need nothing
//! but the request and this service's own store.
//!
//! Four rules are tabled here, each over the class it generalises rather than the one
//! value an issue cited:
//!
//! * THE HEALTH-CHECK PATH. `HealthCheckPath` is a destination the load balancer can
//!   request: an absolute path, printable ASCII, no spaces, inside the length the model
//!   bounds `Path` to (1..1024). The model carries only that length, so the handler is the
//!   only gate and a value outside the rule is HTTP 400 `ValidationError` — on
//!   `CreateTargetGroup` and on `ModifyTargetGroup` alike, and a refused call stores
//!   nothing.
//! * THE PROTOCOL VERSION. `ProtocolVersion` is documented "[HTTP/HTTPS protocol]", so it
//!   belongs to an HTTP or HTTPS target group alone; naming it on a group of any other
//!   protocol, or on a Lambda group, is the modelled `InvalidConfigurationRequest`.
//! * THE HEALTH-CHECK MATCHER DEFAULT. An HTTP-semantic health check with no `Matcher`
//!   takes the default of the load balancer FAMILY the target group serves —
//!   `Matcher$HttpCode`: 200 for an Application Load Balancer's group, 200-399 for a
//!   Network or Gateway Load Balancer's — and the gRPC success code 12 for a GRPC group.
//! * THE TARGETS OF AN `ip` GROUP, and the quota of a target group. A target of an `ip`
//!   group is an address of the group's own address type, and a group holds at most the
//!   published number of targets for its family, past which `RegisterTargets` answers the
//!   modelled `TooManyTargets`.
//!
//! The rules that need the ACCOUNT'S NETWORK — which VPC a load balancer's subnets are
//! in, which subnet holds an `ip` target, which VPC a security group belongs to — are
//! resolved against EC2, so they are driven with both services mounted in
//! `core/router/tests/all/elbv2_network_resolution.rs` instead.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) is the one `provisioning_loop`
//! owns; nothing here restates it.

use crate::provisioning_loop::{action, call, emulator, first_text, members, p, Reply};
use crate::tag_writes::message;
use crate::target_group_attributes_and_modification::unescaped;
use crate::target_group_health_check_port::describe;
use smithy_server_runtime::MultiServiceRouter;

/// One row of the health-check path table: the case name, the value, and whether the
/// service accepts it.
type PathCase = (&'static str, String, bool);

/// Every value the path rule is tabled over: the documented default, paths no recorded
/// call cites, both published length bounds and one past the high bound, and values that
/// are no absolute printable-ASCII path at all.
fn path_cases() -> Vec<PathCase> {
    vec![
        ("the documented default", "/".to_string(), true),
        ("a plain path", "/healthz".to_string(), true),
        (
            "a path with a query the probe may carry",
            "/health?deep=1".to_string(),
            true,
        ),
        (
            "the gRPC method form",
            "/AWS.ALB/healthcheck".to_string(),
            true,
        ),
        // `Path` is `@length 1..1024`: the low bound is one character, which has to be
        // the leading slash, and the high bound is 1024 characters.
        ("the low length bound", "/".to_string(), true),
        (
            "the high length bound",
            format!("/{}", "a".repeat(1023)),
            true,
        ),
        (
            "one past the high length bound",
            format!("/{}", "a".repeat(1024)),
            false,
        ),
        (
            "no leading slash",
            "missing-leading-slash".to_string(),
            false,
        ),
        ("a relative path", "health/ready".to_string(), false),
        ("a path with a space", "/health check".to_string(), false),
        ("a path with a tab", "/health\tcheck".to_string(), false),
        (
            "a path outside printable ASCII",
            "/health\u{00e9}".to_string(),
            false,
        ),
        ("a whole URL", "https://host/health".to_string(), false),
    ]
}

/// One row of the protocol-version table: the case name, the target group's own fields,
/// the `ProtocolVersion` asked for, and whether the service accepts the pairing.
type ProtocolVersionCase = (&'static str, &'static [(&'static str, &'static str)], &'static str, bool);

/// The pairings the `ProtocolVersion` rule is tabled over: each protocol the model pairs
/// with a load balancer family (`Protocol`: HTTP and HTTPS for Application, TCP, TLS, UDP,
/// TCP_UDP and QUIC for Network, GENEVE for Gateway), plus a Lambda target group, which
/// has no protocol at all.
const PROTOCOL_VERSION_CASES: &[ProtocolVersionCase] = &[
    (
        "HTTP1 on an HTTP group",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        "HTTP1",
        true,
    ),
    (
        "HTTP2 on an HTTPS group",
        &[("Protocol", "HTTPS"), ("Port", "443"), ("VpcId", "vpc-1")],
        "HTTP2",
        true,
    ),
    (
        "GRPC on an HTTP group",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        "GRPC",
        true,
    ),
    (
        "HTTP2 on a TCP group",
        &[("Protocol", "TCP"), ("Port", "80"), ("VpcId", "vpc-1")],
        "HTTP2",
        false,
    ),
    (
        "HTTP1 on a TLS group",
        &[("Protocol", "TLS"), ("Port", "443"), ("VpcId", "vpc-1")],
        "HTTP1",
        false,
    ),
    (
        "HTTP1 on a UDP group",
        &[("Protocol", "UDP"), ("Port", "53"), ("VpcId", "vpc-1")],
        "HTTP1",
        false,
    ),
    (
        "GRPC on a TCP_UDP group",
        &[("Protocol", "TCP_UDP"), ("Port", "80"), ("VpcId", "vpc-1")],
        "GRPC",
        false,
    ),
    (
        "HTTP2 on a GENEVE group",
        &[("Protocol", "GENEVE"), ("Port", "6081"), ("VpcId", "vpc-1")],
        "HTTP2",
        false,
    ),
    (
        "HTTP1 on a lambda group",
        &[("TargetType", "lambda")],
        "HTTP1",
        false,
    ),
];

/// One row of the matcher-default table: the case name, the group's fields, and the
/// `HttpCode` / `GrpcCode` its health check must then report.
type MatcherCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    Option<&'static str>,
    Option<&'static str>,
);

/// The matcher default over the families `Matcher$HttpCode` names, plus the gRPC group its
/// sibling `GrpcCode` is for. Each Network/Gateway row has an HTTP-side health check, so
/// the default is read for a check that HAS a matcher while the GROUP is not an
/// Application Load Balancer's — the case the recorded TCP target group is one of.
const MATCHER_CASES: &[MatcherCase] = &[
    (
        "an HTTP group",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        Some("200"),
        None,
    ),
    (
        "an HTTPS group",
        &[("Protocol", "HTTPS"), ("Port", "443"), ("VpcId", "vpc-1")],
        Some("200"),
        None,
    ),
    (
        "a GRPC group",
        &[
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("VpcId", "vpc-1"),
            ("ProtocolVersion", "GRPC"),
        ],
        None,
        Some("12"),
    ),
    (
        "a TCP group with an HTTP health check",
        &[
            ("Protocol", "TCP"),
            ("Port", "80"),
            ("VpcId", "vpc-1"),
            ("HealthCheckProtocol", "HTTP"),
        ],
        Some("200-399"),
        None,
    ),
    (
        "a TLS group with an HTTPS health check",
        &[
            ("Protocol", "TLS"),
            ("Port", "443"),
            ("VpcId", "vpc-1"),
            ("HealthCheckProtocol", "HTTPS"),
        ],
        Some("200-399"),
        None,
    ),
    (
        "a UDP group with an HTTP health check",
        &[
            ("Protocol", "UDP"),
            ("Port", "53"),
            ("VpcId", "vpc-1"),
            ("HealthCheckProtocol", "HTTP"),
        ],
        Some("200-399"),
        None,
    ),
    (
        "a GENEVE group with an HTTP health check",
        &[
            ("Protocol", "GENEVE"),
            ("Port", "6081"),
            ("VpcId", "vpc-1"),
            ("HealthCheckProtocol", "HTTP"),
        ],
        Some("200-399"),
        None,
    ),
    (
        "a lambda group, whose check is an HTTP one",
        &[("TargetType", "lambda")],
        Some("200"),
        None,
    ),
];

/// One row of the `ip` target table: the case name, the group's `IpAddressType`, the
/// target id, and whether the service accepts it as an address of such a group.
type IpTargetCase = (&'static str, &'static str, &'static str, bool);

/// The ids the address rule is tabled over, for both address types a target group carries
/// its targets in (`TargetGroup$IpAddressType`): addresses of each version, and values
/// that are no address of the group's version at all.
const IP_TARGET_CASES: &[IpTargetCase] = &[
    ("an IPv4 address", "ipv4", "10.80.1.10", true),
    ("another IPv4 address", "ipv4", "172.16.4.9", true),
    ("an instance id", "ipv4", "i-0123456789abcdef0", false),
    ("a host name", "ipv4", "target.internal", false),
    ("an IPv6 address on an ipv4 group", "ipv4", "fd00::5", false),
    ("an address with a port", "ipv4", "10.80.1.10:80", false),
    ("a CIDR block", "ipv4", "10.80.1.0/24", false),
    ("an IPv6 address", "ipv6", "fd00::5", true),
    ("an IPv4 address on an ipv6 group", "ipv6", "10.80.1.10", false),
];

/// Create a target group from `fields` and answer the reply, whatever it is.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    fields: &[(&str, &str)],
) -> Reply {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    for (key, value) in fields {
        form.push(p(key, value));
    }
    call(router, &form).await
}

/// Create a target group that must be accepted, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, name: &str, fields: &[(&str, &str)]) -> String {
    let reply = create(router, name, fields).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected the create to be accepted, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {}", reply.body))
}

/// Register `targets` — each a `(Id, Port)` pair, with an empty port meaning none — and
/// answer the reply.
async fn register(router: &MultiServiceRouter, arn: &str, targets: &[(&str, &str)]) -> Reply {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    for (index, (id, port)) in targets.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), id));
        if !port.is_empty() {
            form.push(p(&format!("Targets.member.{}.Port", index + 1), port));
        }
    }
    call(router, &form).await
}

/// How many targets `DescribeTargetHealth` reports for the group.
async fn registered_count(router: &MultiServiceRouter, arn: &str) -> usize {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "reading the registrations back: expected 200, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions").len()
}

#[tokio::test]
async fn a_health_check_path_is_accepted_only_by_the_published_rule() {
    let (router, _engine) = emulator();

    for (index, (case, value, accepted)) in path_cases().into_iter().enumerate() {
        // On the create.
        let name = format!("hc-path-{index}");
        let reply = create(
            &router,
            &name,
            &[
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-1"),
                ("HealthCheckProtocol", "HTTP"),
                ("HealthCheckPath", value.as_str()),
            ],
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the create to be accepted, got {} body {}",
                reply.status, reply.body
            );
            let arn = reply
                .text("TargetGroupArn")
                .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {}", reply.body));
            let stored = describe(&router, &arn).await;
            assert_eq!(
                stored.text("HealthCheckPath").as_deref(),
                Some(value.as_str()),
                "case {case:?}: the accepted path must round-trip, got {}",
                stored.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?}: expected 400 ValidationError, got {} body {}",
                reply.status,
                reply.body
            );
            // A refused create stores nothing, so the name is still free.
            let mut form = action("DescribeTargetGroups");
            form.push(p("Names.member.1", &name));
            let read = call(&router, &form).await;
            assert_eq!(
                (read.status, read.error_code().as_str()),
                (400, "TargetGroupNotFound"),
                "case {case:?}: a refused create must store no target group, got {}",
                read.body
            );
        }

        // And on the modify, where the same member is accepted again: the path of a
        // target group that already exists only ever changes to a value the rule allows.
        let arn = create_ok(
            &router,
            case,
            &format!("mod-path-{index}"),
            &[
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-1"),
                ("HealthCheckProtocol", "HTTP"),
                ("HealthCheckPath", "/before"),
            ],
        )
        .await;
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("HealthCheckPath", value.as_str()));
        let reply = call(&router, &form).await;
        let stored = describe(&router, &arn).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?} on modify: expected 200, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                stored.text("HealthCheckPath").as_deref(),
                Some(value.as_str()),
                "case {case:?} on modify: expected the new path, got {}",
                stored.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?} on modify: expected 400 ValidationError, got {} body {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                stored.text("HealthCheckPath").as_deref(),
                Some("/before"),
                "case {case:?} on modify: a refused modify must change nothing, got {}",
                stored.body
            );
        }
    }
}

#[tokio::test]
async fn a_protocol_version_is_accepted_only_on_an_http_target_group() {
    let (router, _engine) = emulator();

    for (index, (case, fields, protocol_version, accepted)) in
        PROTOCOL_VERSION_CASES.iter().enumerate()
    {
        let name = format!("pv-{index}");
        let mut request: Vec<(&str, &str)> = fields.to_vec();
        request.push(("ProtocolVersion", protocol_version));
        let reply = create(&router, &name, &request).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the create to be accepted, got {} body {}",
                reply.status, reply.body
            );
            let arn = reply
                .text("TargetGroupArn")
                .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {}", reply.body));
            let stored = describe(&router, &arn).await;
            assert_eq!(
                stored.text("ProtocolVersion").as_deref(),
                Some(*protocol_version),
                "case {case:?}: the accepted protocol version must round-trip, got {}",
                stored.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "InvalidConfigurationRequest"),
                "case {case:?}: expected 400 InvalidConfigurationRequest, got {} body {}",
                reply.status,
                reply.body
            );
            // The refusal states the rule over what the request carried.
            assert!(
                reply
                    .text("Message")
                    .is_some_and(|message| message
                        .contains("cannot specify the protocol version")),
                "case {case:?}: the refusal must state the rule, got {}",
                reply.body
            );
        }
        // The same group WITHOUT a protocol version is always accepted, so each refusal
        // above is about the pairing rather than about the rest of the request.
        let control = create(&router, &format!("pv-control-{index}"), fields).await;
        assert_eq!(
            control.status, 200,
            "case {case:?}: the same group with no protocol version must be accepted, \
             got {} body {}",
            control.status, control.body
        );
    }
}

#[tokio::test]
async fn an_http_health_check_takes_the_matcher_default_of_the_load_balancer_family() {
    let (router, _engine) = emulator();

    for (index, (case, fields, http_code, grpc_code)) in MATCHER_CASES.iter().enumerate() {
        let arn = create_ok(&router, case, &format!("matcher-{index}"), fields).await;
        let stored = describe(&router, &arn).await;
        assert_eq!(
            (
                stored.text("HttpCode").as_deref(),
                stored.text("GrpcCode").as_deref()
            ),
            (*http_code, *grpc_code),
            "case {case:?}: unexpected matcher default in {}",
            stored.body
        );
        // What a caller sets is never replaced by the default.
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("Matcher.HttpCode", "201-204"));
        let reply = call(&router, &form).await;
        if grpc_code.is_none() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the matcher to be settable, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                describe(&router, &arn).await.text("HttpCode").as_deref(),
                Some("201-204"),
                "case {case:?}: the matcher a caller set must round-trip"
            );
        }
    }
}

#[tokio::test]
async fn a_target_of_an_ip_target_group_must_be_an_address_of_its_address_type() {
    let (router, _engine) = emulator();

    for (index, (case, address_type, id, accepted)) in IP_TARGET_CASES.iter().enumerate() {
        let arn = create_ok(
            &router,
            case,
            &format!("ip-target-{index}"),
            &[
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-1"),
                ("TargetType", "ip"),
                ("IpAddressType", address_type),
            ],
        )
        .await;
        let reply = register(&router, &arn, &[(id, "")]).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the registration to be accepted, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                registered_count(&router, &arn).await,
                1,
                "case {case:?}: the accepted target must be registered"
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?}: expected 400 ValidationError, got {} body {}",
                reply.status,
                reply.body
            );
            let version = if *address_type == "ipv6" { "IPv6" } else { "IPv4" };
            assert_eq!(
                reply.text("Message").map(|text| unescaped(&text)),
                Some(format!("The IP address '{id}' is not a valid {version} address")),
                "case {case:?}: unexpected refusal message in {}",
                reply.body
            );
            assert_eq!(
                registered_count(&router, &arn).await,
                0,
                "case {case:?}: a refused registration must store nothing"
            );
        }
    }
}

/// One row of the quota table: the case name, the group's fields, and how many targets
/// such a group holds.
type QuotaCase = (&'static str, &'static [(&'static str, &'static str)], usize);

/// The per-target-group quota over the families it differs along, each the published
/// per-Region default `DescribeAccountLimits` reports for that family: an Application Load
/// Balancer's group, a Network Load Balancer's, a Gateway Load Balancer's, and the two
/// target types that carry a single target.
///
/// KNOWN APPROXIMATION on two rows: AWS publishes the Network and Gateway numbers PER
/// Availability Zone, and this emulator models no per-zone accounting, so it applies them
/// to the group as a whole — never more permissive than the published quota, but a group
/// spread over several zones is refused sooner than AWS would refuse it. The rows below
/// assert the bound this service enforces and reports through `DescribeAccountLimits`,
/// not a per-zone one.
const QUOTA_CASES: &[QuotaCase] = &[
    (
        "a Network Load Balancer's group",
        &[("Protocol", "TCP"), ("Port", "80"), ("VpcId", "vpc-1"), ("TargetType", "ip")],
        500,
    ),
    (
        "a Gateway Load Balancer's group",
        &[
            ("Protocol", "GENEVE"),
            ("Port", "6081"),
            ("VpcId", "vpc-1"),
            ("TargetType", "ip"),
        ],
        300,
    ),
    (
        "a lambda target group",
        &[("TargetType", "lambda")],
        1,
    ),
    (
        "an alb target group",
        &[("Protocol", "TCP"), ("Port", "80"), ("VpcId", "vpc-1"), ("TargetType", "alb")],
        1,
    ),
];

#[tokio::test]
async fn a_target_group_holds_the_published_number_of_targets_for_its_family() {
    let (router, _engine) = emulator();

    for (index, (case, fields, quota)) in QUOTA_CASES.iter().enumerate() {
        let arn = create_ok(&router, case, &format!("quota-{index}"), fields).await;
        // The ids are addresses for an `ip` group and ARNs for the single-target types,
        // so each row registers what a group of its kind carries.
        let ids: Vec<String> = (0..=*quota)
            .map(|offset| match fields.iter().find(|(key, _)| *key == "TargetType") {
                Some((_, "ip")) => format!("10.{}.{}.{}", offset / 65_536, (offset / 256) % 256, offset % 256),
                Some((_, "lambda")) => format!(
                    "arn:aws:lambda:us-west-2:000000000000:function:quota-{index}-{offset}"
                ),
                _ => format!(
                    "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/q{index}-{offset}/{offset:016x}"
                ),
            })
            .collect();

        // Exactly the quota fits.
        let fitting: Vec<(&str, &str)> = ids[..*quota].iter().map(|id| (id.as_str(), "")).collect();
        let reply = register(&router, &arn, &fitting).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: registering exactly {quota} targets must be accepted, got {} \
             body {}",
            reply.status, reply.body
        );
        assert_eq!(
            registered_count(&router, &arn).await,
            *quota,
            "case {case:?}: every target of the accepted request must be registered"
        );

        // One more does not, and the refusal stores none of its targets.
        let past = register(&router, &arn, &[(ids[*quota].as_str(), "")]).await;
        assert_eq!(
            (past.status, past.error_code().as_str()),
            (400, "TooManyTargets"),
            "case {case:?}: one target past the quota must be refused, got {} body {}",
            past.status,
            past.body
        );
        // A targets quota is per target group, so the refusal names the group the
        // registration was asked of — read back from the ARN this row created rather than
        // from a name the suite fixed.
        let expected =
            format!("The maximum number of targets on target group '{arn}' has been reached");
        let stated = unescaped(&message(&past));
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
        assert_eq!(
            registered_count(&router, &arn).await,
            *quota,
            "case {case:?}: a refused registration must store nothing"
        );
    }
}

/// One row of the not-found table: the case name, the lookup's form fields, and the
/// message the refusal carries.
type NotFoundCase = (&'static str, Vec<(String, String)>, String);

#[tokio::test]
async fn a_not_found_refusal_names_the_identifiers_the_lookup_carried() {
    let (router, _engine) = emulator();
    let absent_tg = format!(
        "arn:aws:elasticloadbalancing:us-west-2:{}:targetgroup/ghost-tg/0123456789abcdef",
        crate::provisioning_loop::account()
    );

    let cases: Vec<NotFoundCase> = vec![
        (
            // A lookup by ONE identifier names that identifier.
            "DescribeTargetHealth of an absent target group",
            vec![
                p("Action", "DescribeTargetHealth"),
                p("Version", crate::provisioning_loop::VERSION),
                p("TargetGroupArn", &absent_tg),
            ],
            format!("Target groups '{absent_tg}' not found"),
        ),
        (
            // A lookup by a LIST member renders the list.
            "DescribeLoadBalancers by one absent name",
            vec![
                p("Action", "DescribeLoadBalancers"),
                p("Version", crate::provisioning_loop::VERSION),
                p("Names.member.1", "no-such-alb"),
            ],
            "Load balancers '[no-such-alb]' not found".to_string(),
        ),
        (
            "DescribeLoadBalancers by two absent names",
            vec![
                p("Action", "DescribeLoadBalancers"),
                p("Version", crate::provisioning_loop::VERSION),
                p("Names.member.1", "no-such-alb"),
                p("Names.member.2", "nor-this-one"),
            ],
            "Load balancers '[no-such-alb, nor-this-one]' not found".to_string(),
        ),
        (
            // A lookup by ARN names the ARN it could not resolve: both of this read's
            // selectors are refused in the same words, which is what the recorded answer to
            // a `DescribeLoadBalancers(LoadBalancerArns=[...])` of the ARN of a load balancer
            // that was deleted carries.
            "DescribeLoadBalancers by an absent ARN",
            vec![
                p("Action", "DescribeLoadBalancers"),
                p("Version", crate::provisioning_loop::VERSION),
                p(
                    "LoadBalancerArns.member.1",
                    // Well formed — this service's vendor, the Region the request is
                    // signed for, and the resource form it mints — so the read gets as
                    // far as the store and answers for the load balancer it did not find
                    // rather than for the shape of the ARN.
                    "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/ghost\
                     /0123456789abcdef",
                ),
            ],
            "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer\
             /app/ghost/0123456789abcdef]' not found"
                .to_string(),
        ),
    ];

    for (case, form, message) in cases {
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("Message").map(|text| unescaped(&text)),
            Some(message.clone()),
            "case {case:?}: expected the message {message:?}, got {}",
            reply.body
        );
    }

    // A lookup mixing a name that resolves with one that does not is refused on the one
    // that does not, and names only it.
    crate::provisioning_loop::create_http_target_group(&router, "present-tg").await;
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "present-alb"));
    form.push(p("Subnets.member.1", "subnet-0123456789abcdef0"));
    form.push(p("Subnets.member.2", "subnet-0123456789abcdef1"));
    let created = call(&router, &form).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateLoadBalancer answered {} body {}",
        created.status, created.body
    );
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", "present-alb"));
    form.push(p("Names.member.2", "no-such-alb"));
    let reply = call(&router, &form).await;
    assert_eq!(
        (
            reply.status,
            reply.error_code(),
            reply.text("Message").map(|text| unescaped(&text))
        ),
        (
            400,
            "LoadBalancerNotFound".to_string(),
            Some("Load balancers '[no-such-alb]' not found".to_string())
        ),
        "case a mixed lookup: expected the refusal to name only the absent name, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_load_balancer_takes_at_most_one_subnet_per_availability_zone() {
    let (router, _engine) = emulator();

    // (case, the load balancer type, how many subnets the request names, the awsQuery code
    // the request is refused under — empty for a placement that is accepted)
    //
    // "You can specify only one subnet per Availability Zone" (`Subnets`), so the ceiling
    // is the number of zones a Region has — which this service names with the 26 letters
    // a zone name can carry. A request past it names two subnets in one zone whatever its
    // subnets are, which is a placement no load balancer can hold — the modelled
    // `InvalidConfigurationRequest`. The FLOOR is the family's: a Network Load Balancer
    // "can specify subnets from one or more Availability Zones", while an Application Load
    // Balancer "must specify subnets from at least two" — a count the request itself has to
    // satisfy, refused as `ValidationError`.
    let rows: [(&str, &str, usize, &str); 5] = [
        ("one subnet on a network load balancer", "network", 1, ""),
        (
            "one subnet on an application load balancer",
            "application",
            1,
            "ValidationError",
        ),
        ("two subnets", "application", 2, ""),
        ("one subnet per zone of the region", "application", 26, ""),
        (
            "one subnet more than the region has zones",
            "application",
            27,
            "InvalidConfigurationRequest",
        ),
    ];

    for (index, (case, lb_type, count, code)) in rows.into_iter().enumerate() {
        let name = format!("subnet-bound-{index}");
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", &name));
        form.push(p("Type", lb_type));
        for member in 1..=count {
            form.push(p(
                &format!("Subnets.member.{member}"),
                &format!("subnet-{member:017x}"),
            ));
        }
        let reply = call(&router, &form).await;
        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the create to be accepted, got {} body {}",
                reply.status, reply.body
            );
            // Every subnet is in a zone of its own: no two entries report one zone name,
            // whether the zone was resolved or stood in for.
            let zones: Vec<String> = members(&reply.body, "AvailabilityZones")
                .iter()
                .filter_map(|zone| first_text(zone, "ZoneName"))
                .collect();
            let mut distinct = zones.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(
                (zones.len(), distinct.len()),
                (count, count),
                "case {case:?}: expected {count} zones, one per subnet, got {zones:?}"
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, code),
                "case {case:?}: expected 400 {code}, got {} body {}",
                reply.status,
                reply.body
            );
            let mut form = action("DescribeLoadBalancers");
            form.push(p("Names.member.1", &name));
            let read = call(&router, &form).await;
            assert_eq!(
                (read.status, read.error_code().as_str()),
                (400, "LoadBalancerNotFound"),
                "case {case:?}: a refused create must store no load balancer, got {}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_target_group_is_deleted_unless_a_listener_forwards_to_it() {
    let (router, _engine) = emulator();
    let arn = crate::provisioning_loop::create_http_target_group(&router, "delete-tg").await;

    // A target group no listener forwards to goes, and the read of it afterwards says so.
    let mut form = action("DeleteTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case an unreferenced target group: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let read = describe(&router, &arn).await;
    assert_eq!(
        (read.status, read.error_code().as_str()),
        (400, "TargetGroupNotFound"),
        "case an unreferenced target group: it must be gone, got {}",
        read.body
    );
    // Deleting it again is a success that removes nothing: the operation models no
    // not-found error.
    let again = call(&router, &form).await;
    assert_eq!(
        again.status, 200,
        "case deleting twice: expected 200, got {} body {}",
        again.status, again.body
    );

    // One a listener forwards to stays, and the refusal says so without naming it.
    let referenced = crate::provisioning_loop::create_http_target_group(&router, "in-use-tg").await;
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "delete-alb"));
    form.push(p("Subnets.member.1", "subnet-0123456789abcdef0"));
    form.push(p("Subnets.member.2", "subnet-0123456789abcdef1"));
    let created = call(&router, &form).await;
    let load_balancer = created
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("setup: no LoadBalancerArn in {}", created.body));
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &load_balancer));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &referenced));
    let listener = call(&router, &form).await;
    let listener_arn = listener
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("setup: no ListenerArn in {}", listener.body));

    let mut form = action("DeleteTargetGroup");
    form.push(p("TargetGroupArn", &referenced));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ResourceInUse"),
        "case a referenced target group: expected 400 ResourceInUse, got {} body {}",
        reply.status,
        reply.body
    );
    // The refusal is built from the REQUEST — the ARN the caller sent — and names no
    // holding listener or rule. The whole message is pinned, over the forms a reference
    // can take, in `all/target_group_in_use_refusal.rs`.
    assert!(
        reply
            .text("Message")
            .map(|text| unescaped(&text))
            .is_some_and(|message| !message.contains(&listener_arn)),
        "case a referenced target group: the refusal must not name the listener, got {}",
        reply.body
    );
    let still_there = describe(&router, &referenced).await;
    assert_eq!(
        first_text(&still_there.body, "TargetGroupArn").as_deref(),
        Some(referenced.as_str()),
        "case a referenced target group: a refused delete must remove nothing, got {}",
        still_there.body
    );
}
