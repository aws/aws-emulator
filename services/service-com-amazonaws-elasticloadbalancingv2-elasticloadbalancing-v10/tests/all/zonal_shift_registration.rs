// SPDX-License-Identifier: CC0-1.0

//! Whether a load balancer of this service is a MANAGED RESOURCE of Amazon Application
//! Recovery Controller, as the service that REPORTS managed resources sees it.
//!
//! RULE: turning zonal-shift integration on for a load balancer — the
//! `zonal_shift.config.enabled` attribute, "Indicates whether zonal shift is enabled" —
//! makes it one of the resources ARC manages, so the load balancer is recorded in the ONE
//! shared registry ARC reads, under its own ARN and name and spanning the Availability
//! Zones this store puts it in; the integration turned off, the load balancer deleted, or
//! an attribute write that never names the key leaves it unregistered, and a load balancer
//! given different subnets is recorded again with the zones it now spans.
//!
//! The registry is read here exactly as ARC reads it, from the resource side. The zones
//! asserted on are the ones this service ANSWERED for the load balancer, never a value
//! this suite made up: with no EC2 mounted beside it the zone a subnet puts a load balancer
//! in is the zone NAME this store holds, which is what is recorded for a subnet no
//! Availability Zone ID is known for. The other half of the rule — the zone IDs EC2 reports
//! for real subnets, and ARC answering on them — is
//! `core/router/tests/all/elbv2_zonal_shift_managed_resource.rs`.
//!
//! The rows differ along what the rule generalises over: the TYPE of load balancer (both
//! the families the attribute's documentation lists, plus the one that carries no such
//! attribute at all), how MANY subnets it is placed in, and the value the attribute is
//! written with.

use std::sync::Arc;

use service_common::arc_managed_resources::{ManagedResourceRegistry, RegisteredResource};
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioning_loop::{action, call, emulator, first_text, members, p, Reply};

/// The attribute the rule is about, read from the service rather than restated here.
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ZONAL_SHIFT_ATTRIBUTE;

/// What the shared registry holds for `arn`, read the way ARC reads it.
fn registered(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> Option<RegisteredResource> {
    engine
        .state::<ManagedResourceRegistry>()
        .lock()
        .get(arn)
        .cloned()
}

/// Create a load balancer of `lb_type` across `subnets` and answer the reply.
async fn create_load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    lb_type: &str,
    subnets: &[&str],
) -> Reply {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", lb_type));
    // A Gateway Load Balancer carries no scheme at all — "you can't specify a scheme for a
    // Gateway Load Balancer" — so one is named only for the families that do.
    if lb_type != "gateway" {
        form.push(p("Scheme", "internal"));
    }
    for (index, subnet) in subnets.iter().enumerate() {
        form.push(p(&format!("Subnets.member.{}", index + 1), subnet));
    }
    call(router, &form).await
}

/// The zone NAMES this service answered for a load balancer, in the order it answered
/// them: `AvailabilityZones.member.*.ZoneName` of a create or a `SetSubnets` reply.
fn answered_zones(body: &str) -> Vec<String> {
    members(body, "AvailabilityZones")
        .iter()
        .filter_map(|member| first_text(member, "ZoneName"))
        .collect()
}

/// Write one load balancer attribute and answer the reply.
async fn write_attribute(
    router: &MultiServiceRouter,
    lb_arn: &str,
    key: &str,
    value: &str,
) -> Reply {
    let mut form = action("ModifyLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Attributes.member.1.Key", key));
    form.push(p("Attributes.member.1.Value", value));
    call(router, &form).await
}

/// One load balancer the rule is asserted over.
struct LoadBalancerRow {
    case: &'static str,
    name: &'static str,
    /// The `Type` of the load balancer, from `LoadBalancerTypeEnum`.
    lb_type: &'static str,
    /// The subnets it is placed in — one zone per subnet.
    subnets: &'static [&'static str],
}

/// The load balancers the attribute's documentation lists it for: "zonal_shift.config.enabled
/// ... supported by both Application Load Balancers and Network Load Balancers", across the
/// subnet counts one can be placed in.
const CARRIES_THE_ATTRIBUTE: &[LoadBalancerRow] = &[
    LoadBalancerRow {
        case: "a Network Load Balancer in two subnets",
        name: "zs-nlb-two",
        lb_type: "network",
        subnets: &["subnet-11111111", "subnet-22222222"],
    },
    LoadBalancerRow {
        case: "an Application Load Balancer in three subnets",
        name: "zs-alb-three",
        lb_type: "application",
        subnets: &["subnet-11111111", "subnet-22222222", "subnet-33333333"],
    },
    LoadBalancerRow {
        case: "a Network Load Balancer in one subnet",
        name: "zs-nlb-one",
        lb_type: "network",
        subnets: &["subnet-44444444"],
    },
];

#[tokio::test]
async fn a_load_balancer_with_zonal_shift_enabled_is_registered_with_arc() {
    for row in CARRIES_THE_ATTRIBUTE {
        let case = row.case;
        // One engine per row, so each row's assertions are about its own store.
        let (router, engine) = emulator();
        let created = create_load_balancer(&router, row.name, row.lb_type, row.subnets).await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateLoadBalancer must answer 200, got {} {}",
            created.status, created.body
        );
        let lb_arn = created
            .text("LoadBalancerArn")
            .unwrap_or_else(|| panic!("case {case}: no LoadBalancerArn in {}", created.body));
        let zones = answered_zones(&created.body);
        assert_eq!(
            zones.len(),
            row.subnets.len(),
            "case {case}: expected one zone per subnet, got {zones:?}"
        );

        // A load balancer nobody turned the integration on for is no managed resource:
        // the attribute's documented default is false.
        assert_eq!(
            registered(&engine, &lb_arn),
            None,
            "case {case}: a load balancer with the integration off must not be registered, \
             got {:?}",
            registered(&engine, &lb_arn)
        );

        // Writing ANOTHER attribute does not register it either.
        let other = write_attribute(&router, &lb_arn, "deletion_protection.enabled", "true").await;
        assert_eq!(
            other.status, 200,
            "case {case}: the unrelated attribute write must answer 200, got {} {}",
            other.status, other.body
        );
        assert_eq!(
            registered(&engine, &lb_arn),
            None,
            "case {case}: an attribute write that names no zonal-shift key must register \
             nothing, got {:?}",
            registered(&engine, &lb_arn)
        );

        // Turned on, it is a managed resource spanning the zones this store puts it in.
        let enabled = write_attribute(&router, &lb_arn, ZONAL_SHIFT_ATTRIBUTE, "true").await;
        assert_eq!(
            enabled.status, 200,
            "case {case}: enabling zonal shift must answer 200, got {} {}",
            enabled.status, enabled.body
        );
        assert_eq!(
            registered(&engine, &lb_arn),
            Some(RegisteredResource {
                arn: lb_arn.clone(),
                name: row.name.to_string(),
                availability_zones: zones.clone(),
            }),
            "case {case}: expected the load balancer registered across {zones:?}, got {:?}",
            registered(&engine, &lb_arn)
        );

        // Writing the same value again changes nothing; writing false withdraws it.
        let again = write_attribute(&router, &lb_arn, ZONAL_SHIFT_ATTRIBUTE, "true").await;
        assert_eq!(
            (again.status, registered(&engine, &lb_arn).is_some()),
            (200, true),
            "case {case}: a repeated enable must leave it registered, got {} {}",
            again.status,
            again.body
        );
        let disabled = write_attribute(&router, &lb_arn, ZONAL_SHIFT_ATTRIBUTE, "false").await;
        assert_eq!(
            disabled.status, 200,
            "case {case}: disabling zonal shift must answer 200, got {} {}",
            disabled.status, disabled.body
        );
        assert_eq!(
            registered(&engine, &lb_arn),
            None,
            "case {case}: the integration turned off must withdraw the registration, got {:?}",
            registered(&engine, &lb_arn)
        );
    }
}

#[tokio::test]
async fn the_registration_follows_the_zones_the_load_balancer_spans_and_its_deletion() {
    let row = &CARRIES_THE_ATTRIBUTE[0];
    let case = row.case;
    let (router, engine) = emulator();
    let created = create_load_balancer(&router, row.name, row.lb_type, row.subnets).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateLoadBalancer must answer 200, got {} {}",
        created.status, created.body
    );
    let lb_arn = created
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {case}: no LoadBalancerArn in {}", created.body));
    let enabled = write_attribute(&router, &lb_arn, ZONAL_SHIFT_ATTRIBUTE, "true").await;
    assert_eq!(
        enabled.status, 200,
        "case {case}: enabling zonal shift must answer 200, got {} {}",
        enabled.status, enabled.body
    );

    // Given different subnets, the registration spans the zones it now has: the rows are
    // the subnet sets `SetSubnets` is called with, which differ in count and in identity.
    let subnet_sets: &[(&str, &[&str])] = &[
        (
            "three subnets",
            &["subnet-11111111", "subnet-22222222", "subnet-33333333"],
        ),
        ("two other subnets", &["subnet-55555555", "subnet-66666666"]),
    ];
    for (change, subnets) in subnet_sets {
        let mut form = action("SetSubnets");
        form.push(p("LoadBalancerArn", &lb_arn));
        for (index, subnet) in subnets.iter().enumerate() {
            form.push(p(&format!("Subnets.member.{}", index + 1), subnet));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case}, {change}: SetSubnets must answer 200, got {} {}",
            reply.status, reply.body
        );
        let zones = answered_zones(&reply.body);
        assert_eq!(
            registered(&engine, &lb_arn).map(|entry| entry.availability_zones),
            Some(zones.clone()),
            "case {case}, {change}: expected the registration to span {zones:?}, got {:?}",
            registered(&engine, &lb_arn)
        );
    }

    // Deleted, it is no managed resource at all.
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &lb_arn));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: DeleteLoadBalancer must answer 200, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        registered(&engine, &lb_arn),
        None,
        "case {case}: a deleted load balancer must not stay registered, got {:?}",
        registered(&engine, &lb_arn)
    );
}

#[tokio::test]
async fn an_identifier_this_store_holds_no_load_balancer_for_publishes_nothing() {
    // The registry is shared with every other service of the process, so the only
    // identifiers this service may name in it are the load balancer ARNs it minted and
    // holds itself. A delete naming anything else succeeds — "deleting a load balancer
    // that is not there succeeds" — and must leave the registry exactly as it was.
    let row = &CARRIES_THE_ATTRIBUTE[0];
    let (router, engine) = emulator();
    let created = create_load_balancer(&router, row.name, row.lb_type, row.subnets).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateLoadBalancer must answer 200, got {} {}",
        created.status, created.body
    );
    let lb_arn = created
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("setup: no LoadBalancerArn in {}", created.body));
    let enabled = write_attribute(&router, &lb_arn, ZONAL_SHIFT_ATTRIBUTE, "true").await;
    assert_eq!(
        enabled.status, 200,
        "setup: enabling zonal shift must answer 200, got {} {}",
        enabled.status, enabled.body
    );
    let published = registered(&engine, &lb_arn);
    assert!(
        published.is_some(),
        "setup: expected the load balancer registered, got {published:?}"
    );

    // The rows differ in the FORM of the identifier that names no load balancer of this
    // store: a well-formed ARN of this service in another account, one in another Region,
    // a name rather than an ARN, and the registered load balancer's own ARN with one
    // character changed.
    let mut altered = lb_arn.clone();
    altered.push('0');
    let rows: &[(&str, String)] = &[
        (
            "a load balancer ARN in another account",
            "arn:aws:elasticloadbalancing:us-west-2:111111111111:loadbalancer/net/other\
             /0123456789abcdef"
                .to_string(),
        ),
        (
            "a load balancer ARN in another Region",
            "arn:aws:elasticloadbalancing:eu-west-1:000000000000:loadbalancer/net/other\
             /0123456789abcdef"
                .to_string(),
        ),
        ("a name rather than an ARN", row.name.to_string()),
        ("the registered ARN with a character added", altered),
    ];
    for (case, identifier) in rows {
        let mut form = action("DeleteLoadBalancer");
        form.push(p("LoadBalancerArn", identifier));
        let reply = call(&router, &form).await;
        assert_eq!(
            registered(&engine, &lb_arn),
            published,
            "case {case}: a delete naming no load balancer of this store must leave the \
             registry untouched, got {:?} (the delete answered {} {})",
            registered(&engine, &lb_arn),
            reply.status,
            reply.body
        );
        assert_eq!(
            registered(&engine, identifier),
            None,
            "case {case}: an identifier this store holds no load balancer for must be \
             registered by nothing, got {:?}",
            registered(&engine, identifier)
        );
    }
}

#[tokio::test]
async fn a_load_balancer_type_that_carries_no_such_attribute_registers_nothing() {
    // The attribute's documentation lists it for Application and Network Load Balancers
    // only, so a Gateway Load Balancer refuses the write — and a refused write registers
    // nothing, exactly as it stores nothing.
    let case = "a Gateway Load Balancer";
    let (router, engine) = emulator();
    let created = create_load_balancer(
        &router,
        "zs-gwlb",
        "gateway",
        &["subnet-11111111", "subnet-22222222"],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateLoadBalancer must answer 200, got {} {}",
        created.status, created.body
    );
    let lb_arn = created
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {case}: no LoadBalancerArn in {}", created.body));

    let refused = write_attribute(&router, &lb_arn, ZONAL_SHIFT_ATTRIBUTE, "true").await;
    assert_eq!(
        refused.status, 400,
        "case {case}: a key this type does not carry must be refused, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        registered(&engine, &lb_arn),
        None,
        "case {case}: a refused write must register nothing, got {:?}",
        registered(&engine, &lb_arn)
    );
}
