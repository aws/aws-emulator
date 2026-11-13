// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a bad request earns, and the sentence that refusal states — over the
//! wire, through the router.
//!
//! An error reply's `<Code>` and `<Message>` are the contract a client branches on and
//! shows, so each table below asserts one rule of the service over the SET of inputs that
//! rule generalises over rather than the single call that showed it:
//!
//! 1. A value outside the domain an attribute's documentation states is the request to fix
//!    — `ValidationError` — and the sentence states HOW the value missed that domain: it is
//!    no integer, it is outside a bound, or it is outside an enumerated set
//!    ([`an_attribute_value_outside_its_domain_is_refused_for_how_it_missed`]).
//! 2. Which attributes a load balancer carries is decided by its TYPE, and so is the
//!    refusal a key of another type earns: an Application Load Balancer answers the
//!    modelled `InvalidConfigurationRequest` naming the key and its type, while a Network
//!    or Gateway Load Balancer does not recognise the key at all — the same refusal a key
//!    no documentation lists earns
//!    ([`a_key_a_load_balancer_type_does_not_carry_is_refused_the_way_that_type_refuses_it`]).
//! 3. A `HealthCheckProtocol` outside the protocols the target group's own protocol admits
//!    is the request to fix, and the refusal lists what it may be
//!    ([`a_health_check_protocol_the_target_group_does_not_admit_is_refused_with_the_set`]).
//! 4. A refusal whose class has no variable part states AWS's fixed sentence whatever the
//!    request carried ([`a_refusal_with_no_variable_part_states_the_same_sentence`]).
//! 5. `SetRulePriorities` tells three failures apart by CODE: a priority the request
//!    repeated, a priority a rule outside the request holds — each naming that priority —
//!    and a pair naming a rule it does not hold, which is about the SET it was asked to place
//!    ([`set_rule_priorities_tells_a_repeated_priority_from_one_in_use_and_from_a_missing_rule`]).
//! 6. A target id not written the way a target of the group's TYPE is written names no
//!    target of any account, so it is the request to fix
//!    ([`a_target_id_not_written_for_its_target_type_is_refused`]).
//! 7. An action list the model does not admit is the modelled "requested action is not
//!    valid", and the counts a rule is bounded by are the request to fix
//!    ([`a_rule_whose_actions_or_counts_the_model_refuses_states_which_bound_it_broke`]).
//!
//! Every request goes through the harness `provisioning_loop.rs` and `listener_rules.rs`
//! already own — awsQuery form body in, an `<ErrorResponse>` envelope out — so nothing here
//! builds a request a second way.

use crate::listener_rules::{add_rule, create_rule, forward_to, Form};
use crate::load_balancer_attributes::create_load_balancer;
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use crate::rule_deletion_and_condition_value_spellings::default_rule_arn;
use crate::target_group_attributes_and_modification::{create_target_group, unescaped};
use smithy_server_runtime::MultiServiceRouter;

/// The code this API answers an input it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery code of `InvalidConfigurationRequestException`.
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The awsQuery code of `InvalidLoadBalancerActionException`.
const INVALID_ACTION: &str = "InvalidLoadBalancerAction";

/// The awsQuery code of `OperationNotPermittedException`.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery code of `PriorityInUseException`.
const PRIORITY_IN_USE: &str = "PriorityInUse";

/// The awsQuery code of `RuleNotFoundException`.
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// The awsQuery code of `TooManyCertificatesException`.
const TOO_MANY_CERTIFICATES: &str = "TooManyCertificates";

/// The awsQuery code of `TooManyTagsException`.
const TOO_MANY_TAGS: &str = "TooManyTags";

/// Assert one refusal: its status, its awsQuery code and its whole message, with the
/// serializer's escaping undone so the assertion compares the VALUE and not its encoding.
fn assert_refused(case: &str, reply: &Reply, code: &str, message: &str) {
    let stated = unescaped(&reply.text("Message").unwrap_or_default());
    assert_eq!(
        (reply.status, reply.error_code(), stated),
        (400, code.to_string(), message.to_string()),
        "case {case:?}: expected 400 {code} {message:?}, got {}",
        reply.body
    );
}

/// Assert a refusal's status and code alone, for a class whose sentence another table
/// pins.
fn assert_refused_under(case: &str, reply: &Reply, code: &str) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, code),
        "case {case:?}: expected 400 {code}, got {}",
        reply.body
    );
}

/// A certificate ARN to hand a listener; `index` makes each one distinct.
fn certificate_arn(index: usize) -> String {
    format!("arn:aws:acm:us-west-2:000000000000:certificate/{index:08}-1111-1111-1111-111111111111")
}

// ---------------------------------------------------------------------------
// Rule 1: a value outside an attribute's documented domain.
// ---------------------------------------------------------------------------

/// Which resource's attributes a write names, with the operation that writes them and the
/// member naming the resource.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    LoadBalancer,
    Listener,
    TargetGroup,
}

impl Owner {
    fn operation(self) -> &'static str {
        match self {
            Self::LoadBalancer => "ModifyLoadBalancerAttributes",
            Self::Listener => "ModifyListenerAttributes",
            Self::TargetGroup => "ModifyTargetGroupAttributes",
        }
    }

    fn arn_member(self) -> &'static str {
        match self {
            Self::LoadBalancer => "LoadBalancerArn",
            Self::Listener => "ListenerArn",
            Self::TargetGroup => "TargetGroupArn",
        }
    }
}

/// Write one attribute through the operation that writes `owner`'s attributes.
async fn write_attribute(
    router: &MultiServiceRouter,
    owner: Owner,
    arn: &str,
    key: &str,
    value: &str,
) -> Reply {
    let mut form = action(owner.operation());
    form.push(p(owner.arn_member(), arn));
    form.push(p("Attributes.member.1.Key", key));
    form.push(p("Attributes.member.1.Value", value));
    call(router, &form).await
}

/// One row of the domain table: whose attribute it is, the key, the value written, and the
/// sentence the refusal states — `None` for a value inside the domain, which is stored.
struct DomainRow {
    owner: Owner,
    key: &'static str,
    value: &'static str,
    refused: Option<String>,
}

/// Rule 1: a value outside an attribute's documented domain is refused `ValidationError`,
/// with the sentence for HOW it missed the domain.
///
/// The rows cross the three resources that carry attributes with every way a value can
/// miss: no integer at all, one past each end of a documented range, a value that is
/// neither of a boolean's two words, a word outside an enumerated set, and the literal a
/// range-or-literal domain also admits. Each refused row is paired with a value INSIDE the
/// same attribute's domain, which must be stored — so a row is refused for its value and
/// not for the request's shape.
#[tokio::test]
async fn an_attribute_value_outside_its_domain_is_refused_for_how_it_missed() {
    let (router, _engine) = emulator();
    let alb = create_load_balancer(&router, "domain-alb", &[]).await;
    let nlb = create_load_balancer(&router, "domain-nlb", &[("Type", "network")]).await;
    let tg = create_target_group(
        &router,
        "domain-tg",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
    )
    .await;
    // A listener of the NETWORK load balancer carries the ranged listener attribute, and a
    // TCP listener forwards to a TCP target group.
    let tcp_tg = create_target_group(
        &router,
        "domain-tcp-tg",
        &[("Protocol", "TCP"), ("Port", "80"), ("VpcId", "vpc-1")],
    )
    .await;
    let listener = {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &nlb));
        form.push(p("Protocol", "TCP"));
        form.push(p("Port", "80"));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tcp_tg));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case the network listener the listener rows write to: expected 200, got {}",
            reply.body
        );
        reply
            .text("ListenerArn")
            .expect("the created listener reports its ARN")
    };

    let rows: Vec<DomainRow> = vec![
        // A load balancer's attributes: the two integer failures, a boolean and an enum.
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "idle_timeout.timeout_seconds",
            value: "not-a-number",
            refused: Some(
                "The value of 'idle_timeout.timeout_seconds' must be a valid integer".to_string(),
            ),
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "idle_timeout.timeout_seconds",
            value: "0",
            refused: Some(
                "The value of 'idle_timeout.timeout_seconds' cannot be less than 1 characters"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "idle_timeout.timeout_seconds",
            value: "4001",
            refused: Some(
                "The value of 'idle_timeout.timeout_seconds' cannot be greater than 4000 \
                 characters"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "idle_timeout.timeout_seconds",
            value: "4000",
            refused: None,
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "deletion_protection.enabled",
            value: "yes",
            refused: Some(
                "The value of 'deletion_protection.enabled' must be 'true' or 'false', but was \
                 'yes'"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "deletion_protection.enabled",
            value: "true",
            refused: None,
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "routing.http.xff_header_processing.mode",
            value: "rewrite",
            refused: Some(
                "The value of 'routing.http.xff_header_processing.mode' must be 'append' or \
                 'preserve' or 'remove', but was 'rewrite'"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::LoadBalancer,
            key: "routing.http.xff_header_processing.mode",
            value: "preserve",
            refused: None,
        },
        // A listener's attributes: the same range, worded as that validator words it.
        DomainRow {
            owner: Owner::Listener,
            key: "tcp.idle_timeout.seconds",
            value: "59",
            refused: Some(
                "'tcp.idle_timeout.seconds' value '59' must be between '60-6000' inclusive"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::Listener,
            key: "tcp.idle_timeout.seconds",
            value: "6001",
            refused: Some(
                "'tcp.idle_timeout.seconds' value '6001' must be between '60-6000' inclusive"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::Listener,
            key: "tcp.idle_timeout.seconds",
            value: "not-a-number",
            refused: Some(
                "The value of 'tcp.idle_timeout.seconds' must be a valid integer".to_string(),
            ),
        },
        DomainRow {
            owner: Owner::Listener,
            key: "tcp.idle_timeout.seconds",
            value: "60",
            refused: None,
        },
        DomainRow {
            owner: Owner::Listener,
            key: "tcp.idle_timeout.seconds",
            value: "6000",
            refused: None,
        },
        // A target group's attributes: a range, a boolean and a range-or-literal domain.
        DomainRow {
            owner: Owner::TargetGroup,
            key: "deregistration_delay.timeout_seconds",
            value: "3601",
            refused: Some(
                "The value of 'deregistration_delay.timeout_seconds' cannot be greater than 3600 \
                 characters"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::TargetGroup,
            key: "deregistration_delay.timeout_seconds",
            value: "3600",
            refused: None,
        },
        DomainRow {
            owner: Owner::TargetGroup,
            key: "stickiness.enabled",
            value: "sometimes",
            refused: Some(
                "The value of 'stickiness.enabled' must be 'true' or 'false', but was 'sometimes'"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::TargetGroup,
            key: "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            value: "sometimes",
            refused: Some(
                "The value of \
                 'target_group_health.dns_failover.minimum_healthy_targets.percentage' must be \
                 'off' or a valid integer"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::TargetGroup,
            key: "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            value: "0",
            refused: Some(
                "The value of \
                 'target_group_health.dns_failover.minimum_healthy_targets.percentage' cannot be \
                 less than 1 characters"
                    .to_string(),
            ),
        },
        DomainRow {
            owner: Owner::TargetGroup,
            key: "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            value: "off",
            refused: None,
        },
    ];

    for row in &rows {
        let arn = match row.owner {
            Owner::LoadBalancer => alb.as_str(),
            Owner::Listener => listener.as_str(),
            Owner::TargetGroup => tg.as_str(),
        };
        let case = format!("{}={} on {}", row.key, row.value, row.owner.operation());
        let reply = write_attribute(&router, row.owner, arn, row.key, row.value).await;
        match &row.refused {
            Some(message) => assert_refused(&case, &reply, VALIDATION_ERROR, message),
            None => assert_eq!(
                reply.status, 200,
                "case {case:?}: a value inside the domain must be stored, got {}",
                reply.body
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a key the load balancer's type does not carry.
// ---------------------------------------------------------------------------

/// Rule 2: the refusal a load balancer attribute key earns is decided by the load
/// balancer's TYPE.
///
/// The rows cross the three types with keys of each of the other two and with a key no
/// documentation lists at all: an Application Load Balancer answers a key it does not carry
/// as a configuration it cannot hold, naming the key and its type, while a Network or
/// Gateway Load Balancer answers it exactly as it answers a key nothing defines — it does
/// not recognise it.
#[tokio::test]
async fn a_key_a_load_balancer_type_does_not_carry_is_refused_the_way_that_type_refuses_it() {
    let (router, _engine) = emulator();

    // (case, the type the load balancer is created with, the key, the code, the sentence)
    let rows: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "a network-only key on an application load balancer",
            "application",
            "dns_record.client_routing_policy",
            INVALID_CONFIGURATION,
            "Load balancer attribute key 'dns_record.client_routing_policy' is not supported on \
             load balancers with type 'application'"
                .to_string(),
        ),
        (
            "a second network-only key on an application load balancer",
            "application",
            "secondary_ips.auto_assigned.per_subnet",
            INVALID_CONFIGURATION,
            "Load balancer attribute key 'secondary_ips.auto_assigned.per_subnet' is not \
             supported on load balancers with type 'application'"
                .to_string(),
        ),
        (
            "an application-only key on a network load balancer",
            "network",
            "idle_timeout.timeout_seconds",
            VALIDATION_ERROR,
            "Load balancer attribute key 'idle_timeout.timeout_seconds' is not recognized"
                .to_string(),
        ),
        (
            "a second application-only key on a network load balancer",
            "network",
            "routing.http2.enabled",
            VALIDATION_ERROR,
            "Load balancer attribute key 'routing.http2.enabled' is not recognized".to_string(),
        ),
        (
            "a key of both other types on a gateway load balancer",
            "gateway",
            "access_logs.s3.enabled",
            VALIDATION_ERROR,
            "Load balancer attribute key 'access_logs.s3.enabled' is not recognized".to_string(),
        ),
        (
            "a key no documentation lists, on an application load balancer",
            "application",
            "routing.http.invented_mode",
            VALIDATION_ERROR,
            "Load balancer attribute key 'routing.http.invented_mode' is not recognized"
                .to_string(),
        ),
        (
            "a key no documentation lists, on a network load balancer",
            "network",
            "routing.http.invented_mode",
            VALIDATION_ERROR,
            "Load balancer attribute key 'routing.http.invented_mode' is not recognized"
                .to_string(),
        ),
    ];

    for (index, (case, lb_type, key, code, message)) in rows.iter().enumerate() {
        let name = format!("legality-{index}");
        let arn = create_load_balancer(&router, &name, &[("Type", lb_type)]).await;
        let reply = write_attribute(&router, Owner::LoadBalancer, &arn, key, "true").await;
        assert_refused(case, &reply, code, message);
    }

    // And a value an attribute's documentation FIXES for this kind of load balancer is the
    // request to fix, naming the one value it has: the same class as a value outside a
    // domain, which rule 1 drives.
    let alb = create_load_balancer(&router, "legality-fixed", &[]).await;
    let reply = write_attribute(
        &router,
        Owner::LoadBalancer,
        &alb,
        "load_balancing.cross_zone.enabled",
        "false",
    )
    .await;
    assert_refused(
        "turning cross-zone load balancing off on an application load balancer",
        &reply,
        VALIDATION_ERROR,
        "The value for 'load_balancing.cross_zone.enabled' must always be 'true' for an \
         Application Load Balancer",
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the health-check protocols a target group admits.
// ---------------------------------------------------------------------------

/// Rule 3: a `HealthCheckProtocol` outside the set the target group's own protocol admits
/// is refused `ValidationError`, and the refusal lists the set.
///
/// The rows cross the target group protocols with the health-check protocols: HTTP and
/// HTTPS admit only themselves ("the TCP protocol is not supported for health checks if the
/// protocol of the target group is HTTP or HTTPS"), every other protocol admits TCP beside
/// them, and the protocols health checks do not support at all ("GENEVE, TLS, UDP,
/// TCP_UDP, QUIC, and TCP_QUIC") are refused for every group. Each row is driven through
/// BOTH operations that set a health check: the create that first stores it and the modify
/// that changes it.
#[tokio::test]
async fn a_health_check_protocol_the_target_group_does_not_admit_is_refused_with_the_set() {
    let (router, _engine) = emulator();

    // (the group's own protocol and port, the health-check protocol, the set it admits —
    // empty for a health-check protocol the group accepts)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("HTTP", "80", "HTTP", ""),
        ("HTTP", "80", "HTTPS", ""),
        ("HTTP", "80", "TCP", "HTTP, HTTPS"),
        ("HTTPS", "443", "TCP", "HTTP, HTTPS"),
        ("HTTPS", "443", "HTTPS", ""),
        ("TCP", "1234", "TCP", ""),
        ("TCP", "1234", "HTTP", ""),
        ("TCP", "1234", "UDP", "HTTP, HTTPS, TCP"),
        ("TCP", "1234", "GENEVE", "HTTP, HTTPS, TCP"),
        ("TCP", "1234", "TLS", "HTTP, HTTPS, TCP"),
        ("HTTP", "80", "TCP_UDP", "HTTP, HTTPS"),
        ("UDP", "53", "TCP", ""),
        ("UDP", "53", "TCP_QUIC", "HTTP, HTTPS, TCP"),
    ];

    for (index, (protocol, port, health_check, admitted)) in rows.iter().enumerate() {
        let case = format!("a {health_check} health check of a {protocol} target group");
        // The create, which stores the health check the group starts with.
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", &format!("hc-protocol-{index}")));
        form.push(p("Protocol", protocol));
        form.push(p("Port", port));
        form.push(p("VpcId", "vpc-1"));
        form.push(p("HealthCheckProtocol", health_check));
        let created = call(&router, &form).await;

        // The modify, which changes the health check of a group that already holds one.
        let group = create_target_group(
            &router,
            &format!("hc-protocol-modify-{index}"),
            &[("Protocol", protocol), ("Port", port), ("VpcId", "vpc-1")],
        )
        .await;
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &group));
        form.push(p("HealthCheckProtocol", health_check));
        // A layer-7 health check is only a health check once it has both a destination and
        // a return code, and a MODIFY settles neither: a row that turns the check into an
        // HTTP one carries both, so the only thing it is judged on is the protocol.
        if matches!(*health_check, "HTTP" | "HTTPS") {
            form.push(p("HealthCheckPath", "/healthz"));
            form.push(p("Matcher.HttpCode", "200"));
        }
        let modified = call(&router, &form).await;

        if admitted.is_empty() {
            for (operation, reply) in [("CreateTargetGroup", &created), ("ModifyTargetGroup", &modified)] {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} through {operation}: expected the health check to be stored, \
                     got {}",
                    reply.body
                );
            }
            continue;
        }
        let message = format!("Health check protocol '{health_check}' must be one of '{admitted}'");
        for (operation, reply) in [("CreateTargetGroup", &created), ("ModifyTargetGroup", &modified)] {
            assert_refused(
                &format!("{case} through {operation}"),
                reply,
                VALIDATION_ERROR,
                &message,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the refusals with no variable part.
// ---------------------------------------------------------------------------

/// Rule 4: a refusal whose class states a FIXED sentence states it whatever the request
/// carried.
///
/// Each row varies what the request carries — how many certificates past the bound, how
/// many tags past the quota and on which kind of resource, which certificate of which
/// listener, and which load balancer type the address type is illegal for — and the
/// sentence is the same every time, except the one frame that quotes the load balancer's
/// own type.
#[tokio::test]
async fn a_refusal_with_no_variable_part_states_the_same_sentence() {
    let (router, _engine) = emulator();
    let alb = create_load_balancer(&router, "fixed-text-alb", &[]).await;
    let tg = create_http_target_group(&router, "fixed-text-tg").await;

    // A listener's certificate list holds 25 certificates; every count past it earns the
    // one sentence.
    let https = {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &alb));
        form.push(p("Protocol", "HTTPS"));
        form.push(p("Port", "443"));
        form.push(p("Certificates.member.1.CertificateArn", &certificate_arn(0)));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case the HTTPS listener the certificate rows hang on: expected 200, got {}",
            reply.body
        );
        reply
            .text("ListenerArn")
            .expect("the created listener reports its ARN")
    };
    for index in 1..=25 {
        let mut form = action("AddListenerCertificates");
        form.push(p("ListenerArn", &https));
        form.push(p("Certificates.member.1.CertificateArn", &certificate_arn(index)));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case filling the certificate list with certificate {index}: expected 200, got {}",
            reply.body
        );
    }
    for index in 26..=28 {
        let mut form = action("AddListenerCertificates");
        form.push(p("ListenerArn", &https));
        form.push(p("Certificates.member.1.CertificateArn", &certificate_arn(index)));
        let reply = call(&router, &form).await;
        assert_refused(
            &format!("certificate {index}, past the list's bound"),
            &reply,
            TOO_MANY_CERTIFICATES,
            "The maximum number of certificates has been reached",
        );
    }

    // The default certificate lives outside that list and is replaced through
    // `ModifyListener`, so removing it is refused with the same sentence whichever
    // listener and whichever certificate the request names.
    let listeners = [
        (https.clone(), certificate_arn(0)),
        {
            let mut form = action("CreateListener");
            form.push(p("LoadBalancerArn", &alb));
            form.push(p("Protocol", "HTTPS"));
            form.push(p("Port", "8443"));
            form.push(p("Certificates.member.1.CertificateArn", &certificate_arn(99)));
            form.push(p("DefaultActions.member.1.Type", "forward"));
            form.push(p("DefaultActions.member.1.TargetGroupArn", &tg));
            let reply = call(&router, &form).await;
            assert_eq!(
                reply.status, 200,
                "case the second HTTPS listener: expected 200, got {}",
                reply.body
            );
            (
                reply
                    .text("ListenerArn")
                    .expect("the created listener reports its ARN"),
                certificate_arn(99),
            )
        },
    ];
    for (listener, certificate) in &listeners {
        let mut form = action("RemoveListenerCertificates");
        form.push(p("ListenerArn", listener));
        form.push(p("Certificates.member.1.CertificateArn", certificate));
        let reply = call(&router, &form).await;
        assert_refused(
            &format!("removing the default certificate of {listener}"),
            &reply,
            OPERATION_NOT_PERMITTED,
            "Default certificate cannot be removed",
        );
    }

    // A tag write past the 50 tags a resource holds states one sentence, whatever the
    // count and whichever resource it was asked of.
    for (case, arn, count) in [
        ("51 tags on a load balancer", &alb, 51),
        ("60 tags on a load balancer", &alb, 60),
        ("51 tags on a target group", &tg, 51),
    ] {
        let mut form = action("AddTags");
        form.push(p("ResourceArns.member.1", arn));
        for index in 1..=count {
            form.push(p(&format!("Tags.member.{index}.Key"), &format!("key-{index}")));
            form.push(p(&format!("Tags.member.{index}.Value"), "v"));
        }
        let reply = call(&router, &form).await;
        assert_refused(
            case,
            &reply,
            TOO_MANY_TAGS,
            "The specified resource contains too many tags",
        );
    }

    // An IP address type illegal for a load balancer's type names that TYPE and nothing
    // the request carried.
    for lb_type in ["network", "gateway"] {
        let arn = create_load_balancer(
            &router,
            &format!("fixed-text-{lb_type}"),
            &[("Type", lb_type)],
        )
        .await;
        let mut form = action("SetIpAddressType");
        form.push(p("LoadBalancerArn", &arn));
        form.push(p("IpAddressType", "dualstack-without-public-ipv4"));
        let reply = call(&router, &form).await;
        assert_refused(
            &format!("dualstack-without-public-ipv4 on a {lb_type} load balancer"),
            &reply,
            INVALID_CONFIGURATION,
            &format!(
                "The specified IP address type is not supported on load balancers with type \
                 '{lb_type}'."
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: what SetRulePriorities tells apart.
// ---------------------------------------------------------------------------

/// The fields of one acceptable rule: a path condition of its own and a forward action.
fn rule_fields(target_group_arn: &str, path: &str) -> Form {
    let mut fields = vec![
        p("Conditions.member.1.Field", "path-pattern"),
        p("Conditions.member.1.Values.member.1", path),
    ];
    fields.extend(forward_to(target_group_arn));
    fields
}

/// Send one `SetRulePriorities` naming each `(rule, priority)` pair.
async fn set_priorities(router: &MultiServiceRouter, pairs: &[(&str, i32)]) -> Reply {
    let mut form = action("SetRulePriorities");
    for (index, (rule, priority)) in pairs.iter().enumerate() {
        form.push(p(
            &format!("RulePriorities.member.{}.RuleArn", index + 1),
            rule,
        ));
        form.push(p(
            &format!("RulePriorities.member.{}.Priority", index + 1),
            &priority.to_string(),
        ));
    }
    call(router, &form).await
}

/// Rule 5: `SetRulePriorities` answers three failures under three codes — a repeated priority
/// and a priority already in use each naming the value the caller has to change, and a pair
/// naming no rule worded about the SET of priorities it was asked to place.
///
/// The rows vary the priority the request repeats, the rule the collision is with, and the
/// shape of the ARN that names no rule — so each code is driven by several inputs rather
/// than one.
#[tokio::test]
async fn set_rule_priorities_tells_a_repeated_priority_from_one_in_use_and_from_a_missing_rule() {
    /// The sentence this operation words a refusal ABOUT THE SET it addresses with.
    const ABOUT_THE_SET: &str = "One or more priorities not found";

    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "priority-classes-alb", &[]).await;
    let tg = create_http_target_group(&router, "priority-classes-tg").await;
    let listener = {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", "80"));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case the listener the rules live on: expected 200, got {}",
            reply.body
        );
        reply
            .text("ListenerArn")
            .expect("the created listener reports its ARN")
    };
    // Three rules at known priorities, and a fourth to move around.
    let mut rules: Vec<String> = Vec::new();
    for priority in [10, 20, 30] {
        rules.push(
            add_rule(
                &router,
                "seeding the priorities under test",
                &listener,
                priority,
                &rule_fields(&tg, &format!("/seeded/{priority}")),
            )
            .await,
        );
    }
    let moved = add_rule(
        &router,
        "the rule the reorder moves",
        &listener,
        40,
        &rule_fields(&tg, "/moved"),
    )
    .await;

    // A priority the REQUEST repeats says nothing about which rule evaluates there: the
    // request is what has to be fixed, and the refusal states the repeated value.
    for priority in [7, 30, 50_000] {
        let reply = set_priorities(
            &router,
            &[(moved.as_str(), priority), (rules[0].as_str(), priority)],
        )
        .await;
        assert_refused(
            &format!("two rules of one request onto the priority {priority}"),
            &reply,
            VALIDATION_ERROR,
            &format!("Priority '{priority}' was provided multiple times"),
        );
    }

    // A priority a rule OUTSIDE the request holds is the modelled conflict, worded with the
    // priority the request asked for — one sentence for one rule of the service, whichever
    // operation places the priority.
    for priority in [10, 20, 30] {
        let reply = set_priorities(&router, &[(moved.as_str(), priority)]).await;
        assert_refused(
            &format!("the reorder onto the taken priority {priority}"),
            &reply,
            PRIORITY_IN_USE,
            &format!("Priority '{priority}' is currently in use"),
        );
    }

    // A pair naming a rule this store does not hold is the modelled not-found, worded about
    // the SET the request addresses.
    let deleted = {
        let mut form = action("DeleteRule");
        form.push(p("RuleArn", &rules[2]));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case deleting the third rule: expected 200, got {}",
            reply.body
        );
        rules[2].clone()
    };
    for (case, rule) in [
        ("a rule that was deleted", deleted.as_str()),
        ("a value that is no rule ARN at all", "not-a-rule-arn"),
        (
            "a rule ARN of the right shape naming nothing",
            "arn:aws:elasticloadbalancing:us-east-1:000000000000:listener-rule/app/nothing/\
             0123456789abcdef/0123456789abcdef/0123456789abcdef",
        ),
    ] {
        let reply = set_priorities(&router, &[(rule, 5)]).await;
        assert_refused(case, &reply, RULE_NOT_FOUND, ABOUT_THE_SET);
    }

    // The listener's own default rule has no numeric priority to move, and that refusal is
    // its own: the three above are about priorities, this one about the rule.
    let default_rule = default_rule_arn(&router, &listener).await;
    let reply = set_priorities(&router, &[(default_rule.as_str(), 5)]).await;
    assert_refused_under(
        "the listener's default rule",
        &reply,
        OPERATION_NOT_PERMITTED,
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the form a target id is written in.
// ---------------------------------------------------------------------------

/// Rule 6: an id not written the way a target of the group's TYPE is written is refused
/// `ValidationError` before anything is resolved against the service that owns the target.
///
/// The rows cross the four target types with ids that miss their form in different ways —
/// the wrong prefix, too few or too many characters, a character the form does not carry,
/// the wrong case, an ARN of the wrong service and an ARN of the wrong resource type — and
/// each type's own well-formed ids are the control rows, which are accepted (nothing here
/// mounts the service that owns the target, so a well-formed id resolves to nothing and is
/// stored).
#[tokio::test]
async fn a_target_id_not_written_for_its_target_type_is_refused() {
    let (router, _engine) = emulator();

    // (target type, the fields a group of that type is created with)
    let groups: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "instance",
            vec![
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-1"),
                ("TargetType", "instance"),
            ],
        ),
        (
            "ip",
            vec![
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-1"),
                ("TargetType", "ip"),
            ],
        ),
        ("lambda", vec![("TargetType", "lambda")]),
        (
            "alb",
            vec![
                ("Protocol", "TCP"),
                ("Port", "80"),
                ("VpcId", "vpc-1"),
                ("TargetType", "alb"),
            ],
        ),
    ];
    /// The fields a group of `target_type` is created with.
    fn group_fields<'a>(
        groups: &'a [(&str, Vec<(&'a str, &'a str)>)],
        target_type: &str,
    ) -> &'a [(&'a str, &'a str)] {
        groups
            .iter()
            .find(|(kind, _)| *kind == target_type)
            .map(|(_, fields)| fields.as_slice())
            .unwrap_or_else(|| panic!("no fields for a {target_type} target group"))
    }

    // (target type, the id, the form the refusal names — `None` for an id of the right
    // form, which is stored)
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("instance", "i-0123456789abcdef0", None),
        ("instance", "i-01234567", None),
        ("instance", "not-an-instance", Some("an instance ID")),
        ("instance", "i-123", Some("an instance ID")),
        ("instance", "i-0123456789abcdef", Some("an instance ID")),
        ("instance", "i-0123456789abcdefg", Some("an instance ID")),
        ("instance", "i-0123456789ABCDEF0", Some("an instance ID")),
        ("instance", "0123456789abcdef0", Some("an instance ID")),
        (
            "lambda",
            "arn:aws:lambda:us-west-2:000000000000:function:greeter",
            None,
        ),
        (
            "lambda",
            "arn:aws:lambda:us-west-2:000000000000:function:greeter:3",
            None,
        ),
        ("lambda", "greeter", Some("a Lambda function ARN")),
        (
            "lambda",
            "arn:aws:lambda:us-west-2:000000000000:layer:greeter",
            Some("a Lambda function ARN"),
        ),
        (
            "lambda",
            "arn:aws:states:us-west-2:000000000000:function:greeter",
            Some("a Lambda function ARN"),
        ),
        (
            "alb",
            "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/inner/\
             0123456789abcdef",
            None,
        ),
        (
            "alb",
            "arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/inner/\
             0123456789abcdef",
            Some("an Application Load Balancer ARN"),
        ),
        ("alb", "inner", Some("an Application Load Balancer ARN")),
    ];

    // A FRESH target group per row, so a row that registers a target never fills a
    // group another row writes to: a `lambda` or `alb` group holds exactly one target.
    for (index, (target_type, id, form)) in rows.iter().enumerate() {
        let arn = create_target_group(
            &router,
            &format!("target-id-{index}"),
            group_fields(&groups, target_type),
        )
        .await;
        let case = format!("{id:?} on a {target_type} target group");
        let mut request = action("RegisterTargets");
        request.push(p("TargetGroupArn", &arn));
        request.push(p("Targets.member.1.Id", id));
        let reply = call(&router, &request).await;
        match form {
            Some(form) => assert_refused(
                &case,
                &reply,
                VALIDATION_ERROR,
                &format!("'{id}' in Targets is not {form}"),
            ),
            None => assert_eq!(
                reply.status, 200,
                "case {case:?}: an id of the right form must be registered, got {}",
                reply.body
            ),
        }
    }

    // An `ip` group's ids are read as addresses, so a value that is no address is refused
    // under the same code — the sentence is the address parse's own.
    let ip_group = create_target_group(
        &router,
        "target-id-addresses",
        group_fields(&groups, "ip"),
    )
    .await;
    for id in ["i-0123456789abcdef0", "10.0.0.300", "not-an-address"] {
        let mut request = action("RegisterTargets");
        request.push(p("TargetGroupArn", &ip_group));
        request.push(p("Targets.member.1.Id", id));
        let reply = call(&router, &request).await;
        assert_refused_under(
            &format!("{id:?} on an ip target group"),
            &reply,
            VALIDATION_ERROR,
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: the action list and the counts a rule is bounded by.
// ---------------------------------------------------------------------------

/// Rule 7: an action list the model does not admit is the modelled "requested action is not
/// valid", while the COUNTS a rule is bounded by are the request to fix.
///
/// The action rows vary the COUNT, which type is repeated and which routing actions are
/// combined, and each is driven through both operations that set a rule's actions; the
/// count rows drive the two bounds a rule's conditions carry — how many conditions it has,
/// and how many values those conditions match on.
#[tokio::test]
async fn a_rule_whose_actions_or_counts_the_model_refuses_states_which_bound_it_broke() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "action-rules-alb", &[]).await;
    let tg = create_http_target_group(&router, "action-rules-tg").await;
    let listener = {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", "80"));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case the listener the rules live on: expected 200, got {}",
            reply.body
        );
        reply
            .text("ListenerArn")
            .expect("the created listener reports its ARN")
    };
    // The rule every modify row rewrites, so no row depends on another's writes.
    let rewritten = add_rule(
        &router,
        "the rule the modify rows rewrite",
        &listener,
        100,
        &rule_fields(&tg, "/rewritten"),
    )
    .await;

    /// One fixed-response action at `index`.
    fn fixed_response(index: usize, status: &str) -> Form {
        vec![
            p(&format!("Actions.member.{index}.Type"), "fixed-response"),
            p(
                &format!("Actions.member.{index}.FixedResponseConfig.StatusCode"),
                status,
            ),
        ]
    }

    /// One forward action at `index`.
    fn forward(index: usize, tg: &str) -> Form {
        vec![
            p(&format!("Actions.member.{index}.Type"), "forward"),
            p(&format!("Actions.member.{index}.TargetGroupArn"), tg),
        ]
    }

    // (case, the action fields, the sentence the refusal states)
    let action_rows: Vec<(&str, Form, String)> = vec![
        (
            "two fixed-response actions",
            {
                let mut fields = fixed_response(1, "200");
                fields.extend(fixed_response(2, "404"));
                fields
            },
            "You cannot specify multiple of the following action type: 'fixed-response'"
                .to_string(),
        ),
        (
            // Past the bound on how many actions a rule carries, the COUNT is what the
            // refusal states — whatever the types in the list are.
            "three fixed-response actions",
            {
                let mut fields = fixed_response(1, "200");
                fields.extend(fixed_response(2, "404"));
                fields.extend(fixed_response(3, "503"));
                fields
            },
            "You can specify up to 2 actions".to_string(),
        ),
        (
            "three actions of three different types",
            {
                let mut fields = vec![p("Actions.member.1.Type", "authenticate-oidc")];
                fields.extend(forward(2, &tg));
                fields.extend(fixed_response(3, "200"));
                fields
            },
            "You can specify up to 2 actions".to_string(),
        ),
        (
            "two forward actions",
            {
                let mut fields = forward(1, &tg);
                fields.extend(forward(2, &tg));
                fields
            },
            "You cannot specify multiple of the following action type: 'forward'".to_string(),
        ),
        (
            "a forward beside a fixed response",
            {
                let mut fields = forward(1, &tg);
                fields.extend(fixed_response(2, "200"));
                fields
            },
            "A rule must include exactly one of the following actions: forward, fixed-response, \
             redirect"
                .to_string(),
        ),
    ];

    for (index, (case, fields, message)) in action_rows.iter().enumerate() {
        let mut create_fields = vec![
            p("Conditions.member.1.Field", "path-pattern"),
            p(
                "Conditions.member.1.Values.member.1",
                &format!("/actions/{index}"),
            ),
        ];
        create_fields.extend(fields.iter().cloned());
        let created = create_rule(
            &router,
            &listener,
            &(200 + index).to_string(),
            &create_fields,
        )
        .await;
        assert_refused(
            &format!("{case} through CreateRule"),
            &created,
            INVALID_ACTION,
            message,
        );

        let mut form = action("ModifyRule");
        form.push(p("RuleArn", &rewritten));
        form.extend(fields.iter().cloned());
        let modified = call(&router, &form).await;
        assert_refused(
            &format!("{case} through ModifyRule"),
            &modified,
            INVALID_ACTION,
            message,
        );
    }

    // The counts: how many conditions a rule carries, and how many values they match on.
    // Each is stated with the bound it broke, so a caller can tell which one it hit.
    let mut conditions: Form = Vec::new();
    for index in 1..=6 {
        conditions.push(p(
            &format!("Conditions.member.{index}.Field"),
            "http-header",
        ));
        conditions.push(p(
            &format!("Conditions.member.{index}.HttpHeaderConfig.HttpHeaderName"),
            &format!("X-Header-{index}"),
        ));
        conditions.push(p(
            &format!("Conditions.member.{index}.HttpHeaderConfig.Values.member.1"),
            "on",
        ));
    }
    conditions.extend(forward_to(&tg));
    let reply = create_rule(&router, &listener, "300", &conditions).await;
    assert_refused(
        "six conditions on one rule",
        &reply,
        VALIDATION_ERROR,
        "You can specify up to 5 conditions",
    );

    let mut values: Form = vec![p("Conditions.member.1.Field", "path-pattern")];
    for index in 1..=6 {
        values.push(p(
            &format!("Conditions.member.1.Values.member.{index}"),
            &format!("/value/{index}"),
        ));
    }
    values.extend(forward_to(&tg));
    let reply = create_rule(&router, &listener, "301", &values).await;
    assert_refused(
        "six condition values on one rule",
        &reply,
        VALIDATION_ERROR,
        "A rule can only have '5' condition values and regex values",
    );

    // The control: five conditions and five values are accepted, so the rows above are
    // refused for the count and not for the request's shape.
    let mut five_conditions: Form = Vec::new();
    for index in 1..=5 {
        five_conditions.push(p(
            &format!("Conditions.member.{index}.Field"),
            "http-header",
        ));
        five_conditions.push(p(
            &format!("Conditions.member.{index}.HttpHeaderConfig.HttpHeaderName"),
            &format!("X-Allowed-{index}"),
        ));
        five_conditions.push(p(
            &format!("Conditions.member.{index}.HttpHeaderConfig.Values.member.1"),
            "on",
        ));
    }
    five_conditions.extend(forward_to(&tg));
    let accepted = create_rule(&router, &listener, "302", &five_conditions).await;
    assert_eq!(
        accepted.status, 200,
        "case five conditions on one rule: expected the rule to be created, got {}",
        accepted.body
    );
    let rules = members(&accepted.body, "Rules");
    assert_eq!(
        first_text(rules.first().unwrap_or(&String::new()), "Priority").as_deref(),
        Some("302"),
        "case five conditions on one rule: the created rule reports its priority, got {}",
        accepted.body
    );
}
