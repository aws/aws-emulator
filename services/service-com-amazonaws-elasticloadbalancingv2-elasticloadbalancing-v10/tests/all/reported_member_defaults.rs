// SPDX-License-Identifier: CC0-1.0

//! The members every answer of these reads CARRIES, over the wire.
//!
//! Three readings of one rule: a member whose value AWS always has for the resource is
//! reported on every answer, never left out for the client to supply itself.
//!
//! * `LoadBalancer$EnablePrefixForIpv6SourceNat` has a documented default ("the default
//!   value is `off`"), so every load balancer reports it — and reports what a caller set
//!   once one of the two operations the model gives the member (`CreateLoadBalancer`,
//!   `SetSubnets`) sets it.
//! * `DescribeCapacityReservationResult$DecreaseRequestsRemaining` is the account's
//!   allowance for the DAY, so it is reported for every load balancer, including one that
//!   reserves no capacity and so has nothing to decrease.
//! * `TargetHealthDescription$HealthCheckPort` ("the port to use to connect with the
//!   target") follows the target group's health-check configuration read against the
//!   target the request named, which every described target has — a registered one and an
//!   unregistered one alike.
//!
//! The DNS name a load balancer publishes is the other half of this issue and has its own
//! suite (`load_balancer_dns_name_form.rs`).
//!
//! The wire harness (`emulator`, `call`, `action`, `p`, `members`, `first_text`) comes from
//! `provisioning_loop` and the load balancer creator from `load_balancer_attributes`;
//! nothing here restates either.

use crate::load_balancer_attributes::create_load_balancer;
use crate::provisioning_loop::{action, call, emulator, first_text, members, p, Reply};
use smithy_server_runtime::MultiServiceRouter;

/// The value `EnablePrefixForIpv6SourceNat`'s own documentation fixes as its default: "The
/// default value is `off`" (one of the two values the model's
/// `EnablePrefixForIpv6SourceNatEnum` names). Written out here rather than read from the
/// crate, so the VALUE the service reports is under test and not only its propagation.
const DOCUMENTED_SOURCE_NAT_DEFAULT: &str = "off";

/// The allowance the published Elastic Load Balancing quota gives an account for
/// capacity-reservation decreases per load balancer per day, written out here for the same
/// reason.
const DOCUMENTED_DAILY_DECREASES: &str = "2";

/// One load balancer to make: the case name and the fields it is created with.
type LoadBalancerCase = (&'static str, &'static [(&'static str, &'static str)]);

/// The load balancers these tables run against: every family the model's
/// `LoadBalancerTypeEnum` names (`application`, `network`, `gateway`), both schemes its
/// `LoadBalancerSchemeEnum` names for the families that carry one, and address types and
/// placements of one, two and three zones — none of which the rules below depend on, which is
/// what the rows show.
const LOAD_BALANCERS: &[LoadBalancerCase] = &[
    ("an internet-facing application load balancer", &[]),
    (
        "an internal application load balancer",
        &[("Scheme", "internal")],
    ),
    (
        "an application load balancer in three zones",
        &[
            ("Subnets.member.1", "subnet-11111111"),
            ("Subnets.member.2", "subnet-22222222"),
            ("Subnets.member.3", "subnet-33333333"),
        ],
    ),
    (
        "a network load balancer in one zone",
        &[("Type", "network"), ("Subnets.member.1", "subnet-11111111")],
    ),
    (
        "a dualstack network load balancer",
        &[
            ("Type", "network"),
            ("IpAddressType", "dualstack"),
            ("Subnets.member.1", "subnet-11111111"),
        ],
    ),
    (
        // A Gateway Load Balancer carries no scheme at all — "you can't specify a scheme for
        // a Gateway Load Balancer" — so this row names none.
        "a gateway load balancer",
        &[("Type", "gateway"), ("IpAddressType", "ipv4")],
    ),
];

/// What `DescribeLoadBalancers` reports for the load balancer `arn` names.
async fn describe(router: &MultiServiceRouter, case: &str, arn: &str) -> Reply {
    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from the read, got {} body {}",
        reply.status, reply.body
    );
    reply
}

/// Every load balancer reports `EnablePrefixForIpv6SourceNat` at its documented default
/// until a caller sets it — on the create's own answer and on every later read.
#[tokio::test]
async fn every_load_balancer_reports_the_source_nat_prefix_default() {
    let (router, _engine) = emulator();

    for (index, (case, fields)) in LOAD_BALANCERS.iter().enumerate() {
        let name = format!("lb-source-nat-{index}");
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", &name));
        if !fields.iter().any(|(key, _)| key.starts_with("Subnets")) {
            form.push(p("Subnets.member.1", "subnet-11111111"));
            form.push(p("Subnets.member.2", "subnet-22222222"));
        }
        for (key, value) in fields.iter() {
            form.push(p(key, value));
        }
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} body {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("EnablePrefixForIpv6SourceNat").as_deref(),
            Some(DOCUMENTED_SOURCE_NAT_DEFAULT),
            "case {case:?}: the create must answer the documented default, got body {}",
            created.body
        );
        let arn = created
            .text("LoadBalancerArn")
            .unwrap_or_else(|| panic!("case {case:?}: no LoadBalancerArn in {}", created.body));
        let read = describe(&router, case, &arn).await;
        assert_eq!(
            read.text("EnablePrefixForIpv6SourceNat").as_deref(),
            Some(DOCUMENTED_SOURCE_NAT_DEFAULT),
            "case {case:?}: a read must report the documented default, got body {}",
            read.body
        );
    }
}

/// Once a caller SETS the member, what every read reports is what it set — the default is
/// the value until one does, not a value the service pins.
///
/// Both operations the model gives the member to take it: `CreateLoadBalancer`, which
/// starts the load balancer at the value the request names, and `SetSubnets`, which changes
/// it afterwards. The values are the two the model's `EnablePrefixForIpv6SourceNatEnum`
/// names (`on`, `off`), set in turn on one dualstack Network Load Balancer: `on` is that
/// family's alone, and only at the `dualstack` address type the member's documentation
/// requires — which is why the last row, asking for it on a load balancer that is not
/// dualstack, is refused and stores nothing.
#[tokio::test]
async fn the_source_nat_prefix_follows_what_a_caller_sets() {
    let (router, _engine) = emulator();

    // The create takes the member: a load balancer starts at what the request named.
    let arn = create_load_balancer(
        &router,
        "lb-source-nat-created-on",
        &[
            ("Type", "network"),
            ("IpAddressType", "dualstack"),
            ("Subnets.member.1", "subnet-11111111"),
            ("EnablePrefixForIpv6SourceNat", "on"),
        ],
    )
    .await;
    assert_eq!(
        describe(&router, "created with the prefix on", &arn)
            .await
            .text("EnablePrefixForIpv6SourceNat")
            .as_deref(),
        Some("on"),
        "case \"created with the prefix on\": a read must report what the create named"
    );

    // And the create is refused where the member's documented pairing is not met, exactly
    // as `SetSubnets` refuses it — so nothing is stored for that name.
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "lb-source-nat-not-dualstack"));
    form.push(p("Type", "network"));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("EnablePrefixForIpv6SourceNat", "on"));
    let refused = call(&router, &form).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "InvalidConfigurationRequest"),
        "case \"created with the prefix on and no dualstack\": expected the modelled \
         refusal, got {} body {}",
        refused.status,
        refused.body
    );
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", "lb-source-nat-not-dualstack"));
    let gone = call(&router, &form).await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "case \"created with the prefix on and no dualstack\": a refused create stores \
         nothing, got {} body {}",
        gone.status,
        gone.body
    );

    for (case, value) in [
        ("turned off", "off"),
        ("turned on again", "on"),
        ("turned off once more", "off"),
    ] {
        let mut form = action("SetSubnets");
        form.push(p("LoadBalancerArn", &arn));
        form.push(p("Subnets.member.1", "subnet-11111111"));
        form.push(p("EnablePrefixForIpv6SourceNat", value));
        let set = call(&router, &form).await;
        assert_eq!(
            set.status, 200,
            "case {case:?}: expected 200 from SetSubnets, got {} body {}",
            set.status, set.body
        );
        let read = describe(&router, case, &arn).await;
        assert_eq!(
            read.text("EnablePrefixForIpv6SourceNat").as_deref(),
            Some(value),
            "case {case:?}: a read must report what the caller set, got body {}",
            read.body
        );
    }
}

/// Every load balancer reports the account's daily capacity-decrease allowance, whatever
/// its family, scheme or placement and with no capacity reserved for it.
#[tokio::test]
async fn every_load_balancer_reports_the_daily_decrease_allowance() {
    let (router, _engine) = emulator();

    for (index, (case, fields)) in LOAD_BALANCERS.iter().enumerate() {
        let arn = create_load_balancer(&router, &format!("lb-decreases-{index}"), fields).await;
        let mut form = action("DescribeCapacityReservation");
        form.push(p("LoadBalancerArn", &arn));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("DecreaseRequestsRemaining").as_deref(),
            Some(DOCUMENTED_DAILY_DECREASES),
            "case {case:?}: expected the whole daily allowance, got body {}",
            reply.body
        );
    }
}

/// One row of the health-check-port table: the case, the group's own `Port`, the
/// `HealthCheckPort` it is created with (empty for a group that names none), the port a
/// described target carries (`None` for one named without a port), and the port every
/// description must then report.
struct PortRow {
    case: &'static str,
    group_port: &'static str,
    health_check_port: &'static str,
    target_port: Option<&'static str>,
    reported: &'static str,
}

/// The rows span what the rule generalises over: whether the group names a health-check
/// port at all, whether it names the literal `traffic-port` its documentation gives as the
/// default, a port of its own, and whether the TARGET carries a port the group's own does
/// not — each read for a target that is registered and for one that is not.
const PORT_ROWS: &[PortRow] = &[
    PortRow {
        case: "no health check port named, target on the group's port",
        group_port: "80",
        health_check_port: "",
        target_port: None,
        reported: "80",
    },
    PortRow {
        case: "the literal traffic-port default, target on the group's port",
        group_port: "8080",
        health_check_port: "traffic-port",
        target_port: None,
        reported: "8080",
    },
    PortRow {
        case: "the literal traffic-port default, target on a port of its own",
        group_port: "8080",
        health_check_port: "traffic-port",
        target_port: Some("9090"),
        reported: "9090",
    },
    PortRow {
        case: "a health check port of the group's own, target on the group's port",
        group_port: "80",
        health_check_port: "8443",
        target_port: None,
        reported: "8443",
    },
    PortRow {
        case: "a health check port of the group's own, target on a port of its own",
        group_port: "80",
        health_check_port: "8443",
        target_port: Some("9090"),
        reported: "8443",
    },
    PortRow {
        case: "the lowest port a health check runs against",
        group_port: "80",
        health_check_port: "1",
        target_port: None,
        reported: "1",
    },
    PortRow {
        case: "the highest port a health check runs against",
        group_port: "80",
        health_check_port: "65535",
        target_port: Some("9090"),
        reported: "65535",
    },
];

/// A target group of `row`, created over the wire; answers its ARN.
async fn create_group(router: &MultiServiceRouter, name: &str, row: &PortRow) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", row.group_port));
    form.push(p("VpcId", "vpc-0abc1234"));
    form.push(p("TargetType", "instance"));
    if !row.health_check_port.is_empty() {
        form.push(p("HealthCheckPort", row.health_check_port));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected the group to be created, got {} body {}",
        row.case, reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body))
}

/// The `(Id, HealthCheckPort)` pair of every description `DescribeTargetHealth` answers
/// for `arn`, asked about the target `target` names (`None` for a read that names none).
async fn described_ports(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    target: Option<(&str, Option<&str>)>,
) -> Vec<(String, String)> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    if let Some((id, port)) = target {
        form.push(p("Targets.member.1.Id", id));
        if let Some(port) = port {
            form.push(p("Targets.member.1.Port", port));
        }
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from the health read, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            (
                first_text(description, "Id").unwrap_or_default(),
                first_text(description, "HealthCheckPort").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every described target carries the port a health check of it runs against — the group's
/// health-check configuration read against that target — whether or not the group holds a
/// registration for it.
#[tokio::test]
async fn every_described_target_reports_its_health_check_port() {
    let (router, _engine) = emulator();

    for (index, row) in PORT_ROWS.iter().enumerate() {
        let arn = create_group(&router, &format!("tg-check-port-{index}"), row).await;
        let registered = format!("i-0aaaaaaaaaaaaaa{index:02x}");
        let never = format!("i-0ffffffffffffff{index:02x}");

        let mut form = action("RegisterTargets");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("Targets.member.1.Id", &registered));
        if let Some(port) = row.target_port {
            form.push(p("Targets.member.1.Port", port));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected the registration to be served, got {} body {}",
            row.case, reply.status, reply.body
        );

        // The registered target, read both by the unfiltered list and by name.
        let wanted = vec![(registered.clone(), row.reported.to_string())];
        let listed = described_ports(&router, row.case, &arn, None).await;
        assert_eq!(
            listed, wanted,
            "case {:?}: the unfiltered read must report the health check port, expected \
             {wanted:?}",
            row.case
        );
        let named = described_ports(
            &router,
            row.case,
            &arn,
            Some((registered.as_str(), row.target_port)),
        )
        .await;
        assert_eq!(
            named, wanted,
            "case {:?}: a read naming the registered target must report the same port, \
             expected {wanted:?}",
            row.case
        );

        // The target the group holds NO registration for: the same port, because the port
        // follows the group's configuration and the target the request named.
        let unregistered = described_ports(
            &router,
            row.case,
            &arn,
            Some((never.as_str(), row.target_port)),
        )
        .await;
        assert_eq!(
            unregistered,
            vec![(never.clone(), row.reported.to_string())],
            "case {:?}: an unregistered target carries the same health check port",
            row.case
        );
    }
}
