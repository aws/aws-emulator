// SPDX-License-Identifier: CC0-1.0

//! Which PROTOCOL a target group may carry for the kind of target it holds, driven over
//! the wire exactly as a client drives it.
//!
//! One rule is asserted here, over a SET of inputs rather than over the one call a
//! recording happens to carry: `TargetType` says what a target group's members ARE —
//! "`instance` - Register targets by instance ID … `ip` - Register targets by IP address …
//! `lambda` - Register a single Lambda function as a target … `alb` - Register a single
//! Application Load Balancer as a target" — and `Protocol` says how traffic reaches them,
//! so a create may only pair a target type with a protocol Elastic Load Balancing can
//! forward to that kind of target. An Application Load Balancer registered as a target
//! terminates the application protocol itself, so the load balancer in front of it passes
//! TCP connections to it and no other protocol pairs with that target type; a host target
//! (`instance`, `ip`) is reached over every protocol a target group can carry. A pairing
//! outside that is the modelled `InvalidConfigurationRequest`, naming the target type and
//! the protocol the request asked for.
//!
//! The rows differ along what the rule generalises over: every `TargetTypeEnum` value
//! crossed with every `ProtocolEnum` value, plus the target type whose targets are invoked
//! rather than connected to, which carries no protocol at all. Each accepted row is read
//! back through `DescribeTargetGroups(Names=[…])` — the lookup a client has when it only
//! holds the name it chose — and each refused row is read back the same way, to prove a
//! refused create stored nothing.

use crate::provisioning_loop::{action, call, emulator, p, Reply};
use crate::target_group_attributes_and_modification::unescaped;
use crate::target_group_health_check_schedule::describe_by_name;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    check_target_type_protocol, protocols_for_target_type, LOAD_BALANCER_TARGET_TYPE,
};

/// The awsQuery code of `InvalidConfigurationRequestException`, which `CreateTargetGroup`
/// models (`smithy_query(query="operation",operation="CreateTargetGroup")`).
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The code `DescribeTargetGroups` reports a name nothing holds under
/// (`TargetGroupNotFoundException`).
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// Every value of `ProtocolEnum`
/// (`smithy_query(query="shape",shape="ProtocolEnum")`), which is the protocol catalog a
/// target group's `Protocol` is drawn from.
const PROTOCOLS: [&str; 9] = [
    "GENEVE", "HTTP", "HTTPS", "QUIC", "TCP", "TCP_QUIC", "TCP_UDP", "TLS", "UDP",
];

/// Every value of `TargetTypeEnum`
/// (`smithy_query(query="shape",shape="TargetTypeEnum")`) whose targets receive traffic on
/// a protocol, which is what this rule is about. `lambda` — the one value left out — is
/// invoked rather than connected to and carries no protocol at all; it is its own row
/// below.
const TARGET_TYPES: [&str; 3] = [LOAD_BALANCER_TARGET_TYPE, "instance", "ip"];

/// The one protocol a target that is itself an Application Load Balancer is reached over.
const LOAD_BALANCER_TARGET_PROTOCOL: &str = "TCP";

/// The refusal `reply` carries: its status, its code and its message, with the serializer's
/// escaping undone, so one assertion names all three.
fn refusal(reply: &Reply) -> (u16, String, String) {
    (
        reply.status,
        reply.error_code(),
        unescaped(&reply.text("Message").unwrap_or_default()),
    )
}

/// Every pairing of a target type whose targets receive traffic with a protocol a target
/// group can carry: the pairing the service can forward is served and reported, and every
/// other one is refused with the modelled `InvalidConfigurationRequest` and stores nothing.
#[tokio::test]
async fn a_target_group_carries_only_a_protocol_its_target_type_is_reached_over() {
    let (router, _engine) = emulator();

    for (type_index, target_type) in TARGET_TYPES.iter().enumerate() {
        for (protocol_index, protocol) in PROTOCOLS.iter().enumerate() {
            let case = format!("a {target_type} target group with the {protocol} protocol");
            let name = format!("ttp-{type_index}-{protocol_index}");
            let mut form = action("CreateTargetGroup");
            form.push(p("Name", &name));
            form.push(p("TargetType", target_type));
            form.push(p("Protocol", protocol));
            // "If the protocol is GENEVE, the supported port is 6081"
            // (`CreateTargetGroupInput$Port`) — every other protocol takes any port the
            // member's range admits, so the row sends the port its protocol supports and
            // what it varies stays the PAIRING under test.
            form.push(p("Port", if *protocol == "GENEVE" { "6081" } else { "8080" }));
            form.push(p("VpcId", "vpc-12345678"));
            let reply = call(&router, &form).await;

            let admitted = *target_type != LOAD_BALANCER_TARGET_TYPE
                || *protocol == LOAD_BALANCER_TARGET_PROTOCOL;
            if admitted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected the create to be served, got {} body {}",
                    reply.status, reply.body
                );
                // The pairing the create stored is what a read reports back, and the read
                // resolves the target group by the NAME the caller chose.
                let described = describe_by_name(&router, &name).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: expected the stored target group to be reported, got {} \
                     body {}",
                    described.status, described.body
                );
                assert_eq!(
                    (
                        described.text("TargetType"),
                        described.text("Protocol"),
                        described.text("TargetGroupName")
                    ),
                    (
                        Some(target_type.to_string()),
                        Some(protocol.to_string()),
                        Some(name.clone())
                    ),
                    "case {case:?}: expected the stored pairing to be reported, got body {}",
                    described.body
                );
                continue;
            }

            let expected = format!(
                "Target type '{target_type}' is not supported for target groups with \
                 {protocol} protocol"
            );
            assert_eq!(
                refusal(&reply),
                (
                    400,
                    INVALID_CONFIGURATION.to_string(),
                    expected.clone()
                ),
                "case {case:?}: expected 400 {INVALID_CONFIGURATION} {expected:?}, got {}",
                reply.body
            );
            // A refused create writes nothing: the name it asked for is held by nothing.
            let described = describe_by_name(&router, &name).await;
            assert_eq!(
                (described.status, described.error_code()),
                (400, TARGET_GROUP_NOT_FOUND.to_string()),
                "case {case:?}: a refused create must store nothing, got {} body {}",
                described.status,
                described.body
            );
        }
    }
}

/// The target type whose targets are INVOKED rather than connected to carries no protocol
/// at all ("if the target is a Lambda function, this parameter does not apply"), so it
/// pairs with no protocol and is served without one.
#[tokio::test]
async fn a_lambda_target_group_carries_no_protocol() {
    let (router, _engine) = emulator();

    let case = "a lambda target group with no protocol";
    let name = "ttp-lambda";
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("TargetType", "lambda"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected the create to be served, got {} body {}",
        reply.status, reply.body
    );

    let described = describe_by_name(&router, name).await;
    assert_eq!(
        (
            described.status,
            described.text("TargetType"),
            described.text("Protocol")
        ),
        (200, Some("lambda".to_string()), None),
        "case {case:?}: expected a protocol-less target group to be reported, got {} body {}",
        described.status,
        described.body
    );
}

/// The reading the create applies, asserted directly over the catalog: only the target type
/// that holds an Application Load Balancer bounds the protocol, and it bounds it to the one
/// protocol such a target is reached over.
#[test]
fn only_the_load_balancer_target_type_bounds_the_protocol() {
    for target_type in ["instance", "ip", "lambda"] {
        assert_eq!(
            protocols_for_target_type(target_type),
            None,
            "case a {target_type} target: expected no protocol bound of its own, got {:?}",
            protocols_for_target_type(target_type)
        );
        for protocol in PROTOCOLS {
            assert_eq!(
                check_target_type_protocol(target_type, Some(protocol)),
                Ok(()),
                "case a {target_type} target over {protocol}: expected the pairing to hold"
            );
        }
    }
    assert_eq!(
        protocols_for_target_type(LOAD_BALANCER_TARGET_TYPE),
        Some([LOAD_BALANCER_TARGET_PROTOCOL].as_slice()),
        "case an {LOAD_BALANCER_TARGET_TYPE} target: expected only \
         {LOAD_BALANCER_TARGET_PROTOCOL}, got {:?}",
        protocols_for_target_type(LOAD_BALANCER_TARGET_TYPE)
    );
    // A create that names no protocol is judged by neither rule: what such a target group
    // carries is settled elsewhere.
    assert_eq!(
        check_target_type_protocol(LOAD_BALANCER_TARGET_TYPE, None),
        Ok(()),
        "case an {LOAD_BALANCER_TARGET_TYPE} target with no protocol: expected no refusal of \
         the pairing"
    );
}
