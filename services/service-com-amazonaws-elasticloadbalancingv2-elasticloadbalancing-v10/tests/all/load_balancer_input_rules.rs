// SPDX-License-Identifier: CC0-1.0

//! What `CreateLoadBalancer` accepts, and what it refuses before it mints anything.
//!
//! Every rule under test is a clause of this operation's OWN modelled documentation, and
//! each is asserted over a TABLE whose rows differ along what the clause generalises over
//! — the length, the characters and the prefix of a name; the family and the scheme a
//! subnet mapping's pinned address is documented for; the number of subnets a family may
//! be placed in; the IP address type a scheme may carry:
//!
//! * `Name` — "can have a maximum of 32 characters, must contain only alphanumeric
//!   characters or hyphens, must not begin or end with a hyphen, and must not begin with
//!   `internal-`", and "must be unique per region per account", which is the duplicate
//!   refusal and the documented idempotent repeat;
//! * `SubnetMappings` — "[Application Load Balancers] ... You can't specify Elastic IP
//!   addresses for your subnets", "[Gateway Load Balancers] ... You can't specify Elastic
//!   IP addresses for your subnets", and "[Network Load Balancers] ... one Elastic IP
//!   address per subnet ... for your internet-facing load balancer. For internal load
//!   balancers, you can specify one private IP address per subnet ... For internet-facing
//!   load balancer, you can specify one IPv6 address per subnet";
//! * `IpAddressType` — `dualstack-without-public-ipv4` is an Application Load Balancer's
//!   alone, and the load balancer's scheme decides nothing;
//! * `CanonicalHostedZoneId` — the Route 53 hosted zone Elastic Load Balancing publishes
//!   for the load balancer's Region and family.
//!
//! A refused create must also store NOTHING, so every refusal row reads the name back
//! through `DescribeLoadBalancers` and expects the not-found refusal. Every request goes
//! through the router exactly as a client's would — awsQuery form body in, XML out — and
//! the harness is `all/provisioning_loop.rs`'s, reused rather than written twice.

use crate::provisioning_loop::{action, call, call_in_region, emulator, first_text, members, p};
use crate::target_group_attributes_and_modification::unescaped;
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code Elastic Load Balancing answers a value it cannot accept with,
/// for a rule its model documents in prose and declares no error shape for.
const VALIDATION_ERROR: &str = "ValidationError";

/// The modelled refusal for a request whose members cannot hold together.
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The modelled refusal for a name the account already holds, with different settings.
const DUPLICATE_NAME: &str = "DuplicateLoadBalancerName";

/// A `CreateLoadBalancer` request for `name` with `extra` fields and two subnets, which is
/// the smallest placement every family accepts.
fn create(name: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-aaaa0001"));
    form.push(p("Subnets.member.2", "subnet-aaaa0002"));
    for (key, value) in extra {
        form.push(p(key, value));
    }
    form
}

/// Whether the store holds a load balancer called `name`, read back the way a client
/// reads it.
async fn stored(router: &MultiServiceRouter, name: &str) -> bool {
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", name));
    let reply = call(router, &form).await;
    reply.status == 200 && !members(&reply.body, "LoadBalancers").is_empty()
}

#[tokio::test]
async fn a_load_balancer_name_outside_its_documented_form_is_refused() {
    let (router, _engine) = emulator();

    // (case, the name, whether it is accepted)
    //
    // The rows walk every clause of `Name`'s documentation and the value on either side of
    // each bound it fixes: the 32nd character and the 33rd, a character the name may not
    // hold, a hyphen at each end, and the one reserved prefix.
    let rows: [(&str, &str, bool); 10] = [
        ("a plain name", "plain-lb", true),
        ("a name of digits alone", "20261023", true),
        ("exactly 32 characters", "a2345678901234567890123456789012", true),
        ("a hyphen inside the name", "a-b-c-d", true),
        ("33 characters", "a23456789012345678901234567890123", false),
        ("an underscore", "under_score", false),
        ("a dot", "dotted.name", false),
        ("a leading hyphen", "-leading", false),
        ("a trailing hyphen", "trailing-", false),
        ("the reserved prefix", "internal-lb", false),
    ];

    for (case, name, accepted) in rows {
        let reply = call(&router, &create(name, &[])).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the name to be accepted, got {} body {}",
                reply.status, reply.body
            );
            assert!(
                stored(&router, name).await,
                "case {case:?}: an accepted create must store the load balancer"
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, VALIDATION_ERROR),
                "case {case:?}: expected 400 {VALIDATION_ERROR}, got {} body {}",
                reply.status,
                reply.body
            );
            assert!(
                !stored(&router, name).await,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }

    // An empty name is refused under the same code, and names the member rather than a
    // value: there is none to quote.
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", ""));
    form.push(p("Subnets.member.1", "subnet-aaaa0001"));
    form.push(p("Subnets.member.2", "subnet-aaaa0002"));
    let empty = call(&router, &form).await;
    assert_eq!(
        (empty.status, empty.error_code().as_str()),
        (400, VALIDATION_ERROR),
        "case an empty name: expected 400 {VALIDATION_ERROR}, got {} body {}",
        empty.status,
        empty.body
    );
}

#[tokio::test]
async fn a_placement_is_bounded_by_the_zones_a_region_has_however_it_is_spelled() {
    let (router, _engine) = emulator();

    // (case, how many `Subnets` entries, how many `SubnetMappings` entries, whether a
    // mapping spells its `SubnetId` out, whether the request is accepted)
    //
    // "You can specify only one subnet per Availability Zone", so the ceiling is the
    // number of zones a Region has — 26, as this service names them. What the request is
    // bounded by is the number of PLACEMENTS it asks for: a `SubnetMappings` entry places
    // the load balancer in one subnet whether or not it names that subnet, so a request of
    // 27 mappings is past the ceiling even when every one of them carries only an address.
    type PlacementCase = (&'static str, usize, usize, bool, bool);
    let rows: [PlacementCase; 6] = [
        ("one subnet per zone of the region", 26, 0, false, true),
        ("one more than the region has zones", 27, 0, false, false),
        ("one mapping per zone of the region", 0, 26, true, true),
        ("one mapping more than the region has zones", 0, 27, true, false),
        (
            "mappings that name only an address, one per zone",
            0,
            26,
            false,
            true,
        ),
        (
            "mappings that name only an address, one zone too many",
            0,
            27,
            false,
            false,
        ),
    ];

    for (index, (case, subnets, mappings, spell_subnet, accepted)) in rows.into_iter().enumerate() {
        let name = format!("placement-{index}");
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", &name));
        // A network load balancer, which is the family that may be placed in one zone, so
        // every row is judged by the ceiling alone.
        form.push(p("Type", "network"));
        for member in 1..=subnets {
            form.push(p(
                &format!("Subnets.member.{member}"),
                &format!("subnet-{member:017x}"),
            ));
        }
        for member in 1..=mappings {
            if spell_subnet {
                form.push(p(
                    &format!("SubnetMappings.member.{member}.SubnetId"),
                    &format!("subnet-{member:017x}"),
                ));
            }
            form.push(p(
                &format!("SubnetMappings.member.{member}.AllocationId"),
                &format!("eipalloc-{member:017x}"),
            ));
        }
        let reply = call(&router, &form).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the placement to be accepted, got {} body {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, INVALID_CONFIGURATION),
                "case {case:?}: expected 400 {INVALID_CONFIGURATION}, got {} body {}",
                reply.status,
                reply.body
            );
            assert!(
                !stored(&router, &name).await,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn a_subnet_mapping_pins_only_what_the_family_and_scheme_carry() {
    let (router, _engine) = emulator();

    // (case, the type, the scheme, the mapping member, its value, whether it is accepted)
    //
    // One row per (member, family, scheme) the documentation pairs or forbids, so the table
    // covers the member that applies to one scheme of one family and the families that
    // carry none of them at all.
    type MappingCase = (&'static str, &'static str, &'static str, &'static str, &'static str, bool);
    let rows: [MappingCase; 11] = [
        (
            "an elastic ip on an internet-facing network load balancer",
            "network",
            "internet-facing",
            "AllocationId",
            "eipalloc-0abc1",
            true,
        ),
        (
            "an elastic ip on an internal network load balancer",
            "network",
            "internal",
            "AllocationId",
            "eipalloc-0abc2",
            false,
        ),
        (
            "an elastic ip on an application load balancer",
            "application",
            "internet-facing",
            "AllocationId",
            "eipalloc-0abc3",
            false,
        ),
        (
            // A Gateway Load Balancer carries no scheme, so this row names none: what it
            // asserts is that the family carries no Elastic IP.
            "an elastic ip on a gateway load balancer",
            "gateway",
            "",
            "AllocationId",
            "eipalloc-0abc4",
            false,
        ),
        (
            "a private address on an internal network load balancer",
            "network",
            "internal",
            "PrivateIPv4Address",
            "10.0.1.5",
            true,
        ),
        (
            "a private address on an internet-facing network load balancer",
            "network",
            "internet-facing",
            "PrivateIPv4Address",
            "10.0.1.6",
            false,
        ),
        (
            "a private address on an application load balancer",
            "application",
            "internal",
            "PrivateIPv4Address",
            "10.0.1.7",
            false,
        ),
        (
            "an IPv6 address on an internet-facing network load balancer",
            "network",
            "internet-facing",
            "IPv6Address",
            "2001:db8::1",
            true,
        ),
        (
            "an IPv6 address on an internal network load balancer",
            "network",
            "internal",
            "IPv6Address",
            "2001:db8::2",
            false,
        ),
        (
            "a source NAT prefix on a network load balancer",
            "network",
            "internet-facing",
            "SourceNatIpv6Prefix",
            "auto_assigned",
            true,
        ),
        (
            "a source NAT prefix on an application load balancer",
            "application",
            "internet-facing",
            "SourceNatIpv6Prefix",
            "auto_assigned",
            false,
        ),
    ];

    for (index, (case, lb_type, scheme, member, value, accepted)) in rows.into_iter().enumerate() {
        let name = format!("mapping-{index}");
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", &name));
        form.push(p("Type", lb_type));
        // A row whose family carries no scheme names none.
        if !scheme.is_empty() {
            form.push(p("Scheme", scheme));
        }
        // Two mappings, so an Application Load Balancer's own floor of two zones is met
        // whatever the row asserts, and the pinned member is on the first of them.
        form.push(p(
            "SubnetMappings.member.1.SubnetId",
            &format!("subnet-bbbb{index:04}"),
        ));
        form.push(p(&format!("SubnetMappings.member.1.{member}"), value));
        form.push(p(
            "SubnetMappings.member.2.SubnetId",
            &format!("subnet-cccc{index:04}"),
        ));
        let reply = call(&router, &form).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the mapping to be accepted, got {} body {}",
                reply.status, reply.body
            );
            // What the caller pinned is read back off the stored record, not echoed from
            // the create's own answer.
            let mut read = action("DescribeLoadBalancers");
            read.push(p("Names.member.1", &name));
            let described = call(&router, &read).await;
            let reported = first_text(&described.body, member);
            // `SourceNatIpv6Prefix` is reported under the zone's own prefix list rather
            // than as an address member, so a pinned address is what is read back here.
            if member != "SourceNatIpv6Prefix" {
                assert_eq!(
                    reported.as_deref(),
                    Some(value),
                    "case {case:?}: expected the pinned {member} to be reported, got {}",
                    described.body
                );
            }
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, INVALID_CONFIGURATION),
                "case {case:?}: expected 400 {INVALID_CONFIGURATION}, got {} body {}",
                reply.status,
                reply.body
            );
            assert!(
                !stored(&router, &name).await,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn the_ip_address_type_a_create_takes_follows_the_family_not_the_scheme() {
    let (router, _engine) = emulator();

    // (case, the type, the scheme, the IP address type, whether it is accepted)
    //
    // `dualstack-without-public-ipv4` is bracketed for Application Load Balancers alone,
    // which is the one rule the family decides — the same one `SetIpAddressType` applies to
    // a load balancer that already exists. The SCHEME decides nothing: an internal load
    // balancer carries IPv6 addresses like any other once its subnets have IPv6 ranges,
    // and which ranges a subnet holds is EC2's to say, so with no EC2 mounted here nothing
    // is known about them and nothing is refused on them (that rule is tabled over what
    // EC2 reports in `all/input_rejection_classes.rs`).
    type AddressTypeCase = (&'static str, &'static str, &'static str, &'static str, bool);
    let rows: [AddressTypeCase; 6] = [
        ("ipv4 on an internal load balancer", "application", "internal", "ipv4", true),
        (
            "dualstack on an internet-facing load balancer",
            "application",
            "internet-facing",
            "dualstack",
            true,
        ),
        (
            "dualstack on an internal load balancer",
            "application",
            "internal",
            "dualstack",
            true,
        ),
        (
            "dualstack on an internal network load balancer",
            "network",
            "internal",
            "dualstack",
            true,
        ),
        (
            "an application-only type on an application load balancer",
            "application",
            "internet-facing",
            "dualstack-without-public-ipv4",
            true,
        ),
        (
            "an application-only type on a network load balancer",
            "network",
            "internet-facing",
            "dualstack-without-public-ipv4",
            false,
        ),
    ];

    for (index, (case, lb_type, scheme, address_type, accepted)) in rows.into_iter().enumerate() {
        let name = format!("address-type-{index}");
        let reply = call(
            &router,
            &create(
                &name,
                &[
                    ("Type", lb_type),
                    ("Scheme", scheme),
                    ("IpAddressType", address_type),
                ],
            ),
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the type to be accepted, got {} body {}",
                reply.status, reply.body
            );
            let mut read = action("DescribeLoadBalancers");
            read.push(p("Names.member.1", &name));
            let described = call(&router, &read).await;
            assert_eq!(
                first_text(&described.body, "IpAddressType").as_deref(),
                Some(address_type),
                "case {case:?}: expected the stored IP address type, got {}",
                described.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, INVALID_CONFIGURATION),
                "case {case:?}: expected 400 {INVALID_CONFIGURATION}, got {} body {}",
                reply.status,
                reply.body
            );
            assert!(
                !stored(&router, &name).await,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn a_repeat_create_is_idempotent_only_with_the_same_settings() {
    let (router, _engine) = emulator();

    let first = call(&router, &create("repeat-lb", &[("Scheme", "internal")])).await;
    assert_eq!(
        first.status, 200,
        "case the first create: expected 200, got {} body {}",
        first.status, first.body
    );
    let arn = first
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case the first create: no LoadBalancerArn in {}", first.body));

    // "This operation is idempotent ... If you attempt to create multiple load balancers
    // with the same settings, each call succeeds": the same request answers the SAME load
    // balancer and mints no second one.
    let again = call(&router, &create("repeat-lb", &[("Scheme", "internal")])).await;
    assert_eq!(
        (again.status, again.text("LoadBalancerArn")),
        (200, Some(arn.clone())),
        "case the same settings: expected the stored load balancer, got {} body {}",
        again.status,
        again.body
    );

    // (case, the settings that differ)
    let differing: [(&str, Vec<(&str, &str)>); 3] = [
        ("a different scheme", vec![]),
        (
            "a different type",
            vec![("Scheme", "internal"), ("Type", "network")],
        ),
        (
            "a different IP address type",
            vec![("Scheme", "internet-facing"), ("IpAddressType", "dualstack")],
        ),
    ];
    for (case, extra) in differing {
        let refused = call(&router, &create("repeat-lb", &extra)).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, DUPLICATE_NAME),
            "case {case:?}: expected 400 {DUPLICATE_NAME}, got {} body {}",
            refused.status,
            refused.body
        );
        let message = unescaped(&refused.text("Message").unwrap_or_default());
        assert_eq!(
            message,
            "A load balancer with the same name 'repeat-lb' exists, but with different settings",
            "case {case:?}: expected the refusal to name the name that is taken, got {message:?}"
        );
    }

    // And the refusals changed nothing: the load balancer is still the one the first call
    // minted.
    let mut read = action("DescribeLoadBalancers");
    read.push(p("Names.member.1", "repeat-lb"));
    let described = call(&router, &read).await;
    assert_eq!(
        (
            members(&described.body, "LoadBalancers").len(),
            described.text("Scheme")
        ),
        (1, Some("internal".to_string())),
        "case after the refusals: expected the first load balancer unchanged, got {}",
        described.body
    );
}

#[tokio::test]
async fn a_load_balancer_reports_the_hosted_zone_published_for_its_region_and_family() {
    let (router, _engine) = emulator();

    // (case, the Region, the type, the hosted zone a client aliases the load balancer
    // through)
    //
    // The ids Elastic Load Balancing publishes per Region and family in its "endpoints and
    // quotas" hosted-zone table — Application Load Balancers share a Region's zone with
    // Classic load balancers, Network Load Balancers have their own, and a Gateway Load
    // Balancer is reached through VPC endpoints rather than a DNS name, so it has none.
    type ZoneCase = (&'static str, &'static str, &'static str, Option<&'static str>);
    let rows: [ZoneCase; 7] = [
        (
            "an application load balancer in us-east-1",
            "us-east-1",
            "application",
            Some("Z35SXDOTRQ7X7K"),
        ),
        (
            "a network load balancer in us-east-1",
            "us-east-1",
            "network",
            Some("Z26RNL4JYFTOTI"),
        ),
        (
            "an application load balancer in eu-west-1",
            "eu-west-1",
            "application",
            Some("Z32O12XQLNTSW2"),
        ),
        (
            "a network load balancer in ap-northeast-1",
            "ap-northeast-1",
            "network",
            Some("Z31USIVHYNEOWT"),
        ),
        (
            "a network load balancer in us-west-2",
            "us-west-2",
            "network",
            Some("Z18D5FSROUN65G"),
        ),
        (
            "a gateway load balancer, which has no hosted zone",
            "us-east-1",
            "gateway",
            None,
        ),
        (
            "a Region the published table does not carry",
            "mock-region-1",
            "application",
            None,
        ),
    ];

    for (index, (case, region, lb_type, expected)) in rows.into_iter().enumerate() {
        let name = format!("zone-{index}");
        let created = call_in_region(
            &router,
            region,
            &create(&name, &[("Type", lb_type)]),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            created.status, created.body
        );
        // Read it back in its own Region: the hosted zone belongs to the stored load
        // balancer, not to the call that reports it.
        let mut read = action("DescribeLoadBalancers");
        read.push(p("Names.member.1", &name));
        let described = call_in_region(&router, region, &read).await;
        let reported = first_text(&described.body, "CanonicalHostedZoneId");
        assert_eq!(
            reported.as_deref(),
            expected,
            "case {case:?}: expected the published hosted zone, got {}",
            described.body
        );
        // The create's own answer carries the same id, so a client that never re-reads
        // sees it too.
        assert_eq!(
            first_text(&created.body, "CanonicalHostedZoneId").as_deref(),
            expected,
            "case {case:?}: the create must answer the hosted zone it stored, got {}",
            created.body
        );
    }
}
