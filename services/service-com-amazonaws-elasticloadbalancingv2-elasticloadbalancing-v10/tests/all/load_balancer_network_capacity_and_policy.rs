// SPDX-License-Identifier: CC0-1.0

//! The load-balancer-level network, capacity-reservation and resource-policy operations —
//! over the wire, through the router.
//!
//! The rules these tables pin:
//!
//! * An operation that addresses ONE load balancer through its required
//!   `LoadBalancerArn` and finds none refuses with its modelled
//!   `LoadBalancerNotFoundException` (awsQuery code `LoadBalancerNotFound`, HTTP 400 —
//!   `smithy_query(query="operation",operation="…")`) and a message naming the ARN the
//!   request carried, in an `<ErrorResponse>` that carries the `<RequestId>` AWS puts
//!   beside `<Error>`; never a 501, which would say nobody has written the operation, and
//!   never `InvalidAction`, which would say the API has no such action.
//! * `SetIpAddressType`, `SetSecurityGroups`, `SetSubnets` and `ModifyIpPools` each
//!   persist what the request names onto that load balancer and answer what it then
//!   carries, which is what a later `DescribeLoadBalancers` reports; a request outside a
//!   rule their own documentation states is the modelled
//!   `InvalidConfigurationRequestException`.
//! * `ModifyCapacityReservation` refuses a request to configure minimum capacity with the
//!   modelled `OperationNotPermittedException`, because reserving capacity is a capability
//!   this account does not have, and serves a reset; `DescribeCapacityReservation` reports
//!   the reservation the store holds.
//! * `GetResourcePolicy` answers the policy attached to the trust store the ARN names, or
//!   the empty policy document for one no policy is attached to, and refuses an ARN it
//!   resolves no trust store from with the modelled `ResourceNotFoundException`.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`, `members`, `first_text`) and the
//! trust-store seeding come from `provisioning_loop`; the load balancer creator from
//! `load_balancer_attributes`.

use crate::load_balancer_attributes::{create_load_balancer, INVALID_CONFIGURATION};
use crate::provisioning_loop::{
    account, action, call, emulator, first_text, members, p, seed_trust_store, Reply,
    TrustStoreSeed, REGION,
};
use crate::target_group_attributes_and_modification::unescaped;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::EMPTY_RESOURCE_POLICY;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The awsQuery error code of `LoadBalancerNotFoundException`.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";

/// The awsQuery error code of `ResourceNotFoundException`, which `GetResourcePolicy`
/// declares as its one error.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// The awsQuery error code of `OperationNotPermittedException`.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The error code this service answers a value it cannot accept with, for a rule its
/// model states in prose rather than as an error shape.
const VALIDATION_ERROR: &str = "ValidationError";

/// The refusal a request carrying the PrivateLink security group setting earns when the
/// load balancer it names is not one that setting is available on.
///
/// `pub(crate)`: the suite that pins the SENTENCE of the other boundary of the same operation
/// (`network_load_balancer_security_group_sentence`) is held to this same wording, so the
/// crate's tests hold one copy of it.
pub(crate) const PRIVATE_LINK_SETTING_UNAVAILABLE: &str = "This security group setting for \
     PrivateLink traffic is only available for Network Load Balancers with security groups";

/// One row of the operation table: the case name, the action, and the fields it carries
/// beside `LoadBalancerArn`.
type OperationRow = (&'static str, &'static str, Vec<(&'static str, &'static str)>);

/// One row of a refusal table: the case name, the fields the request carries, and the
/// awsQuery error code expected — empty for a row that must be served.
type Row = (&'static str, Vec<(String, String)>, &'static str);

/// A load balancer ARN of the form this service mints that no load balancer in the store
/// carries: the type code, the name, the id and the Region are all the caller's, so a row
/// can vary any of them.
///
/// `pub(crate)` because the capacity-reservation suite addresses an absent load balancer
/// the same way: one spelling of "an ARN nothing carries" for the crate.
pub(crate) fn absent_load_balancer_arn(region: &str, type_code: &str, name: &str, id: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{region}:{}:loadbalancer/{type_code}/{name}/{id}",
        account()
    )
}

/// Send one awsQuery request for `operation` against the load balancer `arn` names, with
/// `fields` beside it.
///
/// `pub(crate)` because the capacity-reservation suite addresses one load balancer through
/// the same required `LoadBalancerArn`: one request builder for the crate.
pub(crate) async fn call_on(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    fields: &[(&str, &str)],
) -> Reply {
    let mut form = action(operation);
    form.push(p("LoadBalancerArn", arn));
    for (key, value) in fields {
        form.push(p(key, value));
    }
    call(router, &form).await
}

/// The XML of the one load balancer `DescribeLoadBalancers` reports for `arn`.
async fn described_load_balancer(router: &MultiServiceRouter, arn: &str) -> String {
    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case describe {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "LoadBalancers")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case describe {arn}: no load balancer in {}", reply.body))
}

/// The subnets the load balancer `arn` names is placed in, in the order it reports its
/// zones, each with the zone it is in.
async fn described_placement(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let reported = described_load_balancer(router, arn).await;
    members(&reported, "AvailabilityZones")
        .iter()
        .map(|zone| {
            (
                first_text(zone, "SubnetId").unwrap_or_default(),
                first_text(zone, "ZoneName").unwrap_or_default(),
            )
        })
        .collect()
}

/// The text of one element of the load balancer `arn` names, as a later read reports it.
async fn described_member(router: &MultiServiceRouter, arn: &str, tag: &str) -> Option<String> {
    let reported = described_load_balancer(router, arn).await;
    first_text(&reported, tag)
}

/// The security groups the load balancer `arn` names carries, as a later read reports
/// them.
async fn described_security_groups(router: &MultiServiceRouter, arn: &str) -> Vec<String> {
    let reported = described_load_balancer(router, arn).await;
    members(&reported, "SecurityGroups")
}

/// Assert that `reply` is the modelled refusal `code` and that its message contains
/// `names`, with the request id the awsQuery error envelope carries.
///
/// `pub(crate)` because the capacity-reservation suite asserts the same envelope: one
/// reading of a modelled refusal for the crate.
pub(crate) fn assert_refused(case: &str, reply: &Reply, code: &str, names: &str) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, code),
        "case {case:?}: expected 400 {code}, got {} body {}",
        reply.status,
        reply.body
    );
    let request_id = reply.text("RequestId");
    assert!(
        request_id.is_some_and(|id| id.split('-').map(str::len).eq([8, 4, 4, 4, 12])),
        "case {case:?}: an error envelope must carry a request id, got {}",
        reply.body
    );
    let message = reply.text("Message").map(|text| unescaped(&text));
    assert!(
        message.as_deref().is_some_and(|text| text.contains(names)),
        "case {case:?}: expected a message naming {names:?}, got {message:?}"
    );
}

/// The operations that address ONE load balancer through their required
/// `LoadBalancerArn`, each with the other members it needs to reach its own lookup.
fn load_balancer_operations() -> Vec<OperationRow> {
    vec![
        ("DescribeLoadBalancerAttributes", "DescribeLoadBalancerAttributes", vec![]),
        (
            "ModifyLoadBalancerAttributes",
            "ModifyLoadBalancerAttributes",
            vec![
                ("Attributes.member.1.Key", "deletion_protection.enabled"),
                ("Attributes.member.1.Value", "true"),
            ],
        ),
        (
            "SetIpAddressType",
            "SetIpAddressType",
            vec![("IpAddressType", "dualstack")],
        ),
        (
            "SetSecurityGroups",
            "SetSecurityGroups",
            vec![("SecurityGroups.member.1", "sg-11111111")],
        ),
        (
            "SetSubnets",
            "SetSubnets",
            vec![
                ("Subnets.member.1", "subnet-11111111"),
                ("Subnets.member.2", "subnet-22222222"),
            ],
        ),
        (
            "ModifyIpPools",
            "ModifyIpPools",
            vec![("IpamPools.Ipv4IpamPoolId", "ipam-pool-0123456789abcdef")],
        ),
        ("DescribeCapacityReservation", "DescribeCapacityReservation", vec![]),
        (
            "ModifyCapacityReservation",
            "ModifyCapacityReservation",
            vec![("ResetCapacityReservation", "true")],
        ),
    ]
}

// ---------------------------------------------------------------------------
// An absent load balancer, across every operation that addresses one.
// ---------------------------------------------------------------------------

/// Every operation that addresses one load balancer and finds none answers the modelled
/// `LoadBalancerNotFound`, naming the ARN the request carried, with a request id in the
/// envelope.
///
/// Two dimensions: every such operation, against every shape of ARN that names no load
/// balancer of this store — a well-formed one of each type code, one whose name and id
/// nothing carries, and the ARN of a load balancer that WAS here and was deleted, which
/// must read exactly like one that never existed.
#[tokio::test]
async fn every_operation_naming_an_absent_load_balancer_answers_the_modelled_refusal() {
    let (router, _engine) = emulator();

    // A load balancer that existed and was deleted: its ARN is as absent as one that was
    // never minted.
    let deleted = create_load_balancer(&router, "lb-since-deleted", &[]).await;
    let mut delete = action("DeleteLoadBalancer");
    delete.push(p("LoadBalancerArn", &deleted));
    let reply = call(&router, &delete).await;
    assert_eq!(
        reply.status, 200,
        "case setup: DeleteLoadBalancer answered {} body {}",
        reply.status, reply.body
    );

    let absent: Vec<(&str, String)> = vec![
        (
            "an application load balancer ARN nothing carries",
            absent_load_balancer_arn(REGION, "app", "sim2real-probe-missing", "0000000000000000"),
        ),
        (
            "a network load balancer ARN nothing carries",
            absent_load_balancer_arn(REGION, "net", "absent-net", "abcdef0123456789"),
        ),
        (
            "a gateway load balancer ARN nothing carries",
            absent_load_balancer_arn(REGION, "gwy", "absent-gwy", "1111111111111111"),
        ),
        ("the ARN of a deleted load balancer", deleted),
        // Every row is an ARN of the form this API mints, in the Region the request
        // addresses: the lookup is what it reaches. An ARN of another Region, or of
        // another shape, is refused on the request's own identifier before any lookup —
        // `all/identifier_arn_forms.rs` pins that across the operations that take one.
    ];

    for (arn_case, arn) in &absent {
        for (operation_case, operation, fields) in load_balancer_operations() {
            let case = format!("{operation_case} against {arn_case}");
            let reply = call_on(&router, operation, arn, &fields).await;
            assert_refused(&case, &reply, LOAD_BALANCER_NOT_FOUND, arn);
            assert_eq!(
                reply.text("Message").map(|text| unescaped(&text)),
                Some(format!("Load balancer '{arn}' not found")),
                "case {case:?}: unexpected message in {}",
                reply.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SetIpAddressType.
// ---------------------------------------------------------------------------

/// `SetIpAddressType` stores the type it accepts and answers it, and a later read of the
/// load balancer reports the same one; a type its documented rules refuse is the modelled
/// `InvalidConfigurationRequest` and changes nothing.
///
/// The rows span the three values the enum carries against the three families and both
/// schemes, which is what the family rule generalises over:
/// `dualstack-without-public-ipv4` is documented for Application Load Balancers alone. The
/// SCHEME decides nothing, and the other rule — the IPv6 ranges every type but `ipv4` needs
/// of the load balancer's subnets — is grounded in what EC2 reports about them, so with no
/// EC2 mounted here nothing is known and nothing is refused on it; that rule is tabled in
/// `all/input_rejection_classes.rs`.
#[tokio::test]
async fn set_ip_address_type_stores_what_its_documented_rules_accept() {
    // (case, the fields the load balancer is created with, the type asked for, the
    // awsQuery error code expected — empty for a row that must be served)
    type AddressTypeCase = (
        &'static str,
        Vec<(&'static str, &'static str)>,
        &'static str,
        &'static str,
    );
    let cases: Vec<AddressTypeCase> = vec![
        ("ipv4 on an application load balancer", vec![], "ipv4", ""),
        (
            "dualstack on an application load balancer",
            vec![],
            "dualstack",
            "",
        ),
        (
            "dualstack-without-public-ipv4 on an application load balancer",
            vec![],
            "dualstack-without-public-ipv4",
            "",
        ),
        (
            "dualstack on a network load balancer",
            vec![("Type", "network")],
            "dualstack",
            "",
        ),
        (
            "dualstack on a gateway load balancer",
            vec![("Type", "gateway")],
            "dualstack",
            "",
        ),
        (
            "ipv4 on an internal load balancer",
            vec![("Scheme", "internal")],
            "ipv4",
            "",
        ),
        (
            "dualstack-without-public-ipv4 on a network load balancer",
            vec![("Type", "network")],
            "dualstack-without-public-ipv4",
            INVALID_CONFIGURATION,
        ),
        (
            "dualstack-without-public-ipv4 on a gateway load balancer",
            vec![("Type", "gateway")],
            "dualstack-without-public-ipv4",
            INVALID_CONFIGURATION,
        ),
        (
            "dualstack on an internal load balancer",
            vec![("Scheme", "internal")],
            "dualstack",
            "",
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, asked, code)) in cases.iter().enumerate() {
        let name = format!("lb-ipaddr-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let before = described_member(&router, &arn, "IpAddressType").await;

        let reply = call_on(&router, "SetIpAddressType", &arn, &[("IpAddressType", asked)]).await;
        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.text("IpAddressType").as_deref(),
                Some(*asked),
                "case {case:?}: expected the stored type answered, got {}",
                reply.body
            );
            let after = described_member(&router, &arn, "IpAddressType").await;
            assert_eq!(
                after.as_deref(),
                Some(*asked),
                "case {case:?}: expected a later read to report the stored type, got {after:?}"
            );
        } else {
            // Which address types a load balancer may carry is decided by its TYPE, and
            // that is what this refusal states — not the value the request asked for.
            let lb_type = fields
                .iter()
                .find_map(|(key, value)| (*key == "Type").then_some(*value))
                .unwrap_or("application");
            assert_refused(case, &reply, code, lb_type);
            let after = described_member(&router, &arn, "IpAddressType").await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused request must store nothing, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SetSecurityGroups.
// ---------------------------------------------------------------------------

/// `SetSecurityGroups` replaces the groups the load balancer carries with the ones the
/// request lists, and a later read reports exactly those; the three rules this
/// operation's own documentation states are refused with the modelled
/// `InvalidConfigurationRequest`.
#[tokio::test]
async fn set_security_groups_overrides_the_groups_a_load_balancer_carries() {
    let (router, _engine) = emulator();

    // (case, the fields the load balancer is created with, the groups the request lists,
    // the `Enforce…` member it carries, the error code expected — empty when served)
    type SecurityGroupCase = (
        &'static str,
        Vec<(&'static str, &'static str)>,
        Vec<&'static str>,
        &'static str,
        &'static str,
    );
    let cases: Vec<SecurityGroupCase> = vec![
        (
            "two groups on an application load balancer that had none",
            vec![],
            vec!["sg-11111111", "sg-22222222"],
            "",
            "",
        ),
        (
            "one group replacing the two it was created with",
            vec![
                ("SecurityGroups.member.1", "sg-33333333"),
                ("SecurityGroups.member.2", "sg-44444444"),
            ],
            vec!["sg-55555555"],
            "",
            "",
        ),
        (
            "a network load balancer created with a group, and the PrivateLink member off",
            vec![
                ("Type", "network"),
                ("SecurityGroups.member.1", "sg-66666666"),
            ],
            vec!["sg-77777777"],
            "off",
            "",
        ),
        (
            "a gateway load balancer, which carries no security group at all",
            vec![("Type", "gateway")],
            vec!["sg-88888888"],
            "",
            INVALID_CONFIGURATION,
        ),
        (
            "a network load balancer created without a group",
            vec![("Type", "network")],
            vec!["sg-99999999"],
            "",
            INVALID_CONFIGURATION,
        ),
    ];

    for (index, (case, fields, groups, enforce, code)) in cases.iter().enumerate() {
        let name = format!("lb-sg-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let before = described_security_groups(&router, &arn).await;

        let mut form = action("SetSecurityGroups");
        form.push(p("LoadBalancerArn", &arn));
        for (position, group) in groups.iter().enumerate() {
            form.push(p(
                &format!("SecurityGroups.member.{}", position + 1),
                group,
            ));
        }
        if !enforce.is_empty() {
            form.push(p(
                "EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic",
                enforce,
            ));
        }
        let reply = call(&router, &form).await;

        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
            let answered = members(&reply.body, "SecurityGroupIds");
            assert_eq!(
                answered,
                groups.iter().map(|g| (*g).to_string()).collect::<Vec<_>>(),
                "case {case:?}: expected the stored groups answered, got {answered:?}"
            );
            let after = described_security_groups(&router, &arn).await;
            assert_eq!(
                after,
                groups.iter().map(|g| (*g).to_string()).collect::<Vec<_>>(),
                "case {case:?}: expected a later read to report the stored groups, got {after:?}"
            );
            if !enforce.is_empty() {
                let reported = described_member(
                    &router,
                    &arn,
                    "EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic",
                )
                .await;
                assert_eq!(
                    reported.as_deref(),
                    Some(*enforce),
                    "case {case:?}: expected the stored PrivateLink setting reported, got \
                     {reported:?}"
                );
            }
        } else {
            assert_refused(case, &reply, code, "");
            let after = described_security_groups(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused request must store nothing, got {after:?}"
            );
        }
    }
}

/// A network load balancer carrying a security group reports the documented default of
/// `EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic` ("The default is on") until a
/// caller sets it, and one carrying none reports nothing for a member that "applies only
/// if the load balancer has an associated security group".
#[tokio::test]
async fn the_private_link_setting_follows_the_documented_default() {
    let (router, _engine) = emulator();

    let with_group = create_load_balancer(
        &router,
        "lb-privatelink-on",
        &[
            ("Type", "network"),
            ("SecurityGroups.member.1", "sg-12121212"),
        ],
    )
    .await;
    let without_group =
        create_load_balancer(&router, "lb-privatelink-none", &[("Type", "network")]).await;

    let reported = described_member(
        &router,
        &with_group,
        "EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic",
    )
    .await;
    assert_eq!(
        reported.as_deref(),
        Some("on"),
        "case a network load balancer with a security group: expected the documented default, \
         got {reported:?}"
    );

    let reported = described_member(
        &router,
        &without_group,
        "EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic",
    )
    .await;
    assert_eq!(
        reported, None,
        "case a network load balancer without a security group: the member applies only with \
         one, got {reported:?}"
    );
}

/// The PrivateLink security group setting is available only on a Network Load Balancer
/// that has a security group, so a request carrying that member for anything else is
/// refused for THAT reason — including an ARN naming no load balancer at all, which is no
/// such load balancer either.
///
/// The rows span every load balancer the member can be asked for: each family, a network
/// load balancer without a security group, an absent ARN, and the one load balancer the
/// member IS available on.
#[tokio::test]
async fn the_private_link_setting_is_refused_where_it_is_not_available() {
    let (router, _engine) = emulator();
    let absent = absent_load_balancer_arn(REGION, "app", "absent-privatelink", "0000000000000000");

    // (case, the fields a load balancer is created with — `None` for the row that names
    // no load balancer at all, the error code expected — empty when served)
    type PrivateLinkCase = (
        &'static str,
        Option<Vec<(&'static str, &'static str)>>,
        &'static str,
    );
    let cases: Vec<PrivateLinkCase> = vec![
        (
            "an application load balancer",
            Some(vec![("SecurityGroups.member.1", "sg-aaaaaaa1")]),
            INVALID_CONFIGURATION,
        ),
        (
            "a network load balancer without a security group",
            Some(vec![("Type", "network")]),
            INVALID_CONFIGURATION,
        ),
        (
            "a gateway load balancer",
            Some(vec![("Type", "gateway")]),
            INVALID_CONFIGURATION,
        ),
        ("an ARN naming no load balancer", None, INVALID_CONFIGURATION),
        (
            "a network load balancer with a security group",
            Some(vec![
                ("Type", "network"),
                ("SecurityGroups.member.1", "sg-aaaaaaa2"),
            ]),
            "",
        ),
    ];

    for (index, (case, fields, code)) in cases.iter().enumerate() {
        let arn = match fields {
            Some(fields) => {
                let name = format!("lb-privatelink-{index}");
                create_load_balancer(&router, &name, fields).await
            }
            None => absent.clone(),
        };

        let reply = call_on(
            &router,
            "SetSecurityGroups",
            &arn,
            &[
                ("SecurityGroups.member.1", "sg-bbbbbbb1"),
                ("EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic", "on"),
            ],
        )
        .await;

        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
        } else {
            assert_refused(case, &reply, code, PRIVATE_LINK_SETTING_UNAVAILABLE);
        }
    }
}

// ---------------------------------------------------------------------------
// SetSubnets.
// ---------------------------------------------------------------------------

/// `SetSubnets` replaces the subnets — and so the Availability Zones — the load balancer
/// is placed in, and a later read reports exactly those; a request outside one of its
/// documented rules is refused and changes nothing — as the modelled
/// `InvalidConfigurationRequest` where the rule is a configuration no load balancer can
/// hold, and as `ValidationError` where it is a count the request itself has to satisfy.
#[tokio::test]
async fn set_subnets_replaces_the_placement_a_load_balancer_reports() {
    let (router, _engine) = emulator();

    // (case, the fields the load balancer is created with, the fields the request
    // carries, the subnets a later read must report — empty for a refused row, the error
    // code expected)
    type PlacementCase = (
        &'static str,
        Vec<(&'static str, &'static str)>,
        Vec<(&'static str, &'static str)>,
        Vec<&'static str>,
        &'static str,
    );
    let cases: Vec<PlacementCase> = vec![
        (
            "three subnets replacing the two it was created with",
            vec![],
            vec![
                ("Subnets.member.1", "subnet-aaaaaaa1"),
                ("Subnets.member.2", "subnet-aaaaaaa2"),
                ("Subnets.member.3", "subnet-aaaaaaa3"),
            ],
            vec!["subnet-aaaaaaa1", "subnet-aaaaaaa2", "subnet-aaaaaaa3"],
            "",
        ),
        (
            "two subnet mappings instead of a subnet list",
            vec![],
            vec![
                ("SubnetMappings.member.1.SubnetId", "subnet-bbbbbbb1"),
                ("SubnetMappings.member.2.SubnetId", "subnet-bbbbbbb2"),
            ],
            vec!["subnet-bbbbbbb1", "subnet-bbbbbbb2"],
            "",
        ),
        (
            "one subnet on a network load balancer, which may be placed in one zone",
            vec![("Type", "network")],
            vec![("Subnets.member.1", "subnet-ccccccc1")],
            vec!["subnet-ccccccc1"],
            "",
        ),
        (
            "a dualstack load balancer asking for IPv6 source NAT prefixes",
            vec![("Type", "network")],
            vec![
                ("Subnets.member.1", "subnet-ddddddd1"),
                ("IpAddressType", "dualstack"),
                ("EnablePrefixForIpv6SourceNat", "on"),
            ],
            vec!["subnet-ddddddd1"],
            "",
        ),
        (
            "both a subnet list and subnet mappings",
            vec![],
            vec![
                ("Subnets.member.1", "subnet-eeeeeee1"),
                ("SubnetMappings.member.1.SubnetId", "subnet-eeeeeee2"),
            ],
            vec![],
            INVALID_CONFIGURATION,
        ),
        (
            "neither a subnet list nor subnet mappings",
            vec![],
            vec![],
            vec![],
            INVALID_CONFIGURATION,
        ),
        (
            "one subnet on an application load balancer, which needs two zones",
            vec![],
            vec![("Subnets.member.1", "subnet-fffffff1")],
            vec![],
            VALIDATION_ERROR,
        ),
        (
            "IPv6 source NAT prefixes on a load balancer that is not dualstack",
            vec![("Type", "network")],
            vec![
                ("Subnets.member.1", "subnet-fffffff2"),
                ("EnablePrefixForIpv6SourceNat", "on"),
            ],
            vec![],
            INVALID_CONFIGURATION,
        ),
        (
            "an address type this family does not accept, asked for alongside the subnets",
            vec![("Type", "network")],
            vec![
                ("Subnets.member.1", "subnet-fffffff3"),
                ("IpAddressType", "dualstack-without-public-ipv4"),
            ],
            vec![],
            INVALID_CONFIGURATION,
        ),
    ];

    for (index, (case, fields, request, expected, code)) in cases.iter().enumerate() {
        let name = format!("lb-subnets-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let before = described_placement(&router, &arn).await;

        let reply = call_on(&router, "SetSubnets", &arn, request).await;
        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
            let placement = described_placement(&router, &arn).await;
            let subnets: Vec<String> = placement.iter().map(|(id, _)| id.clone()).collect();
            assert_eq!(
                subnets,
                expected.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
                "case {case:?}: expected a later read to report the subnets set, got \
                 {placement:?}"
            );
            // One subnet per Availability Zone: no two of the zones reported are the
            // same, whether EC2 named them or the service stood a name in.
            let mut zones: Vec<String> = placement.iter().map(|(_, zone)| zone.clone()).collect();
            zones.sort();
            let distinct = zones.len();
            zones.dedup();
            assert_eq!(
                zones.len(),
                distinct,
                "case {case:?}: two subnets must never read as one zone, got {placement:?}"
            );
            // The answer carries the same placement the read reports.
            let answered: Vec<String> = members(&reply.body, "AvailabilityZones")
                .iter()
                .map(|zone| first_text(zone, "SubnetId").unwrap_or_default())
                .collect();
            assert_eq!(
                answered, subnets,
                "case {case:?}: expected the answer to carry the stored placement, got \
                 {answered:?}"
            );
        } else {
            assert_refused(case, &reply, code, "");
            let after = described_placement(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused request must store nothing, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ModifyIpPools.
// ---------------------------------------------------------------------------

/// `ModifyIpPools` sets and removes the IPAM pool the load balancer draws from, and a
/// later read of the load balancer reports what it now carries.
///
/// The rows span the three shapes of such a request — set, remove, and both at once —
/// and the pool a load balancer is CREATED with, which the same member reports.
#[tokio::test]
async fn modify_ip_pools_stores_the_pools_a_load_balancer_draws_from() {
    let (router, _engine) = emulator();
    let pool = "ipam-pool-0123456789abcdef";
    let other_pool = "ipam-pool-fedcba9876543210";

    // A load balancer created with a pool reports it before anything modifies one.
    let created_with_pool = create_load_balancer(
        &router,
        "lb-pool-created",
        &[("IpamPools.Ipv4IpamPoolId", pool)],
    )
    .await;
    let reported = described_member(&router, &created_with_pool, "Ipv4IpamPoolId").await;
    assert_eq!(
        reported.as_deref(),
        Some(pool),
        "case a load balancer created with a pool: expected it reported, got {reported:?}"
    );

    // (case, the fields the request carries, the status it is answered with, the pool a
    // later read must report)
    type PoolStep<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, Option<&'a str>);
    let steps: Vec<PoolStep<'_>> = vec![
        (
            "setting a pool on a load balancer that drew from none",
            vec![("IpamPools.Ipv4IpamPoolId", pool)],
            200,
            Some(pool),
        ),
        (
            "replacing it with another pool",
            vec![("IpamPools.Ipv4IpamPoolId", other_pool)],
            200,
            Some(other_pool),
        ),
        (
            // A request naming NEITHER member asks for no change at all, so it is refused
            // and the pool it carried stays. The sentence and the code are pinned in
            // `crate::ip_pool_modification_members`; here the walk only has to show the
            // refusal changes nothing.
            "a request that names no pool at all, which is refused",
            vec![],
            400,
            Some(other_pool),
        ),
        (
            "removing the IPv4 pool",
            vec![("RemoveIpamPools.member.1", "ipv4")],
            200,
            None,
        ),
        (
            "setting and removing the same family in one request",
            vec![
                ("IpamPools.Ipv4IpamPoolId", pool),
                ("RemoveIpamPools.member.1", "ipv4"),
            ],
            200,
            None,
        ),
        (
            "setting it again after the removal",
            vec![("IpamPools.Ipv4IpamPoolId", pool)],
            200,
            Some(pool),
        ),
    ];

    let arn = create_load_balancer(&router, "lb-pool-modified", &[]).await;
    for (case, request, status, expected) in &steps {
        let reply = call_on(&router, "ModifyIpPools", &arn, request).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected {status}, got {} body {}",
            reply.status, reply.body
        );
        if *status == 200 {
            assert_eq!(
                reply.text("Ipv4IpamPoolId").as_deref(),
                *expected,
                "case {case:?}: expected the stored pool answered, got {}",
                reply.body
            );
        }
        let reported = described_member(&router, &arn, "Ipv4IpamPoolId").await;
        assert_eq!(
            reported.as_deref(),
            *expected,
            "case {case:?}: expected a later read to report the stored pool, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The capacity reservation.
// ---------------------------------------------------------------------------

/// Configuring minimum load balancer capacity is refused with the modelled
/// `OperationNotPermitted`, in the words AWS words that refusal; the other shapes of the
/// request are refused or served as the model documents.
///
/// The rows span what the request can ask for: minimum capacity (at two different
/// capacity-unit counts, and against both an existing and an absent load balancer, since
/// the account capability is decided before any lookup), a reset, both at once and
/// neither.
#[tokio::test]
async fn modify_capacity_reservation_refuses_what_this_account_cannot_configure() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-capacity", &[]).await;
    let absent = absent_load_balancer_arn(REGION, "app", "absent-capacity", "0000000000000000");

    let unsupported = "This AWS account does not support configuring minimum load balancer \
                       capacity reservation. For more information, please contact AWS Support.";

    let cases: Vec<Row> = vec![
        (
            "reserving 100 capacity units on a load balancer this store holds",
            vec![
                p("LoadBalancerArn", &arn),
                p("MinimumLoadBalancerCapacity.CapacityUnits", "100"),
            ],
            OPERATION_NOT_PERMITTED,
        ),
        (
            "reserving 1 capacity unit",
            vec![
                p("LoadBalancerArn", &arn),
                p("MinimumLoadBalancerCapacity.CapacityUnits", "1"),
            ],
            OPERATION_NOT_PERMITTED,
        ),
        (
            "reserving capacity on a load balancer nothing holds",
            vec![
                p("LoadBalancerArn", &absent),
                p("MinimumLoadBalancerCapacity.CapacityUnits", "100"),
            ],
            OPERATION_NOT_PERMITTED,
        ),
        (
            "asking for neither a minimum capacity nor a reset",
            vec![p("LoadBalancerArn", &arn)],
            INVALID_CONFIGURATION,
        ),
        (
            "asking for a reset that is not asked for",
            vec![
                p("LoadBalancerArn", &arn),
                p("ResetCapacityReservation", "false"),
            ],
            INVALID_CONFIGURATION,
        ),
        (
            "asking for both a minimum capacity and a reset",
            vec![
                p("LoadBalancerArn", &arn),
                p("MinimumLoadBalancerCapacity.CapacityUnits", "100"),
                p("ResetCapacityReservation", "true"),
            ],
            INVALID_CONFIGURATION,
        ),
    ];

    for (case, fields, code) in &cases {
        let mut form = action("ModifyCapacityReservation");
        form.extend(fields.iter().cloned());
        let reply = call(&router, &form).await;
        assert_refused(case, &reply, code, "");
        if *code == OPERATION_NOT_PERMITTED {
            assert_eq!(
                reply.text("Message").map(|text| unescaped(&text)).as_deref(),
                Some(unsupported),
                "case {case:?}: unexpected message in {}",
                reply.body
            );
        }
    }
}

/// A load balancer nothing has written a reservation for reports none, a reset is served
/// and recorded, and the read answers the instant the write recorded.
#[tokio::test]
async fn the_capacity_reservation_a_read_reports_is_the_one_the_store_holds() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-capacity-read", &[]).await;

    let reply = call_on(&router, "DescribeCapacityReservation", &arn, &[]).await;
    assert_eq!(
        reply.status, 200,
        "case a fresh load balancer: expected 200, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.text("LastModifiedTime"),
        None,
        "case a fresh load balancer: nothing has written a reservation, got {}",
        reply.body
    );
    assert_eq!(
        reply.text("CapacityUnits"),
        None,
        "case a fresh load balancer: it reserves no minimum capacity, got {}",
        reply.body
    );

    let reset = call_on(
        &router,
        "ModifyCapacityReservation",
        &arn,
        &[("ResetCapacityReservation", "true")],
    )
    .await;
    assert_eq!(
        reset.status, 200,
        "case resetting the reservation: expected 200, got {} body {}",
        reset.status, reset.body
    );
    let recorded = reset.text("LastModifiedTime");
    assert!(
        recorded.is_some(),
        "case resetting the reservation: the write records when it happened, got {}",
        reset.body
    );

    let reply = call_on(&router, "DescribeCapacityReservation", &arn, &[]).await;
    assert_eq!(
        reply.text("LastModifiedTime"),
        recorded,
        "case reading the reservation back: expected the instant the write recorded, got {}",
        reply.body
    );
    assert_eq!(
        reply.text("CapacityUnits"),
        None,
        "case reading the reservation back: a reset leaves no minimum capacity, got {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// GetResourcePolicy.
// ---------------------------------------------------------------------------

/// `GetResourcePolicy` answers the policy attached to the trust store the ARN names, and
/// refuses every other ARN — each for its own reason: a value that is no ARN is refused
/// for its form, a resource type this API reads no policy for is refused for its type,
/// and a trust store ARN this store holds no trust store under is the modelled
/// `ResourceNotFound` naming the ARN the request carried.
///
/// The rows span the shapes of a value that resolves to nothing: a trust store this store
/// does not hold, two kinds of this service's own resource that carry no resource policy,
/// an ARN of another service, and three values that are no ARN.
#[tokio::test]
async fn get_resource_policy_answers_the_policy_the_store_holds() {
    let (router, engine) = emulator();

    // A trust store that was shared carries the policy that share attached. Seeded,
    // because no operation of this model attaches a policy: sharing is AWS RAM's, and this
    // model only reads the policy back.
    let shared = seed_trust_store(
        &engine,
        0,
        &TrustStoreSeed {
            name: "shared-store",
            settled: true,
            ..TrustStoreSeed::default()
        },
    );
    let unshared = seed_trust_store(
        &engine,
        1,
        &TrustStoreSeed {
            name: "unshared-store",
            settled: true,
            ..TrustStoreSeed::default()
        },
    );
    let policy = r#"{"Version":"2012-10-17","Statement":[]}"#;
    attach_policy(&engine, &shared, policy);

    let mut form = action("GetResourcePolicy");
    form.push(p("ResourceArn", &shared));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case a shared trust store: expected 200, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.text("Policy").map(|text| unescaped(&text)).as_deref(),
        Some(policy),
        "case a shared trust store: expected the stored policy, got {}",
        reply.body
    );

    // A trust store this store HOLDS and no policy is attached to is reported, not
    // refused: the absence of a policy is in the body. Spanned over every shape of an
    // attached and unattached policy by
    // `get_resource_policy_reports_an_unattached_policy_as_the_empty_document` below.
    let mut form = action("GetResourcePolicy");
    form.push(p("ResourceArn", &unshared));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.text("Policy").map(|text| unescaped(&text))),
        (200, Some(EMPTY_RESOURCE_POLICY.to_string())),
        "case a trust store no policy is attached to: expected 200 and the empty policy \
         document, got {} body {}",
        reply.status,
        reply.body
    );

    let load_balancer = create_load_balancer(&router, "lb-policy-probe", &[]).await;
    let absent_trust_store = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:truststore/not-here/0123456789abcdef",
        account()
    );
    let target_group = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/not-here/0123456789abcdef",
        account()
    );

    // A trust store ARN this store holds no trust store under: the modelled refusal,
    // naming the ARN.
    let not_found: Vec<(&str, String)> = vec![(
        "a trust store this store does not hold",
        absent_trust_store,
    )];
    for (case, arn) in &not_found {
        let mut form = action("GetResourcePolicy");
        form.push(p("ResourceArn", arn));
        let reply = call(&router, &form).await;
        assert_refused(case, &reply, RESOURCE_NOT_FOUND, arn);
        assert_eq!(
            reply.text("Message").map(|text| unescaped(&text)),
            Some(format!("Trust store ARN '{arn}' not found")),
            "case {case:?}: unexpected message in {}",
            reply.body
        );
    }

    // A resource type this API reads no policy for: refused for its TYPE, whatever the
    // store holds.
    let unsupported: Vec<(&str, String)> = vec![
        (
            "a load balancer, which carries no resource policy",
            load_balancer,
        ),
        (
            "a target group, which carries no resource policy",
            target_group,
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:::bucket-{}", account()),
        ),
    ];
    for (case, arn) in &unsupported {
        let mut form = action("GetResourcePolicy");
        form.push(p("ResourceArn", arn));
        let reply = call(&router, &form).await;
        assert_refused(case, &reply, VALIDATION_ERROR, "");
        assert_eq!(
            reply.text("Message").map(|text| unescaped(&text)),
            Some("The specified resource type is not supported".to_string()),
            "case {case:?}: unexpected message in {}",
            reply.body
        );
    }

    // A value that is no ARN at all: refused for its FORM, naming the value sent.
    let malformed: Vec<(&str, &str)> = vec![
        ("a bare word", "not-an-arn"),
        ("a value with too few ARN fields", "arn:aws:elasticloadbalancing"),
        ("an ARN whose resource is empty", "arn:aws:elasticloadbalancing:us-west-2:000000000000:"),
    ];
    for (case, arn) in &malformed {
        let mut form = action("GetResourcePolicy");
        form.push(p("ResourceArn", arn));
        let reply = call(&router, &form).await;
        assert_refused(case, &reply, VALIDATION_ERROR, arn);
        assert_eq!(
            reply.text("Message").map(|text| unescaped(&text)),
            Some(format!("'{arn}' must be in ARN format")),
            "case {case:?}: unexpected message in {}",
            reply.body
        );
    }

    // A request that states nothing in the member names no resource at all, which is its
    // own refusal: there is no value to quote back.
    let mut form = action("GetResourcePolicy");
    form.push(p("ResourceArn", ""));
    let reply = call(&router, &form).await;
    assert_refused("an empty ResourceArn", &reply, VALIDATION_ERROR, "");
    assert_eq!(
        reply.text("Message").map(|text| unescaped(&text)),
        Some("An ARN must be specified".to_string()),
        "case an empty ResourceArn: unexpected message in {}",
        reply.body
    );
}

/// Attach `policy` to the trust store `arn` names, through the same store a handler
/// reads: what sharing the trust store would have written.
fn attach_policy(engine: &Arc<engine::InMemoryEngine>, arn: &str, policy: &str) {
    let state = engine.state::<ElbState>();
    let mut store = state.lock();
    store
        .trust_stores
        .get_mut(arn)
        .expect("the trust store seeded above")
        .resource_policy = Some(policy.to_string());
}

/// What a policy read reports for a trust store this store HOLDS: the document attached to
/// it, or the empty policy document for one nothing attached a policy to.
///
/// Whether a policy is attached is a fact about the BODY, never a reason to refuse — the
/// modelled `ResourceNotFound` is reserved for an ARN this store resolves no trust store
/// from, which is the row at the end. The served rows differ along what the rule
/// generalises over: whether a policy is attached at all, the document attached (the empty
/// object, one statement, several statements, a document carrying the characters awsQuery
/// XML has to escape), and whether the clock still owes the trust store its move to
/// `ACTIVE` — a trust store inside its settling span exists, so its policy reads back too.
///
/// The read reports and writes nothing, so every row is read TWICE and must answer the
/// same both times: a first read must not turn an unattached policy into a stored one.
#[tokio::test]
async fn get_resource_policy_reports_an_unattached_policy_as_the_empty_document() {
    let (router, engine) = emulator();

    // Each row: the case, whether the clock has settled the trust store, and the policy a
    // share attached to it — `None` for a trust store that was never shared.
    let rows: Vec<(&str, bool, Option<&str>)> = vec![
        ("no policy attached, settled", true, None),
        ("no policy attached, still CREATING", false, None),
        (
            "a policy with no statements attached",
            true,
            Some(r#"{"Version":"2012-10-17","Statement":[]}"#),
        ),
        (
            "a policy with one statement attached",
            true,
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"210987654321"},"Action":"elasticloadbalancing:*","Resource":"*"}]}"#,
            ),
        ),
        (
            "a policy with several statements attached",
            true,
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"210987654321"},"Action":"elasticloadbalancing:DescribeTrustStores","Resource":"*"},{"Effect":"Deny","Principal":{"AWS":"310987654321"},"Action":"elasticloadbalancing:*","Resource":"*"}]}"#,
            ),
        ),
        (
            "a policy carrying characters XML escapes",
            true,
            Some(r#"{"Comment":"a & b <c> \"d\"","Statement":[]}"#),
        ),
        (
            "the empty policy document attached outright",
            true,
            Some(EMPTY_RESOURCE_POLICY),
        ),
    ];

    for (index, (case, settled, attached)) in rows.iter().enumerate() {
        let arn = seed_trust_store(
            &engine,
            index,
            &TrustStoreSeed {
                name: &format!("policy-row-{index}"),
                settled: *settled,
                ..TrustStoreSeed::default()
            },
        );
        if let Some(policy) = attached {
            attach_policy(&engine, &arn, policy);
        }
        let expected = attached.unwrap_or(EMPTY_RESOURCE_POLICY).to_string();
        for read in ["the first read", "the read after it"] {
            let mut form = action("GetResourcePolicy");
            form.push(p("ResourceArn", &arn));
            let reply = call(&router, &form).await;
            assert_eq!(
                (
                    reply.status,
                    reply.text("Policy").map(|text| unescaped(&text))
                ),
                (200, Some(expected.clone())),
                "case {case:?} on {read}: expected 200 and {expected:?}, got {} body {}",
                reply.status,
                reply.body
            );
        }
    }

    // The one case the read resolves nothing from: an ARN of the form this service mints
    // that no trust store of this store carries.
    let absent = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:truststore/never-created/00000000000000ff",
        account()
    );
    let mut form = action("GetResourcePolicy");
    form.push(p("ResourceArn", &absent));
    let reply = call(&router, &form).await;
    assert_refused(
        "a trust store this store holds none under",
        &reply,
        RESOURCE_NOT_FOUND,
        &absent,
    );
    assert_eq!(
        reply.text("Message").map(|text| unescaped(&text)),
        Some(format!("Trust store ARN '{absent}' not found")),
        "case a trust store this store holds none under: unexpected message in {}",
        reply.body
    );
}
