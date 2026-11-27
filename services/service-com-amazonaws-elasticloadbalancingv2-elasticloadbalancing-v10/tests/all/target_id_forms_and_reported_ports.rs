// SPDX-License-Identifier: CC0-1.0

//! The FORM a target id has to be written in, on every operation that takes a `Targets`
//! list, and the PORT a reported target description carries.
//!
//! Two rules, each asserted over a set of inputs:
//!
//! * THE FORM OF AN ID. What a target id names depends on the target group's own
//!   `TargetType` (`smithy_query(query="doc",shape="TargetDescription",member="Id")`: "if
//!   the target type of the target group is instance, specify an instance ID. If the target
//!   type is ip, specify an IP address. If the target type is lambda, specify the ARN of the
//!   Lambda function. If the target type is alb, specify the ARN of the Application Load
//!   Balancer target"), and a value of none of those forms names no target of any account.
//!   So it is refused — `ValidationError`, naming the value and the form it failed to take
//!   — whatever is being ASKED of it: the deregistration that removes a target is judged
//!   exactly as the registration that adds one, because a value that could never have been
//!   registered is not the no-op a well-formed target that was simply never registered
//!   earns.
//! * THE PORT A REPORTED TARGET CARRIES. `TargetGroup$Port` is "the port on which the
//!   targets receive traffic", which a registration may override ("you can override the
//!   port for a target when you register it"), so the port of a reported target is the one
//!   the request named, else the target group's own — for a target that is registered and
//!   for one that is not, since the port is what decides WHICH registration a request is
//!   about. It follows from the group and the request alone, so a target group whose target
//!   type carries no port of its own (`TargetDescription$Port` "is not used if the target is
//!   a Lambda function") reports none, and
//!   `TargetHealthDescription$HealthCheckPort` is the group's health-check configuration read
//!   against that same target: the configured port, else the port that target receives
//!   traffic on.
//!
//! The rows differ along what the rules generalise over: the group's target type, the form
//! the offending id takes (an id of another target type, a near miss of the right form, a
//! value of no form at all), whether a well-formed id was ever registered, whether the
//! request names a port of its own, the group's own port (the default, a port no default
//! picks, each published bound of the modelled port range, and a group that has none),
//! whether the group configures a health-check port of its own, and how many targets one
//! read names.

use crate::provisioning_loop::{
    account, action, call, emulator, first_text, health_block, members, p, Reply, REGION,
};
use crate::target_group_attributes_and_modification::unescaped;
use crate::target_group_health_check_port::create_group;
use smithy_server_runtime::MultiServiceRouter;

/// The error code this service answers a member whose form the model states in prose with.
const VALIDATION_ERROR: &str = "ValidationError";

/// One row: the case, the group's `TargetType`, the id the request carries, and the
/// sentence the refusal states.
struct FormRow {
    case: &'static str,
    target_type: &'static str,
    id: &'static str,
    message: &'static str,
}

/// Every KIND of value that is no target of the group it is handed to: the id form of
/// ANOTHER target type, a near miss of the right form, and a value of no form at all.
///
/// The sentences are the ones the service states for each form — "The IP address '<id>' is
/// not a valid IPv4 address" for an `ip` group, "Instance ID '<id>' is not valid" for an
/// `instance` group — each with the value from the request as its only variable part.
const FORM_ROWS: &[FormRow] = &[
    FormRow {
        case: "an instance id handed to an ip target group",
        target_type: "ip",
        id: "i-0123456789abcdef0",
        message: "The IP address 'i-0123456789abcdef0' is not a valid IPv4 address",
    },
    FormRow {
        case: "a host name handed to an ip target group",
        target_type: "ip",
        id: "target.example.com",
        message: "The IP address 'target.example.com' is not a valid IPv4 address",
    },
    FormRow {
        case: "an address with a prefix length handed to an ip target group",
        target_type: "ip",
        id: "10.0.0.7/32",
        message: "The IP address '10.0.0.7/32' is not a valid IPv4 address",
    },
    FormRow {
        case: "an address handed to an instance target group",
        target_type: "instance",
        id: "10.0.0.7",
        message: "Instance ID '10.0.0.7' is not valid",
    },
    FormRow {
        case: "an instance id of the wrong length",
        target_type: "instance",
        id: "i-123",
        message: "Instance ID 'i-123' is not valid",
    },
    FormRow {
        case: "an instance id with a character no instance id carries",
        target_type: "instance",
        id: "i-0123456789abcdefg",
        message: "Instance ID 'i-0123456789abcdefg' is not valid",
    },
];

/// One target as `DescribeTargetHealth` reports it: its id, the port traffic to it goes
/// to, the port its health check runs against, and the health state and reason it is under.
///
/// `pub(crate)` with [`reported_for`] and [`target_group_on`] because the sibling suite that
/// asks the same rule of a read naming SEVERAL targets
/// (`crate::health_read_reported_target_port`) reads a health row exactly so: one harness
/// shared by these two suites, so neither can drift on how a row is read or on how the group
/// under it is created.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReportedTarget {
    pub(crate) id: Option<String>,
    pub(crate) port: Option<String>,
    pub(crate) health_check_port: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) reason: Option<String>,
}

/// The status, code and message of a refusal, as a caller reads them.
fn refusal(reply: &Reply) -> (u16, String, String) {
    (
        reply.status,
        reply.error_code(),
        unescaped(&reply.text("Message").unwrap_or_default()),
    )
}

/// A target group of `target_type` on port 80, the shape each form row starts from.
async fn target_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    target_type: &str,
) -> String {
    target_group_on(router, case, name, target_type, Some("80"), None).await
}

/// A target group of `target_type` on `port`, checking `health_check_port` — the shape the
/// port rows vary. A group whose target type carries no port of its own is in no network and
/// speaks no protocol, so it names neither.
pub(crate) async fn target_group_on(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    target_type: &str,
    port: Option<&str>,
    health_check_port: Option<&str>,
) -> String {
    let mut fields = vec![p("TargetType", target_type)];
    if let Some(port) = port {
        fields.push(p("Protocol", "HTTP"));
        fields.push(p("Port", port));
        fields.push(p("VpcId", "vpc-1"));
    }
    if let Some(health_check_port) = health_check_port {
        fields.push(p("HealthCheckPort", health_check_port));
    }
    create_group(router, case, name, &fields).await
}

/// Drive one `Targets`-taking operation with a single target.
pub(crate) async fn one_target(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    id: &str,
    port: Option<&str>,
) -> Reply {
    let mut form = action(operation);
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", id));
    if let Some(port) = port {
        form.push(p("Targets.member.1.Port", port));
    }
    call(router, &form).await
}

/// Every target the health read reports for `arn`, in the order it reports them — the whole
/// group where `named` names nothing, else that one target.
async fn reported_targets(
    router: &MultiServiceRouter,
    arn: &str,
    named: Option<(&str, Option<&str>)>,
) -> Vec<ReportedTarget> {
    let named: Vec<(String, Option<&str>)> = named
        .map(|(id, port)| vec![(id.to_string(), port)])
        .unwrap_or_default();
    reported_for(router, arn, &named).await
}

/// Every target the health read reports for `arn` when it names the `(id, port)` targets of
/// `named`, in the order it reports them. An empty `named` is the unfiltered read.
pub(crate) async fn reported_for(
    router: &MultiServiceRouter,
    arn: &str,
    named: &[(String, Option<&str>)],
) -> Vec<ReportedTarget> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    for (index, (id, port)) in named.iter().enumerate() {
        let at = index + 1;
        form.push(p(&format!("Targets.member.{at}.Id"), id));
        if let Some(port) = port {
            form.push(p(&format!("Targets.member.{at}.Port"), port));
        }
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "a health read must be accepted, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            // The target's own id and port are read out of its `Target`, so neither can be
            // confused with the health-check port reported beside it.
            let target = first_text(description, "Target").unwrap_or_default();
            // The health's own element, because the override beside it carries members of
            // the same names ([`health_block`]).
            let health = health_block(description);
            ReportedTarget {
                id: first_text(&target, "Id"),
                port: first_text(&target, "Port"),
                health_check_port: first_text(description, "HealthCheckPort"),
                state: first_text(&health, "State"),
                reason: first_text(&health, "Reason"),
            }
        })
        .collect()
}

#[tokio::test]
async fn deregister_targets_judges_a_target_id_by_the_groups_target_type() {
    let (router, _engine) = emulator();

    for (index, row) in FORM_ROWS.iter().enumerate() {
        let name = format!("dereg-form-{index}");
        let arn = target_group(&router, row.case, &name, row.target_type).await;
        // A well-formed target of this group, so the refusal below cannot be mistaken for
        // "the group holds nothing": this one is still registered afterwards.
        let held = match row.target_type {
            "ip" => "10.0.0.11",
            _ => "i-00000000000000011",
        };
        let registered = one_target(&router, "RegisterTargets", &arn, held, None).await;
        assert_eq!(
            registered.status, 200,
            "case {}: the well-formed target must register, got {} body {}",
            row.case, registered.status, registered.body
        );

        let reply = one_target(&router, "DeregisterTargets", &arn, row.id, None).await;
        assert_eq!(
            refusal(&reply),
            (400, VALIDATION_ERROR.to_string(), row.message.to_string()),
            "case {}: expected the form refusal, got {}",
            row.case,
            reply.body
        );
        // A refused deregistration removes nothing.
        assert_eq!(
            reported_targets(&router, &arn, None)
                .await
                .into_iter()
                .map(|reported| reported.id)
                .collect::<Vec<Option<String>>>(),
            vec![Some(held.to_string())],
            "case {}: a refused deregistration must leave the group as it was",
            row.case
        );
    }
}

#[tokio::test]
async fn register_targets_refuses_the_same_forms_in_the_same_words() {
    // The rule belongs to the MEMBER, so the registration that adds a target and the
    // deregistration that removes one answer the same sentence for the same value.
    let (router, _engine) = emulator();

    for (index, row) in FORM_ROWS.iter().enumerate() {
        let name = format!("reg-form-{index}");
        let arn = target_group(&router, row.case, &name, row.target_type).await;
        let reply = one_target(&router, "RegisterTargets", &arn, row.id, None).await;
        assert_eq!(
            refusal(&reply),
            (400, VALIDATION_ERROR.to_string(), row.message.to_string()),
            "case {}: expected the form refusal, got {}",
            row.case,
            reply.body
        );
        assert!(
            reported_targets(&router, &arn, None).await.is_empty(),
            "case {}: a refused registration must store nothing",
            row.case
        );
    }
}

#[tokio::test]
async fn a_well_formed_target_that_was_never_registered_is_still_a_no_op_to_deregister() {
    // The refusal above is about the FORM of the value, not about whether the group holds
    // it: a target that could have been registered and simply was not is the no-op the
    // model documents, on an `ip` group and on an `instance` group alike.
    let (router, _engine) = emulator();

    for (case, target_type, id) in [
        ("an address never registered", "ip", "10.0.0.250"),
        ("an instance never registered", "instance", "i-0123456789abcdef0"),
    ] {
        let name = format!("dereg-noop-{target_type}");
        let arn = target_group(&router, case, &name, target_type).await;
        let reply = one_target(&router, "DeregisterTargets", &arn, id, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: deregistering a well-formed target nothing registered is a no-op, \
             got {} body {}",
            reply.status, reply.body
        );
    }
}

/// One row of the reported-port table: the case, the id the read names, the port it names,
/// the port and the health-check port the answer must carry, and the health state and reason
/// that target is under.
struct PortRow {
    case: &'static str,
    id: &'static str,
    names_port: Option<&'static str>,
    reports_port: &'static str,
    reports_health_check_port: &'static str,
    state: &'static str,
    reason: &'static str,
}

#[tokio::test]
async fn a_reported_target_carries_the_port_the_request_named_else_the_groups_own() {
    // The group's own port, and a registration that overrides it: each is reported under
    // the port traffic to that target goes to, whether or not the target is registered.
    let (router, _engine) = emulator();
    let arn = target_group(&router, "the reported port", "reported-port-tg", "instance").await;
    let registered = "i-00000000000000022";
    let never = "i-00000000000000033";

    let reply = one_target(&router, "RegisterTargets", &arn, registered, None).await;
    assert_eq!(
        reply.status, 200,
        "the registration must be accepted, got {} body {}",
        reply.status, reply.body
    );

    // A registered target of a group no load balancer forwards to has nothing checking it,
    // which is what `Target.NotInUse` words; one that was never registered reads
    // `Target.NotRegistered`. Either way the PORT is the one traffic to it would go to.
    let rows: &[PortRow] = &[
        PortRow {
            case: "a registered target read with no port",
            id: registered,
            names_port: None,
            reports_port: "80",
            reports_health_check_port: "80",
            state: "unused",
            reason: "Target.NotInUse",
        },
        PortRow {
            case: "a registered target read on the group's port",
            id: registered,
            names_port: Some("80"),
            reports_port: "80",
            reports_health_check_port: "80",
            state: "unused",
            reason: "Target.NotInUse",
        },
        PortRow {
            case: "an unregistered target read with no port",
            id: never,
            names_port: None,
            reports_port: "80",
            reports_health_check_port: "80",
            state: "unused",
            reason: "Target.NotRegistered",
        },
        PortRow {
            case: "an unregistered target read on a port of its own",
            id: never,
            names_port: Some("8080"),
            reports_port: "8080",
            // This group configures no health-check port, so the probe runs against the port
            // that target receives traffic on.
            reports_health_check_port: "8080",
            state: "unused",
            reason: "Target.NotRegistered",
        },
    ];
    for row in rows {
        let reported = reported_targets(&router, &arn, Some((row.id, row.names_port))).await;
        assert_eq!(
            reported,
            vec![ReportedTarget {
                id: Some(row.id.to_string()),
                port: Some(row.reports_port.to_string()),
                health_check_port: Some(row.reports_health_check_port.to_string()),
                state: Some(row.state.to_string()),
                reason: Some(row.reason.to_string()),
            }],
            "case {}: expected the port {:?} on the reported target, got {reported:?}",
            row.case,
            row.reports_port
        );
    }
}

/// One row of the group-port table: the group the read is made against, the target it names,
/// and the row that read must answer.
struct GroupPortRow {
    case: &'static str,
    /// The group's `TargetType`.
    target_type: &'static str,
    /// The group's own `Port`, absent for a target type that carries none.
    group_port: Option<&'static str>,
    /// The group's `HealthCheckPort`, absent where the group leaves it to the traffic port.
    group_health_check_port: Option<&'static str>,
    /// The target the read names, and whether it is registered first.
    id: TargetId,
    registered: bool,
    /// The `Port` the read names beside the id, or `None` for an id-only read.
    names_port: Option<&'static str>,
    /// The `Target$Port` and `HealthCheckPort` the answer must carry.
    reports_port: Option<&'static str>,
    reports_health_check_port: Option<&'static str>,
    reason: &'static str,
}

/// How a group-port row writes the id it reads about: a literal of the group's target type,
/// or — for a `lambda` group, whose targets are function ARNs — a function of this account.
enum TargetId {
    Literal(&'static str),
    Function(&'static str),
}

impl TargetId {
    /// The value the request carries.
    fn resolve(&self) -> String {
        match self {
            TargetId::Literal(id) => (*id).to_string(),
            TargetId::Function(name) => {
                format!("arn:aws:lambda:{REGION}:{}:function:{name}", account())
            }
        }
    }
}

/// The published range of every port member the model types as an integer
/// (`smithy_query(query="shape",shape="Port")`: `range 1..65535`), whose bounds the group ports
/// below are drawn from — so the rule is asked of a group port no default could have supplied.
const LOW_BOUND: &str = "1";
const HIGH_BOUND: &str = "65535";

/// The groups and reads the rule is asked of beyond the default-port group above: the port the
/// group is on, down to a group that has none, crossed with the target type, the health-check
/// configuration, whether the read names a port and whether the id is registered.
fn group_port_rows() -> Vec<GroupPortRow> {
    vec![
        GroupPortRow {
            case: "an unregistered instance read by id alone, on a port no default picks",
            target_type: "instance",
            group_port: Some("44300"),
            group_health_check_port: Some("4000"),
            id: TargetId::Literal("i-0123456789abcdef0"),
            registered: false,
            names_port: None,
            reports_port: Some("44300"),
            reports_health_check_port: Some("4000"),
            reason: "Target.NotRegistered",
        },
        GroupPortRow {
            case: "an unregistered instance read on a port of its own",
            target_type: "instance",
            group_port: Some("44300"),
            group_health_check_port: Some("4000"),
            id: TargetId::Literal("i-0123456789abcdef0"),
            registered: false,
            names_port: Some("44301"),
            reports_port: Some("44301"),
            reports_health_check_port: Some("4000"),
            reason: "Target.NotRegistered",
        },
        GroupPortRow {
            case: "an unregistered instance on a group at the low bound of the port range",
            target_type: "instance",
            group_port: Some(LOW_BOUND),
            group_health_check_port: None,
            id: TargetId::Literal("i-00000000000000044"),
            registered: false,
            names_port: None,
            reports_port: Some(LOW_BOUND),
            reports_health_check_port: Some(LOW_BOUND),
            reason: "Target.NotRegistered",
        },
        GroupPortRow {
            case: "an unregistered instance on a group at the high bound of the port range",
            target_type: "instance",
            group_port: Some(HIGH_BOUND),
            group_health_check_port: None,
            id: TargetId::Literal("i-00000000000000055"),
            registered: false,
            names_port: None,
            reports_port: Some(HIGH_BOUND),
            reports_health_check_port: Some(HIGH_BOUND),
            reason: "Target.NotRegistered",
        },
        GroupPortRow {
            case: "an unregistered address read by id alone",
            target_type: "ip",
            group_port: Some("8081"),
            group_health_check_port: Some("9090"),
            id: TargetId::Literal("10.0.0.77"),
            registered: false,
            names_port: None,
            reports_port: Some("8081"),
            reports_health_check_port: Some("9090"),
            reason: "Target.NotRegistered",
        },
        GroupPortRow {
            case: "a registered instance read by id alone",
            target_type: "instance",
            group_port: Some("44300"),
            group_health_check_port: Some("4000"),
            id: TargetId::Literal("i-00000000000000066"),
            registered: true,
            names_port: None,
            reports_port: Some("44300"),
            reports_health_check_port: Some("4000"),
            reason: "Target.NotInUse",
        },
        GroupPortRow {
            case: "a registered address read by id alone",
            target_type: "ip",
            group_port: Some("8081"),
            group_health_check_port: None,
            id: TargetId::Literal("10.0.0.9"),
            registered: true,
            names_port: None,
            reports_port: Some("8081"),
            reports_health_check_port: Some("8081"),
            reason: "Target.NotInUse",
        },
        GroupPortRow {
            case: "an unregistered function on a group that has no port of its own",
            target_type: "lambda",
            group_port: None,
            group_health_check_port: None,
            id: TargetId::Function("never-registered"),
            registered: false,
            names_port: None,
            // A Lambda function is invoked rather than connected to, so neither the group nor
            // the read supplies a port and the row carries none.
            reports_port: None,
            reports_health_check_port: None,
            reason: "Target.NotRegistered",
        },
    ]
}

#[tokio::test]
async fn a_reported_target_carries_its_own_groups_port_whatever_that_port_is() {
    // The port the rule defaults to is the GROUP's, read off the group under the read: a port
    // no default picks, each bound of the published range, and a target type that carries no
    // port at all.
    let (router, _engine) = emulator();

    for (index, row) in group_port_rows().iter().enumerate() {
        let name = format!("group-port-{index}");
        let arn = target_group_on(
            &router,
            row.case,
            &name,
            row.target_type,
            row.group_port,
            row.group_health_check_port,
        )
        .await;
        let id = row.id.resolve();
        if row.registered {
            let reply = one_target(&router, "RegisterTargets", &arn, &id, None).await;
            assert_eq!(
                reply.status, 200,
                "case {}: the registration must be accepted, got {} body {}",
                row.case, reply.status, reply.body
            );
        }

        let reported = reported_for(&router, &arn, &[(id.clone(), row.names_port)]).await;
        let expected = vec![ReportedTarget {
            id: Some(id.clone()),
            port: row.reports_port.map(str::to_string),
            health_check_port: row.reports_health_check_port.map(str::to_string),
            state: Some("unused".to_string()),
            reason: Some(row.reason.to_string()),
        }];
        assert_eq!(
            reported, expected,
            "case {}: expected {expected:?}, got {reported:?}",
            row.case
        );
    }
}

