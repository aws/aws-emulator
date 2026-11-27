// SPDX-License-Identifier: CC0-1.0

//! The PORT a target group's protocol fixes, over the wire.
//!
//! One rule: `Port` is "the port on which the targets receive traffic", and where the model
//! documents ONE supported port for a protocol — "if the protocol is GENEVE, the supported
//! port is 6081" (`CreateTargetGroupInput$Port`) — that protocol admits that port alone,
//! whatever the member's own `1..65535` range allows. A create on any other port is refused
//! the modelled `InvalidConfigurationRequest` and stores nothing; a protocol the model fixes
//! no port for takes every port in range, 6081 among them.
//!
//! The same sentence is on the member that OVERRIDES the group's port for one target — "the
//! port on which the target is listening. If the target group protocol is GENEVE, the
//! supported port is 6081" (`TargetDescription$Port`) — so the second test drives the rule
//! through `RegisterTargets`, where it is the modelled refusal for a target the group cannot
//! carry.
//!
//! Why: a target group whose traffic could never reach its targets is accepted at create time
//! and fails only when traffic is sent, so a client's negative control passes where the
//! service refuses the configuration.
//!
//! The rows span what the rule generalises over: the protocol (the one with a fixed port, and
//! protocols from the other two families), and the port — the fixed one, its neighbours on
//! either side, a port of another protocol's traffic, and each published bound of the member's
//! own range.
//!
//! The wire harness (`router`, `call`, `action`, `p`) comes from `wire` and the message reader
//! from `target_group_attributes_and_modification`; nothing here restates them.

use smithy_server_runtime::MultiServiceRouter;

use crate::target_group_attributes_and_modification::unescaped;
use crate::wire::{action, call, p, router, Reply};

/// The awsQuery code a pairing of members no target group can hold is refused with
/// (`InvalidConfigurationRequestException`), the code a target the group cannot carry is
/// refused with (`InvalidTargetException`), and the status both are answered under.
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";
const INVALID_TARGET: &str = "InvalidTarget";
const REFUSED_STATUS: u16 = 400;

/// The code a read of a target group nothing holds is refused with, which is what says a
/// refused create stored nothing.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// The protocol the model fixes one port for, and that port: "For Gateway Load Balancers, the
/// supported protocol is GENEVE" (`CreateTargetGroupInput$Protocol`) and "if the protocol is
/// GENEVE, the supported port is 6081" (`member="Port"`). Written out here rather than read
/// from the crate, so the VALUES the service holds the request to are under test and not only
/// their propagation.
const FIXED_PORT_PROTOCOL: &str = "GENEVE";
const FIXED_PORT: &str = "6081";

/// The bounds of `Port`'s own range (`smithy_query(query="operation",
/// operation="CreateTargetGroup")`: `Port` is `range 1..65535`), so a row can ask for a port
/// the range admits and the protocol does not.
const LOWEST_PORT: &str = "1";
const HIGHEST_PORT: &str = "65535";

/// One row: the case, the `Protocol` and `Port` the create names, and whether it is served.
struct Row {
    case: &'static str,
    protocol: &'static str,
    port: &'static str,
    accepted: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "the protocol's own port",
        protocol: FIXED_PORT_PROTOCOL,
        port: FIXED_PORT,
        accepted: true,
    },
    Row {
        case: "one port above the protocol's own",
        protocol: FIXED_PORT_PROTOCOL,
        port: "6082",
        accepted: false,
    },
    Row {
        case: "one port below the protocol's own",
        protocol: FIXED_PORT_PROTOCOL,
        port: "6080",
        accepted: false,
    },
    Row {
        case: "a port another protocol's traffic uses",
        protocol: FIXED_PORT_PROTOCOL,
        port: "80",
        accepted: false,
    },
    Row {
        case: "the lowest port the member's range admits",
        protocol: FIXED_PORT_PROTOCOL,
        port: LOWEST_PORT,
        accepted: false,
    },
    Row {
        case: "the highest port the member's range admits",
        protocol: FIXED_PORT_PROTOCOL,
        port: HIGHEST_PORT,
        accepted: false,
    },
    Row {
        case: "a network protocol on the fixed port of another protocol",
        protocol: "TCP",
        port: FIXED_PORT,
        accepted: true,
    },
    Row {
        case: "a network protocol on a port no protocol fixes",
        protocol: "TCP",
        port: "6082",
        accepted: true,
    },
    Row {
        case: "an application protocol on its usual port",
        protocol: "HTTP",
        port: "80",
        accepted: true,
    },
    Row {
        case: "a datagram protocol at the highest port the range admits",
        protocol: "UDP",
        port: HIGHEST_PORT,
        accepted: true,
    },
];

/// The sentence a create on a port the protocol does not support is refused with: the port it
/// must use, and the protocol that fixes it.
fn wrong_port(protocol: &str, port: &str) -> String {
    format!("Port must be {port} for {protocol} target groups.")
}

/// Ask `CreateTargetGroup` for a group called `name` carrying `protocol` on `port`.
async fn create(router: &MultiServiceRouter, name: &str, protocol: &str, port: &str) -> Reply {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", protocol));
    form.push(p("Port", port));
    form.push(p("VpcId", "vpc-12345678"));
    call(router, &form).await
}

/// What `DescribeTargetGroups` answers for the target group called `name`.
async fn describe(router: &MultiServiceRouter, name: &str) -> Reply {
    let mut form = action("DescribeTargetGroups");
    form.push(p("Names.member.1", name));
    call(router, &form).await
}

#[tokio::test]
async fn a_target_group_takes_only_a_port_its_protocol_supports() {
    let router = router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("protocol-port-{index}");
        let created = create(&router, &name, row.protocol, row.port).await;

        if !row.accepted {
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (REFUSED_STATUS, INVALID_CONFIGURATION),
                "case {case:?}: expected {REFUSED_STATUS} {INVALID_CONFIGURATION}, got {} body {}",
                created.status,
                created.body
            );
            let stated = created
                .text("Message")
                .map(|text| unescaped(&text))
                .unwrap_or_else(|| panic!("case {case:?}: no message in {}", created.body));
            let expected = wrong_port(row.protocol, FIXED_PORT);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
            // A refused create stores nothing: the name it asked for is held by no group.
            let read = describe(&router, &name).await;
            assert_eq!(
                (read.status, read.error_code().as_str()),
                (REFUSED_STATUS, TARGET_GROUP_NOT_FOUND),
                "case {case:?}: a refused create must store no target group, got {} body {}",
                read.status,
                read.body
            );
            continue;
        }

        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} body {}",
            created.status, created.body
        );
        // The pairing the create stored is what a later read reports back.
        let read = describe(&router, &name).await;
        assert_eq!(
            (
                read.status,
                read.text("Protocol").as_deref(),
                read.text("Port").as_deref()
            ),
            (200, Some(row.protocol), Some(row.port)),
            "case {case:?}: expected the stored pairing to be reported, got body {}",
            read.body
        );
    }
}

/// Every registration of a target group's own protocol is held to the same port: the group's
/// port is what a target takes by default, an override to that port is accepted, and an
/// override to any other port is refused the modelled `InvalidTarget` — and registers nothing.
#[tokio::test]
async fn a_registration_takes_only_a_port_the_groups_protocol_supports() {
    let router = router();

    let name = "protocol-port-registration";
    // Targets registered BY ADDRESS, so the registrations below name an address of the
    // group's network rather than an instance of an inventory nothing holds.
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", FIXED_PORT_PROTOCOL));
    form.push(p("Port", FIXED_PORT));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", "vpc-12345678"));
    let created = call(&router, &form).await;
    assert_eq!(
        created.status, 200,
        "case the group under test: expected the create to be served, got {} body {}",
        created.status, created.body
    );
    let arn = created
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case the group under test: no ARN in {}", created.body));

    // (the case, the port the registration names, whether it is accepted)
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("a target naming no port, which takes the group's", None, true),
        ("a target on the group's own port", Some(FIXED_PORT), true),
        ("a target one port above it", Some("6082"), false),
        ("a target one port below it", Some("6080"), false),
        ("a target at the highest port the range admits", Some(HIGHEST_PORT), false),
    ];

    for (index, (case, port, accepted)) in rows.iter().enumerate() {
        let target = format!("10.0.1.{}", index + 10);
        let mut form = action("RegisterTargets");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("Targets.member.1.Id", &target));
        if let Some(port) = port {
            form.push(p("Targets.member.1.Port", port));
        }
        let registered = call(&router, &form).await;

        if *accepted {
            assert_eq!(
                registered.status, 200,
                "case {case:?}: expected the registration to be served, got {} body {}",
                registered.status, registered.body
            );
        } else {
            assert_eq!(
                (registered.status, registered.error_code().as_str()),
                (REFUSED_STATUS, INVALID_TARGET),
                "case {case:?}: expected {REFUSED_STATUS} {INVALID_TARGET}, got {} body {}",
                registered.status,
                registered.body
            );
            let stated = registered
                .text("Message")
                .map(|text| unescaped(&text))
                .unwrap_or_else(|| panic!("case {case:?}: no message in {}", registered.body));
            let expected = wrong_port(FIXED_PORT_PROTOCOL, FIXED_PORT);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }

        // What the group HOLDS after each row, read back through the health of its
        // registrations: a refused registration left none of its own behind.
        let mut health = action("DescribeTargetHealth");
        health.push(p("TargetGroupArn", &arn));
        let reported = call(&router, &health).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: expected 200 from DescribeTargetHealth, got {} body {}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.body.contains(&target),
            *accepted,
            "case {case:?}: expected the group to hold {target} only where the registration was \
             served, got body {}",
            reported.body
        );
    }
}
