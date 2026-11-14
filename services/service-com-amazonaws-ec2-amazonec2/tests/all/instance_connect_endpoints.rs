// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EC2 Instance Connect Endpoint slice:
//! `CreateInstanceConnectEndpoint` and `DescribeInstanceConnectEndpoints`, driven over the
//! wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `DescribeInstanceConnectEndpoints` REPORTS THE STORE: the set it answers with is the
//!    account's endpoints narrowed by the `InstanceConnectEndpointIds` and `Filters` the
//!    request names and cut into `MaxResults`-sized pages, and a read changes nothing a
//!    later caller sees;
//! 2. `CreateInstanceConnectEndpoint` MINTS AN ENDPOINT FROM THE REQUEST: every member the
//!    request sets round-trips through the describe, and the members the service owns (the
//!    id, the ARN, the account, the DNS names, the create time, and the VPC and zone of the
//!    endpoint's subnet) are derived rather than taken;
//! 3. the endpoint's `IpAddressType` is the one the request names, and when it names none
//!    the one the SUBNET's CIDR families determine
//!    (`CreateInstanceConnectEndpointRequest$IpAddressType`); a type the subnet has no CIDR
//!    block for is refused;
//! 4. an endpoint is `create-in-progress` and the CLOCK, not a caller, moves it to
//!    `create-complete`;
//! 5. one `ClientToken` names one endpoint;
//! 6. `DeleteInstanceConnectEndpoint` STARTS A CLOCK-OWED DELETION of the endpoint the
//!    request names: it answers that endpoint `delete-in-progress`, the describe reports the
//!    same endpoint, the CLOCK moves it to `delete-complete`, and an id the account holds no
//!    endpoint for is refused rather than treated as already gone;
//! 7. `ModifyInstanceConnectEndpoint` REPLACES EXACTLY THE MEMBERS THE REQUEST CARRIES on
//!    the endpoint it names — its `IpAddressType`, its `PreserveClientIp` and its
//!    security-group set — each validated against that endpoint's own subnet and VPC, every
//!    other member and every other endpoint left as it was, and the change read back through
//!    the describe.
//!
//! Each table row is a distinct input along what the rule generalises over — which address
//! family the subnet has, which address type the request asks for, which filter, how many
//! values, which ids, which page size — so no row is the specification.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::ops::types::{SubnetCidrBlockState, SubnetIpv6CidrBlockAssociation};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INSTANCE_CONNECT_ENDPOINT_ID_PREFIX,
    INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_COMPLETE as STATE_CREATE_COMPLETE,
    INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_IN_PROGRESS as STATE_CREATE_IN_PROGRESS,
    INSTANCE_CONNECT_ENDPOINT_STATE_DELETE_COMPLETE as STATE_DELETE_COMPLETE,
    INSTANCE_CONNECT_ENDPOINT_STATE_DELETE_IN_PROGRESS as STATE_DELETE_IN_PROGRESS,
    IP_ADDRESS_TYPE_DUALSTACK, IP_ADDRESS_TYPE_IPV4, IP_ADDRESS_TYPE_IPV6, SecurityGroupSpec,
    SubnetSpec, TRANSITION_DELAY, availability_zones_in_region,
    instance_connect_endpoint_dns_name, instance_connect_endpoint_fips_dns_name,
    instance_connect_endpoint_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{
    call, element, error_code, error_message, filters, indexed, member as envelope_member,
};
// The XML readers every suite in this crate shares, rather than a fourth copy of them.
use super::wire::{child, child_text, document};

/// The Region every call here is signed for; the zone names and ids the assertions use are
/// derived from it, and it is the Region the endpoints' ARNs and DNS names name.
const REGION: &str = "us-east-1";

/// The IPv6 CIDR block the dual-stack and the IPv6-only seeded subnets carry.
const SEEDED_IPV6_CIDR_BLOCK: &str = "2600:1f14:abc:1200::/64";

/// A well-formed endpoint id nothing of the account carries, for the rows that name one.
///
/// Every resource kind but the instance draws its identifiers from the store's ONE id mint, so
/// an id that names nothing is one the mint has not reached: far above what a fresh account's
/// ambient resources and this suite's own fixture spend between them.
const UNISSUED_ENDPOINT_ID: &str = "eice-0000000000000ffff";

/// A subnet id of the same kind (see [`UNISSUED_ENDPOINT_ID`]).
const UNISSUED_SUBNET_ID: &str = "subnet-0000000000000ffff";

/// A security group id of the same kind (see [`UNISSUED_ENDPOINT_ID`]).
const UNISSUED_GROUP_ID: &str = "sg-0000000000000ffff";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Params, &'static str);

/// One row of the address-type table: `(the subnet's families, the requested
/// `IpAddressType` or `None`, the `PreserveClientIp` the request carries or `None`, the
/// value that must be reported or `None` for a refusal)`.
type PreserveRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateInstanceConnectEndpoint", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeInstanceConnectEndpoints", params).await
}

async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteInstanceConnectEndpoint", params).await
}

/// `TagSpecification.1` form fields tagging the endpoint a create mints.
fn tag_params(tags: &[(&str, &str)]) -> Params {
    let mut params = vec![p(
        "TagSpecification.1.ResourceType",
        "instance-connect-endpoint",
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            *key,
        ));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// The network a suite's endpoints are created into: one VPC with a subnet per address
/// family, plus a second VPC whose security group belongs to another network.
struct Fixture {
    router: MultiServiceRouter,
    engine: std::sync::Arc<engine::InMemoryEngine>,
    vpc_id: String,
    /// The subnet with an IPv4 CIDR block only.
    ipv4_subnet: String,
    /// The subnet with both an IPv4 and an IPv6 CIDR block.
    dual_subnet: String,
    /// The subnet with an IPv6 CIDR block only.
    ipv6_subnet: String,
    /// The default security group of [`Fixture::vpc_id`], which a create that names none
    /// associates with the endpoint.
    default_group: String,
    /// Another security group of that same VPC.
    extra_group: String,
    /// A security group of a DIFFERENT VPC.
    foreign_group: String,
}

/// Seed the network the tables are written against, the way the creating operations would
/// write it: the IPv6 CIDR blocks are attached to the stored subnets directly, because
/// `CreateSubnet` answers IPv6 with a 501 in this emulator (it holds no IPv6 subnet of its
/// own), and this suite is about what an endpoint does with the subnet it finds.
fn fixture() -> Fixture {
    let (router, engine) = super::ec2_query::emulator();
    let zones = availability_zones_in_region(REGION);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let vpc_id = guard.insert_vpc(
        "10.0.0.0/16",
        false,
        "default",
        "available",
        REGION,
        Vec::new(),
    );
    let default_group = guard.insert_default_security_group(&vpc_id, REGION);
    let extra_group = guard.insert_security_group(SecurityGroupSpec {
        vpc_id: vpc_id.clone(),
        group_name: "endpoint-access".to_string(),
        description: "the group an endpoint row names explicitly".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: Vec::new(),
        ip_permissions_egress: Vec::new(),
    });
    let foreign_vpc = guard.insert_vpc(
        "10.1.0.0/16",
        false,
        "default",
        "available",
        REGION,
        Vec::new(),
    );
    let foreign_group = guard.insert_default_security_group(&foreign_vpc, REGION);

    let mut subnet_in = |index: usize, cidr_block: &str| {
        guard.insert_subnet(SubnetSpec {
            vpc_id: vpc_id.clone(),
            cidr_block: cidr_block.to_string(),
            availability_zone: zones[index].zone_name.clone(),
            availability_zone_id: zones[index].zone_id.clone(),
            region: REGION.to_string(),
            state: "available".to_string(),
            default_for_az: false,
            map_public_ip_on_launch: false,
            tags: Vec::new(),
        })
    };
    let ipv4_subnet = subnet_in(0, "10.0.0.0/20");
    let dual_subnet = subnet_in(1, "10.0.16.0/20");
    let ipv6_subnet = subnet_in(2, "10.0.32.0/20");

    let association = SubnetIpv6CidrBlockAssociation {
        association_id: Some(guard.next_id("subnet-cidr-assoc")),
        ipv6_cidr_block: Some(SEEDED_IPV6_CIDR_BLOCK.to_string()),
        ipv6_cidr_block_state: Some(SubnetCidrBlockState {
            state: Some("associated".to_string()),
            status_message: None,
        }),
        ipv6_address_attribute: None,
        ip_source: None,
    };
    if let Some(subnet) = guard.subnets.get_mut(&dual_subnet) {
        subnet.ipv6_cidr_block_association_set = Some(vec![association.clone()]);
    }
    if let Some(subnet) = guard.subnets.get_mut(&ipv6_subnet) {
        // "Indicates whether this is an IPv6 only subnet" (`Subnet$Ipv6Native`): no IPv4
        // CIDR block at all.
        subnet.cidr_block = None;
        subnet.ipv6_native = Some(true);
        subnet.ipv6_cidr_block_association_set = Some(vec![association]);
    }
    drop(guard);

    Fixture {
        router,
        engine,
        vpc_id,
        ipv4_subnet,
        dual_subnet,
        ipv6_subnet,
        default_group,
        extra_group,
        foreign_group,
    }
}

impl Fixture {
    /// How many endpoints the store holds, read straight off the state the handlers share.
    fn stored_endpoints(&self) -> usize {
        self.engine
            .state::<Ec2State>()
            .lock()
            .instance_connect_endpoints
            .len()
    }
}

/// One endpoint of a response body, as the ec2Query XML reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointRow {
    id: String,
    arn: String,
    owner_id: String,
    state: String,
    dns_name: String,
    fips_dns_name: String,
    vpc_id: String,
    subnet_id: String,
    availability_zone: String,
    availability_zone_id: String,
    ip_address_type: String,
    preserve_client_ip: String,
    created_at: String,
    security_group_ids: Vec<String>,
    tags: BTreeMap<String, String>,
}

/// The text of `node`'s `name` child, or the empty string when it has none — the shared
/// [`child_text`] with the absent case flattened, so a row compares as plain text.
fn text_of(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child_text(node, name).unwrap_or_default()
}

/// The texts of the items of `node`'s `set` child — an ec2Query list is a `<xSet>` of
/// `<item>`s.
fn set_items(node: roxmltree::Node<'_, '_>, set: &str) -> Vec<String> {
    child(node, set)
        .map(|list| {
            list.children()
                .filter(roxmltree::Node::is_element)
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `tagSet` of `node`, as a key -> value map.
fn tag_map(node: roxmltree::Node<'_, '_>) -> BTreeMap<String, String> {
    child(node, "tagSet")
        .map(|list| {
            list.children()
                .filter(roxmltree::Node::is_element)
                .map(|item| (text_of(item, "key"), text_of(item, "value")))
                .collect()
        })
        .unwrap_or_default()
}

/// Every endpoint a response body reports, in document order. An endpoint is any element
/// carrying an `instanceConnectEndpointId` child, which is both the create response's one
/// endpoint and each item of a describe's set.
fn endpoint_rows(case: &str, body: &str) -> Vec<EndpointRow> {
    let document = document(case, body);
    document
        .descendants()
        .filter(|node| {
            node.is_element() && child(*node, "instanceConnectEndpointId").is_some()
        })
        .map(|node| EndpointRow {
            id: text_of(node, "instanceConnectEndpointId"),
            arn: text_of(node, "instanceConnectEndpointArn"),
            owner_id: text_of(node, "ownerId"),
            state: text_of(node, "state"),
            dns_name: text_of(node, "dnsName"),
            fips_dns_name: text_of(node, "fipsDnsName"),
            vpc_id: text_of(node, "vpcId"),
            subnet_id: text_of(node, "subnetId"),
            availability_zone: text_of(node, "availabilityZone"),
            availability_zone_id: text_of(node, "availabilityZoneId"),
            ip_address_type: text_of(node, "ipAddressType"),
            preserve_client_ip: text_of(node, "preserveClientIp"),
            created_at: text_of(node, "createdAt"),
            security_group_ids: set_items(node, "securityGroupIdSet"),
            tags: tag_map(node),
        })
        .collect()
}

/// The one endpoint a successful create reports.
async fn create_endpoint(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> EndpointRow {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateInstanceConnectEndpoint, \
         got {status} with body {body}"
    );
    let rows = endpoint_rows(case, &body);
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected exactly one endpoint in the create response, got {rows:?}"
    );
    rows.into_iter().next().unwrap_or_else(|| unreachable!())
}

/// The endpoint ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstanceConnectEndpoints, \
         got {status} with body {body}"
    );
    endpoint_rows(case, &body)
        .into_iter()
        .map(|row| row.id)
        .collect()
}

/// An `InstanceConnectEndpointId` is `eice-` followed by 8 or 17 lowercase hex digits — the
/// service's own id rule, imported rather than re-derived so the two cannot drift.
fn is_endpoint_id(candidate: &str) -> bool {
    instance_connect_endpoint_id_is_well_formed(candidate)
}

// ---------------------------------------------------------------------------
// RULE 2: a create mints the endpoint from the request, a describe reports the store
// ---------------------------------------------------------------------------

/// RULE 2 and RULE 1 together: every member the request sets round-trips through
/// `DescribeInstanceConnectEndpoints`, the service-owned members are derived, and the
/// describe reports exactly what the create answered.
#[tokio::test]
async fn a_created_endpoint_round_trips_through_the_describe() {
    let fixture = fixture();
    let case = "an endpoint with two tags and two named security groups";
    let mut params = vec![
        p("SubnetId", fixture.ipv4_subnet.clone()),
        p("PreserveClientIp", "true"),
    ];
    params.extend(indexed(
        "SecurityGroupId",
        &[&fixture.extra_group, &fixture.default_group],
    ));
    params.extend(tag_params(&[("Name", "bastion"), ("Owner", "TeamA")]));
    let created = create_endpoint(&fixture.router, case, &params).await;

    assert!(
        is_endpoint_id(&created.id),
        "case {case:?}: expected a minted endpoint id, got {:?}",
        created.id
    );
    let zones = availability_zones_in_region(REGION);
    let expected = EndpointRow {
        id: created.id.clone(),
        arn: format!(
            "arn:aws:ec2:{REGION}:{account}:instance-connect-endpoint/{id}",
            account = engine::EMULATOR_ACCOUNT_ID,
            id = created.id,
        ),
        owner_id: engine::EMULATOR_ACCOUNT_ID.to_string(),
        state: STATE_CREATE_IN_PROGRESS.to_string(),
        dns_name: created.dns_name.clone(),
        fips_dns_name: created.fips_dns_name.clone(),
        vpc_id: fixture.vpc_id.clone(),
        subnet_id: fixture.ipv4_subnet.clone(),
        availability_zone: zones[0].zone_name.clone(),
        availability_zone_id: zones[0].zone_id.clone(),
        ip_address_type: IP_ADDRESS_TYPE_IPV4.to_string(),
        preserve_client_ip: "true".to_string(),
        created_at: created.created_at.clone(),
        // The ids a request names are reported in the order the service resolved them,
        // which is the request's own order.
        security_group_ids: vec![fixture.extra_group.clone(), fixture.default_group.clone()],
        tags: [
            ("Name".to_string(), "bastion".to_string()),
            ("Owner".to_string(), "TeamA".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    assert_eq!(
        created, expected,
        "case {case:?}: expected the create to answer {expected:?}, got {created:?}"
    );
    assert!(
        !created.created_at.is_empty(),
        "case {case:?}: a created endpoint carries a create time, got {created:?}"
    );

    // The DNS names are DERIVED from the endpoint's id and the Region it was created in,
    // which is what makes them stable across reads and unique per endpoint.
    for (member, value) in [
        ("dnsName", &created.dns_name),
        ("fipsDnsName", &created.fips_dns_name),
    ] {
        assert!(
            value.starts_with(&format!("{}.", created.id)) && value.contains(REGION),
            "case {case:?}: expected {member} to name the endpoint and the Region, got {value:?}"
        );
    }
    assert_ne!(
        created.dns_name, created.fips_dns_name,
        "case {case:?}: the FIPS name is a different name, got {created:?}"
    );

    // RULE 1: the describe reports the store, so it answers exactly what was stored — and
    // reading twice does not change it.
    for read in 1..=2 {
        let (status, body) = describe(
            &fixture.router,
            &indexed("InstanceConnectEndpointId", &[&created.id]),
        )
        .await;
        assert_eq!(
            status, 200,
            "read {read}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            endpoint_rows(case, &body),
            vec![created.clone()],
            "read {read}: the describe must report the endpoint the create stored"
        );
    }

    // A second endpoint gets its own id, ARN and DNS names.
    let other = create_endpoint(
        &fixture.router,
        "a second endpoint in the dual-stack subnet",
        &[p("SubnetId", fixture.dual_subnet.clone())],
    )
    .await;
    assert_ne!(
        other.id, created.id,
        "two endpoints must not share an id, got {other:?} and {created:?}"
    );
    assert_ne!(
        other.dns_name, created.dns_name,
        "two endpoints must not share a DNS name, got {other:?} and {created:?}"
    );
    // The endpoint's network facts come from its SUBNET, not from the request.
    assert_eq!(
        (
            other.vpc_id.as_str(),
            other.availability_zone.as_str(),
            other.availability_zone_id.as_str()
        ),
        (
            fixture.vpc_id.as_str(),
            zones[1].zone_name.as_str(),
            zones[1].zone_id.as_str()
        ),
        "an endpoint reports the VPC and zone of its subnet, got {other:?}"
    );
    // "If you don't specify a security group, the default security group for your VPC will
    // be associated with the endpoint."
    assert_eq!(
        other.security_group_ids,
        vec![fixture.default_group.clone()],
        "an endpoint created without a security group takes its VPC's default, got {other:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the address type the request names, else the subnet's own families
// ---------------------------------------------------------------------------

/// RULE 3 over the subnet's address families crossed with every value of the modelled
/// `IpAddressType` enum and with naming none: the type is the request's when the subnet has
/// a CIDR block for it, the subnet's own when the request names none, and a refusal when
/// the subnet has no CIDR block for the type asked for.
///
/// The catalog of types is the modelled enum
/// (`smithy_query(query="shape",shape="IpAddressType")`), so no value is left untested.
#[tokio::test]
async fn the_address_type_follows_the_request_then_the_subnets_cidr_families() {
    // (the subnet's families, the requested type or None, the type that must be reported)
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("ipv4", None, Some(IP_ADDRESS_TYPE_IPV4)),
        ("ipv4", Some(IP_ADDRESS_TYPE_IPV4), Some(IP_ADDRESS_TYPE_IPV4)),
        ("ipv4", Some(IP_ADDRESS_TYPE_DUALSTACK), None),
        ("ipv4", Some(IP_ADDRESS_TYPE_IPV6), None),
        ("dual", None, Some(IP_ADDRESS_TYPE_DUALSTACK)),
        ("dual", Some(IP_ADDRESS_TYPE_IPV4), Some(IP_ADDRESS_TYPE_IPV4)),
        (
            "dual",
            Some(IP_ADDRESS_TYPE_DUALSTACK),
            Some(IP_ADDRESS_TYPE_DUALSTACK),
        ),
        ("dual", Some(IP_ADDRESS_TYPE_IPV6), Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", None, Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", Some(IP_ADDRESS_TYPE_IPV6), Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", Some(IP_ADDRESS_TYPE_IPV4), None),
        ("ipv6", Some(IP_ADDRESS_TYPE_DUALSTACK), None),
    ];
    for (families, requested, expected) in rows {
        let fixture = fixture();
        let subnet_id = match families {
            "ipv4" => fixture.ipv4_subnet.clone(),
            "dual" => fixture.dual_subnet.clone(),
            _ => fixture.ipv6_subnet.clone(),
        };
        let case = format!("a {families} subnet asked for {requested:?}");
        let mut params = vec![p("SubnetId", subnet_id)];
        if let Some(requested) = requested {
            params.push(p("IpAddressType", requested));
        }
        match expected {
            Some(address_type) => {
                let created = create_endpoint(&fixture.router, &case, &params).await;
                assert_eq!(
                    created.ip_address_type, address_type,
                    "case {case:?}: expected ipAddressType={address_type:?}, got {created:?}"
                );
                // The stored endpoint reports the same type a describe later reads.
                let ids = described_ids(&fixture.router, &case, &[]).await;
                assert_eq!(
                    ids,
                    vec![created.id.clone()],
                    "case {case:?}: expected the describe to report the one endpoint"
                );
            }
            None => {
                let (status, body) = create(&fixture.router, &params).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterCombination",
                    "case {case:?}: expected InvalidParameterCombination, got {body}"
                );
                assert_eq!(
                    fixture.stored_endpoints(),
                    0,
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

/// RULE 3's note: "`PreserveClientIp` is only supported on IPv4 EC2 Instance Connect
/// Endpoints. To use `PreserveClientIp`, the value for `IpAddressType` must be `ipv4`"
/// (`CreateInstanceConnectEndpointRequest$PreserveClientIp`), whose "Default: false" is not
/// using it.
#[tokio::test]
async fn preserve_client_ip_is_refused_outside_an_ipv4_endpoint() {
    // (the subnet's families, the requested type or None, PreserveClientIp or None,
    //  the value that must be reported, or None for a refusal)
    let rows: Vec<PreserveRow> = vec![
        ("ipv4", None, None, Some("false")),
        ("ipv4", None, Some("false"), Some("false")),
        ("ipv4", None, Some("true"), Some("true")),
        ("dual", Some(IP_ADDRESS_TYPE_IPV4), Some("true"), Some("true")),
        ("dual", None, Some("true"), None),
        ("dual", Some(IP_ADDRESS_TYPE_IPV6), Some("true"), None),
        ("ipv6", None, Some("true"), None),
        ("ipv6", None, Some("false"), Some("false")),
        ("ipv6", None, None, Some("false")),
    ];
    for (families, address_type, preserve, expected) in rows {
        let fixture = fixture();
        let subnet_id = match families {
            "ipv4" => fixture.ipv4_subnet.clone(),
            "dual" => fixture.dual_subnet.clone(),
            _ => fixture.ipv6_subnet.clone(),
        };
        let case =
            format!("a {families} subnet, ipAddressType={address_type:?}, preserve={preserve:?}");
        let mut params = vec![p("SubnetId", subnet_id)];
        if let Some(address_type) = address_type {
            params.push(p("IpAddressType", address_type));
        }
        if let Some(preserve) = preserve {
            params.push(p("PreserveClientIp", preserve));
        }
        match expected {
            Some(value) => {
                let created = create_endpoint(&fixture.router, &case, &params).await;
                assert_eq!(
                    created.preserve_client_ip, value,
                    "case {case:?}: expected preserveClientIp={value:?}, got {created:?}"
                );
            }
            None => {
                let (status, body) = create(&fixture.router, &params).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterCombination",
                    "case {case:?}: expected InvalidParameterCombination, got {body}"
                );
                assert_eq!(
                    fixture.stored_endpoints(),
                    0,
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: the clock, not a caller, completes the creation
// ---------------------------------------------------------------------------

/// RULE 4: a created endpoint is `create-in-progress`, every read inside the span still
/// says so, and the clock — nothing the caller does — moves it to `create-complete`. The
/// `state` filter agrees with what each endpoint reports at the same instant.
#[tokio::test]
async fn a_created_endpoint_is_create_in_progress_until_the_clock_completes_it() {
    let fixture = fixture();
    let mut ids = Vec::new();
    // Three endpoints in three different subnets, so the transition is not a property of
    // one row. Each endpoint's own span is read right after its own create — never after
    // the whole loop — so what these assertions observe is the span the service keeps and
    // not how long the rest of the suite took on the machine running it.
    for subnet_id in [
        &fixture.ipv4_subnet,
        &fixture.dual_subnet,
        &fixture.ipv6_subnet,
    ] {
        let case = format!("an endpoint in {subnet_id}");
        let created = create_endpoint(&fixture.router, &case, &[p("SubnetId", subnet_id.clone())]).await;
        assert_eq!(
            created.state, STATE_CREATE_IN_PROGRESS,
            "case {case:?}: expected a fresh endpoint to be {STATE_CREATE_IN_PROGRESS}, \
             got {created:?}"
        );

        // The span is never zero: a caller reading straight after its own create still
        // sees the transient state, and the `state` filter agrees with what the endpoint
        // reports at that instant.
        let in_progress = described_ids(
            &fixture.router,
            &format!("{case}, state=create-in-progress inside its span"),
            &filters(&[("state", &[STATE_CREATE_IN_PROGRESS])]),
        )
        .await;
        assert!(
            in_progress.contains(&created.id),
            "case {case:?}: expected {} among the endpoints the \
             state={STATE_CREATE_IN_PROGRESS} filter reports inside its span, got \
             {in_progress:?}",
            created.id
        );
        let complete = described_ids(
            &fixture.router,
            &format!("{case}, state=create-complete inside its span"),
            &filters(&[("state", &[STATE_CREATE_COMPLETE])]),
        )
        .await;
        assert!(
            !complete.contains(&created.id),
            "case {case:?}: expected {} not to be {STATE_CREATE_COMPLETE} inside its \
             span, got {complete:?}",
            created.id
        );

        ids.push(created.id);
    }
    ids.sort();

    past_the_transition().await;

    for id in &ids {
        // Twice: the materialised state is stable, not consumed by the read.
        for read in 1..=2 {
            let (status, body) =
                describe(&fixture.router, &indexed("InstanceConnectEndpointId", &[id])).await;
            assert_eq!(
                status, 200,
                "read {read}: expected HTTP 200, got {status} with body {body}"
            );
            let rows = endpoint_rows("one endpoint after the span", &body);
            assert_eq!(
                rows.first().map(|row| row.state.as_str()),
                Some(STATE_CREATE_COMPLETE),
                "read {read}: expected {id} to be {STATE_CREATE_COMPLETE} once the \
                 transition is due, got {body}"
            );
        }
    }
    assert_eq!(
        described_ids(
            &fixture.router,
            "state=create-complete after the span",
            &filters(&[("state", &[STATE_CREATE_COMPLETE])])
        )
        .await,
        ids,
        "case \"state=create-complete after the span\": expected every endpoint"
    );
    assert!(
        described_ids(
            &fixture.router,
            "state=create-in-progress after the span",
            &filters(&[("state", &[STATE_CREATE_IN_PROGRESS])])
        )
        .await
        .is_empty(),
        "case \"state=create-in-progress after the span\": no endpoint is in progress any more"
    );
}

// ---------------------------------------------------------------------------
// RULE 1: narrowing by ids and by every documented filter
// ---------------------------------------------------------------------------

/// Three endpoints spread over the fixture's subnets and tags, for the filter and paging
/// tables to name rows by: the ids in the order the describe reports them.
async fn seeded_endpoints(fixture: &Fixture) -> Vec<String> {
    let mut ids = Vec::new();
    for (subnet_id, tags) in [
        (&fixture.ipv4_subnet, vec![("Name", "web"), ("Owner", "TeamA")]),
        (&fixture.dual_subnet, vec![("Name", "batch"), ("Owner", "TeamB")]),
        (&fixture.ipv6_subnet, vec![("Project", "atlas")]),
    ] {
        let case = format!("a seeded endpoint in {subnet_id}");
        let mut params = vec![p("SubnetId", subnet_id.clone())];
        params.extend(tag_params(&tags));
        ids.push(create_endpoint(&fixture.router, &case, &params).await.id);
    }
    ids.sort();
    ids
}

/// RULE 1 over every filter name `DescribeInstanceConnectEndpointsRequest$Filters`
/// documents, plus the AND of two filters, the OR of two values and EC2's `*` wildcard.
#[tokio::test]
async fn every_documented_filter_selects_the_endpoints_it_names() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;
    let all = ids.clone();

    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no filter at all", Vec::new(), all.clone()),
        (
            "instance-connect-endpoint-id, one value",
            filters(&[("instance-connect-endpoint-id", &[ids[1].as_str()])]),
            vec![ids[1].clone()],
        ),
        (
            "instance-connect-endpoint-id, two values joined with OR",
            filters(&[(
                "instance-connect-endpoint-id",
                &[ids[0].as_str(), ids[2].as_str()],
            )]),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "instance-connect-endpoint-id with EC2's wildcard",
            filters(&[(
                "instance-connect-endpoint-id",
                &[&format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}*")],
            )]),
            all.clone(),
        ),
        (
            "subnet-id",
            filters(&[("subnet-id", &[fixture.dual_subnet.as_str()])]),
            vec![ids[1].clone()],
        ),
        (
            "vpc-id, the VPC every endpoint is in",
            filters(&[("vpc-id", &[fixture.vpc_id.as_str()])]),
            all.clone(),
        ),
        (
            "vpc-id of another VPC",
            filters(&[("vpc-id", &["vpc-00000000000000000"])]),
            Vec::new(),
        ),
        (
            "tag:Owner",
            filters(&[("tag:Owner", &["TeamB"])]),
            vec![ids[1].clone()],
        ),
        (
            "tag-key",
            filters(&[("tag-key", &["Owner"])]),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "tag-value",
            filters(&[("tag-value", &["atlas"])]),
            vec![ids[2].clone()],
        ),
        (
            "two filters joined with AND",
            filters(&[
                ("tag-key", &["Owner"]),
                ("subnet-id", &[fixture.ipv4_subnet.as_str()]),
            ]),
            vec![ids[0].clone()],
        ),
        (
            "two filters joined with AND that no endpoint satisfies",
            filters(&[
                ("tag-value", &["atlas"]),
                ("subnet-id", &[fixture.ipv4_subnet.as_str()]),
            ]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let mut selected = described_ids(&fixture.router, case, &params).await;
        selected.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected {expected:?}, got {selected:?}"
        );
    }

    // The `state` filter selects on a value the CLOCK owns, so it is read against an
    // endpoint created immediately before the read rather than as a row of the table
    // above: what it must show is that the filter selects on the state the endpoint
    // reports, not how long the rows before it took.
    let case = "state, the one a freshly created endpoint is in";
    let fresh = create_endpoint(
        &fixture.router,
        case,
        &[p("SubnetId", fixture.ipv4_subnet.clone())],
    )
    .await;
    let selected = described_ids(
        &fixture.router,
        case,
        &filters(&[("state", &[STATE_CREATE_IN_PROGRESS])]),
    )
    .await;
    assert!(
        selected.contains(&fresh.id),
        "case {case:?}: expected {} among the endpoints the \
         state={STATE_CREATE_IN_PROGRESS} filter reports, got {selected:?}",
        fresh.id
    );
}

/// RULE 1: a filter name the operation does not document is refused rather than read as "no
/// results", and the ids it reports are narrowed only by ids it can resolve.
#[tokio::test]
async fn an_unresolvable_filter_or_id_is_refused() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    let rows: Vec<RefusalRow> = vec![
        (
            "a filter name the operation does not document",
            filters(&[("endpoint-id", &[ids[0].as_str()])]),
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["TeamA"])]),
            "InvalidParameterValue",
        ),
        (
            "a filter name that differs only in case",
            filters(&[("Subnet-Id", &[fixture.ipv4_subnet.as_str()])]),
            "InvalidParameterValue",
        ),
        (
            "an id that is not shaped like an endpoint id",
            indexed("InstanceConnectEndpointId", &["not-an-endpoint"]),
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "an id with a non-hex suffix",
            indexed("InstanceConnectEndpointId", &["eice-zzzzzzzz"]),
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "a well-formed id no endpoint of the account has",
            indexed("InstanceConnectEndpointId", &[UNISSUED_ENDPOINT_ID]),
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a known id beside an unknown one",
            indexed(
                "InstanceConnectEndpointId",
                &[ids[0].as_str(), UNISSUED_ENDPOINT_ID],
            ),
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a page token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
    ];
    for (case, params, code) in rows {
        let (status, body) = describe(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }

    // A create cannot name a subnet or a security group the account does not have either.
    let create_rows: Vec<RefusalRow> = vec![
        (
            "a subnet no subnet of the account has",
            vec![p("SubnetId", UNISSUED_SUBNET_ID)],
            "InvalidSubnetID.NotFound",
        ),
        (
            "a security group no group of the account has",
            {
                let mut params = vec![p("SubnetId", fixture.ipv4_subnet.clone())];
                params.extend(indexed("SecurityGroupId", &[UNISSUED_GROUP_ID]));
                params
            },
            "InvalidGroup.NotFound",
        ),
        (
            "a security group of another VPC",
            {
                let mut params = vec![p("SubnetId", fixture.ipv4_subnet.clone())];
                params.extend(indexed("SecurityGroupId", &[&fixture.foreign_group]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming another resource type",
            vec![
                p("SubnetId", fixture.ipv4_subnet.clone()),
                p("TagSpecification.1.ResourceType", "subnet"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "bastion"),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (case, params, code) in create_rows {
        let (status, body) = create(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            fixture.stored_endpoints(),
            ids.len(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// RULE 1: the answer is cut into `MaxResults`-sized pages, and walking the `NextToken`s
/// visits every endpoint exactly once, in id order, whatever the page size.
#[tokio::test]
async fn paging_walks_every_endpoint_once_at_each_page_size() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    // Each page size from one up to one past the number of endpoints, plus the published
    // bounds of the modelled `MaxResults` range (1..50).
    for page_size in [1_usize, 2, 3, 4, 50] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&fixture.router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page: Vec<String> = endpoint_rows(&case, &body)
                .into_iter()
                .map(|row| row.id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} endpoints, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case:?}: paging must terminate, got {seen:?}"
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: paging must visit every endpoint once, in id order"
        );
    }

    // One past each published bound of the modelled `@range`: refused at the boundary,
    // before the handler runs.
    for max_results in ["0", "51"] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = describe(&fixture.router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a MaxResults outside the modelled range, \
             got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: one ClientToken names one endpoint
// ---------------------------------------------------------------------------

/// RULE 5: "Unique, case-sensitive identifier that you provide to ensure the idempotency of
/// the request" — a replay of the same token answers the endpoint that token minted and
/// creates nothing; another token mints another endpoint.
#[tokio::test]
async fn one_client_token_names_one_endpoint() {
    let fixture = fixture();
    let rows = [
        ("a lowercase token", "idem-a"),
        ("the same text in another case", "IDEM-A"),
        ("a token of a different shape", "7f3c-0001"),
    ];
    let mut minted: Vec<String> = Vec::new();
    for (case, token) in rows {
        let params = vec![
            p("SubnetId", fixture.ipv4_subnet.clone()),
            p("ClientToken", token),
        ];
        let first = create_endpoint(&fixture.router, case, &params).await;
        assert_eq!(
            element(
                &create(&fixture.router, &params).await.1,
                "clientToken"
            )
            .as_deref(),
            Some(token),
            "case {case:?}: the response echoes the token the request provided"
        );
        let replayed = create_endpoint(&fixture.router, case, &params).await;
        assert_eq!(
            replayed.id, first.id,
            "case {case:?}: a replay of one token answers the endpoint it minted"
        );
        assert!(
            !minted.contains(&first.id),
            "case {case:?}: a different token must mint a different endpoint, got {:?}",
            first.id
        );
        minted.push(first.id);
    }
    assert_eq!(
        fixture.stored_endpoints(),
        rows.len(),
        "one endpoint per distinct token, not one per request"
    );
    let described = described_ids(&fixture.router, "every endpoint", &[]).await;
    let mut expected = minted;
    expected.sort();
    assert_eq!(
        described, expected,
        "the describe reports exactly the endpoints the tokens minted"
    );
}

/// RULE 5's other half: a token names ONE request, so reusing it with any different
/// idempotency-relevant parameter is refused `IdempotentParameterMismatch` rather than
/// answered with the endpoint the first request minted — and nothing is stored for it.
///
/// The rows differ along each member a create resolves: the subnet, the address type, the
/// client-IP flag, the security groups and the tags.
#[tokio::test]
async fn reusing_a_client_token_with_different_parameters_is_refused() {
    let fixture = fixture();
    let token = "idem-mismatch";
    /// The request the token names, plus whatever a row changes about it.
    fn request(fixture: &Fixture, token: &str, extra: Params) -> Params {
        let mut params = vec![
            p("SubnetId", fixture.dual_subnet.clone()),
            p("ClientToken", token),
            p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
            p("PreserveClientIp", "true"),
        ];
        params.extend(tag_params(&[("Name", "bastion")]));
        params.extend(extra);
        params
    }
    let original = request(&fixture, token, Vec::new());
    let first = create_endpoint(&fixture.router, "the request the token names", &original).await;

    // Each row is the same request with ONE member changed — except the address-type row,
    // where the type the request asks for is also what forbids `PreserveClientIp`.
    let rows: Vec<(&str, Params)> = vec![
        (
            "another subnet",
            {
                let mut params = request(&fixture, token, Vec::new());
                params[0] = p("SubnetId", fixture.ipv4_subnet.clone());
                params
            },
        ),
        (
            "another address type, and so no preserved client IP",
            {
                let mut params = vec![
                    p("SubnetId", fixture.dual_subnet.clone()),
                    p("ClientToken", token),
                    p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK),
                ];
                params.extend(tag_params(&[("Name", "bastion")]));
                params
            },
        ),
        (
            "another client-IP flag",
            {
                let mut params = request(&fixture, token, Vec::new());
                params[3] = p("PreserveClientIp", "false");
                params
            },
        ),
        (
            "another security group",
            request(
                &fixture,
                token,
                indexed("SecurityGroupId", &[&fixture.extra_group]),
            ),
        ),
        (
            "another tag value",
            {
                let mut params = vec![
                    p("SubnetId", fixture.dual_subnet.clone()),
                    p("ClientToken", token),
                    p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                    p("PreserveClientIp", "true"),
                ];
                params.extend(tag_params(&[("Name", "jump")]));
                params
            },
        ),
        (
            "no tags at all",
            vec![
                p("SubnetId", fixture.dual_subnet.clone()),
                p("ClientToken", token),
                p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                p("PreserveClientIp", "true"),
            ],
        ),
    ];
    for (case, params) in rows {
        let (status, body) = create(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        assert_eq!(
            fixture.stored_endpoints(),
            1,
            "case {case:?}: a refused replay must store nothing"
        );
    }

    // The request the token DOES name still replays, and answers the endpoint it minted.
    let replayed = create_endpoint(&fixture.router, "the same request again", &original).await;
    assert_eq!(
        replayed.id, first.id,
        "the request a token names still answers the endpoint it minted"
    );

    // A DryRun of a MISMATCHED replay is refused with the reason it would really fail on,
    // not with DryRunOperation; a dry run of the request the token names reports that it
    // would have succeeded.
    let mut mismatched = request(&fixture, token, vec![p("DryRun", "true")]);
    mismatched[0] = p("SubnetId", fixture.ipv4_subnet.clone());
    for (case, params, status, code) in [
        (
            "a dry run that reuses the token with another subnet",
            mismatched,
            400,
            "IdempotentParameterMismatch",
        ),
        (
            "a dry run of the request the token names",
            request(&fixture, token, vec![p("DryRun", "true")]),
            412,
            "DryRunOperation",
        ),
    ] {
        let (actual, body) = create(&fixture.router, &params).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        1,
        "no dry run stores an endpoint"
    );
}

/// `DryRun` asks whether the request WOULD have succeeded: HTTP 412 `DryRunOperation`, and
/// on the create nothing is stored.
#[tokio::test]
async fn dry_run_answers_without_touching_the_store() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    for (case, action, params) in [
        (
            "a create that would have succeeded",
            "CreateInstanceConnectEndpoint",
            vec![
                p("SubnetId", fixture.ipv4_subnet.clone()),
                p("DryRun", "true"),
            ],
        ),
        (
            "a describe that would have succeeded",
            "DescribeInstanceConnectEndpoints",
            vec![p("DryRun", "true")],
        ),
    ] {
        let (status, body) = call(&fixture.router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        ids.len(),
        "a dry run stores nothing"
    );
    assert_eq!(
        described_ids(&fixture.router, "every endpoint after the dry runs", &[]).await,
        ids,
        "a dry run leaves the store exactly as it was"
    );
}

/// `SecurityGroupIds` names a SET of associations, so a replay of one token that names the
/// same groups in another order is the SAME request: it answers the endpoint the token
/// minted and stores nothing new. The rows differ in how the same two groups are ordered.
#[tokio::test]
async fn a_replay_may_name_the_same_security_groups_in_any_order() {
    let fixture = fixture();
    let token = "idem-group-order";
    let request = |ids: &[&str]| -> Params {
        let mut params = vec![
            p("SubnetId", fixture.dual_subnet.clone()),
            p("ClientToken", token),
        ];
        params.extend(indexed("SecurityGroupId", ids));
        params
    };

    let first = create_endpoint(
        &fixture.router,
        "the request the token names",
        &request(&[&fixture.default_group, &fixture.extra_group]),
    )
    .await;

    for (case, ids) in [
        (
            "the same order",
            [fixture.default_group.clone(), fixture.extra_group.clone()],
        ),
        (
            "the reverse order",
            [fixture.extra_group.clone(), fixture.default_group.clone()],
        ),
    ] {
        let replayed = create_endpoint(
            &fixture.router,
            case,
            &request(&[ids[0].as_str(), ids[1].as_str()]),
        )
        .await;
        assert_eq!(
            replayed, first,
            "case {case:?}: a replay naming the same groups must answer the endpoint the \
             token minted"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        1,
        "a replay stores nothing, whichever order it names the groups in"
    );
}

/// One row of the DNS-suffix table: a Region and the DNS suffix its partition publishes.
///
/// The suffixes are the `dnsSuffix` each partition publishes in the AWS SDK partition
/// metadata (`endpoints.json`), the same source the crate's `NON_COMMERCIAL_PARTITIONS`
/// table names: `aws` and `aws-us-gov` `amazonaws.com`, `aws-cn` `amazonaws.com.cn`,
/// `aws-iso` `c2s.ic.gov`, `aws-iso-b` `sc2s.sgov.gov`, `aws-iso-e` `cloud.adc-e.uk`,
/// `aws-iso-f` `csp.hci.ic.gov`. Every partition is covered, not only the Region the rest
/// of this suite calls in.
const DNS_SUFFIX_TABLE: [(&str, &str); 7] = [
    ("eu-central-1", "amazonaws.com"),
    ("cn-northwest-1", "amazonaws.com.cn"),
    ("us-gov-east-1", "amazonaws.com"),
    ("us-iso-west-1", "c2s.ic.gov"),
    ("us-isob-east-1", "sc2s.sgov.gov"),
    ("us-isof-south-1", "csp.hci.ic.gov"),
    ("eu-isoe-west-1", "cloud.adc-e.uk"),
];

/// An endpoint's `DnsName` and `FipsDnsName` name the endpoint, the Region it was created
/// in, and the DNS suffix of THAT REGION'S PARTITION — so an endpoint outside the
/// commercial partition does not report a commercial name. This is the rule the created
/// endpoint's names in [`a_created_endpoint_round_trips_through_the_describe`] are one row
/// of; the table below spans every partition the crate knows.
#[test]
fn the_endpoint_dns_names_carry_the_regions_partition_suffix() {
    let endpoint_id = format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}0123456789abcdef0");
    assert!(
        instance_connect_endpoint_id_is_well_formed(&endpoint_id),
        "the table's endpoint id must be a well-formed one, got {endpoint_id:?}"
    );

    for (region, suffix) in DNS_SUFFIX_TABLE {
        let plain = instance_connect_endpoint_dns_name(&endpoint_id, region);
        let fips = instance_connect_endpoint_fips_dns_name(&endpoint_id, region);
        for (member, name) in [("DnsName", &plain), ("FipsDnsName", &fips)] {
            assert!(
                name.starts_with(&format!("{endpoint_id}.")),
                "case {region:?}/{member}: expected the name to start with the endpoint id \
                 {endpoint_id:?}, got {name:?}"
            );
            assert!(
                name.ends_with(&format!(".{region}.{suffix}")),
                "case {region:?}/{member}: expected the name to end in the Region and its \
                 partition's DNS suffix {suffix:?}, got {name:?}"
            );
        }
        assert_ne!(
            plain, fips,
            "case {region:?}: the FIPS name is a different name from the plain one"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 6: a delete starts a clock-owed deletion of the endpoint it names
// ---------------------------------------------------------------------------

/// The one endpoint a describe of `endpoint_id` reports.
async fn described_row(router: &MultiServiceRouter, case: &str, endpoint_id: &str) -> EndpointRow {
    let (status, body) = describe(router, &indexed("InstanceConnectEndpointId", &[endpoint_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstanceConnectEndpoints, \
         got {status} with body {body}"
    );
    let rows = endpoint_rows(case, &body);
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected exactly one endpoint in the describe response, got {rows:?}"
    );
    rows.into_iter().next().unwrap_or_else(|| unreachable!())
}

/// The one endpoint a successful delete reports.
async fn delete_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> EndpointRow {
    let (status, body) = delete(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteInstanceConnectEndpoint, \
         got {status} with body {body}"
    );
    let rows = endpoint_rows(case, &body);
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected exactly one endpoint in the delete response, got {rows:?}"
    );
    rows.into_iter().next().unwrap_or_else(|| unreachable!())
}

/// RULE 6: the delete reports the endpoint it names as the store then holds it —
/// `delete-in-progress`, with every other member of the endpoint unchanged — the describe
/// reports that same endpoint, a repeat of the call is REFUSED because the endpoint is
/// already on its way out, and the CLOCK alone moves the endpoint on to `delete-complete`.
///
/// The rows differ along what the rule generalises over: which address family the endpoint's
/// subnet has, whether the request that created it carried tags or a client token, and
/// whether the endpoint had finished being created when the delete arrived.
#[tokio::test]
async fn deleting_an_endpoint_starts_a_clock_owed_deletion() {
    let fixture = fixture();
    // An endpoint the table never deletes, so each row can also show that a delete touches
    // only the endpoint it names.
    let bystander = create_endpoint(
        &fixture.router,
        "the endpoint no row deletes",
        &[p("SubnetId", fixture.ipv4_subnet.clone())],
    )
    .await;

    /// `(case, the create's parameters, whether to wait out the create span first)`.
    type DeleteRow = (&'static str, Params, bool);
    let rows: Vec<DeleteRow> = vec![
        (
            "an IPv4 endpoint deleted inside its create span",
            vec![p("SubnetId", fixture.ipv4_subnet.clone())],
            false,
        ),
        (
            "a tagged dual-stack endpoint deleted once it is create-complete",
            {
                let mut params = vec![p("SubnetId", fixture.dual_subnet.clone())];
                params.extend(tag_params(&[("Name", "bastion"), ("Owner", "TeamA")]));
                params
            },
            true,
        ),
        (
            "an IPv6-only endpoint created with a client token",
            vec![
                p("SubnetId", fixture.ipv6_subnet.clone()),
                p("ClientToken", "delete-row-ipv6"),
            ],
            false,
        ),
        (
            "an endpoint created with an explicit security group",
            {
                let mut params = vec![p("SubnetId", fixture.dual_subnet.clone())];
                params.extend(indexed("SecurityGroupId", &[&fixture.extra_group]));
                params
            },
            true,
        ),
    ];

    for (case, create_params, wait_first) in rows {
        let created = create_endpoint(&fixture.router, case, &create_params).await;
        if wait_first {
            past_the_transition().await;
        }
        // What the store holds the instant before the delete, so the comparison below is
        // against the endpoint as it really stood and not against the create's own answer.
        let before = described_row(&fixture.router, case, &created.id).await;
        let expected_before_state = if wait_first {
            STATE_CREATE_COMPLETE
        } else {
            STATE_CREATE_IN_PROGRESS
        };
        assert_eq!(
            before.state, expected_before_state,
            "case {case:?}: expected the endpoint to be {expected_before_state} before the \
             delete, got {before:?}"
        );

        let params = vec![p("InstanceConnectEndpointId", created.id.clone())];
        let deleted = delete_endpoint(&fixture.router, case, &params).await;
        let expected = EndpointRow {
            state: STATE_DELETE_IN_PROGRESS.to_string(),
            ..before.clone()
        };
        assert_eq!(
            deleted, expected,
            "case {case:?}: expected the delete to answer {expected:?}, got {deleted:?}"
        );

        // The endpoint the delete reported is the endpoint the describe reports, and the
        // span is never zero, so a caller reading straight after its own delete still sees
        // the transient state — which the `state` filter agrees with.
        assert_eq!(
            described_row(&fixture.router, case, &created.id).await,
            expected,
            "case {case:?}: the describe must report the endpoint the delete stored"
        );
        let in_progress = described_ids(
            &fixture.router,
            case,
            &filters(&[("state", &[STATE_DELETE_IN_PROGRESS])]),
        )
        .await;
        assert!(
            in_progress.contains(&created.id),
            "case {case:?}: expected {} among the endpoints the \
             state={STATE_DELETE_IN_PROGRESS} filter reports inside its span, got \
             {in_progress:?}",
            created.id
        );

        // A repeat of the call is REFUSED: the deletion this call asks for is already under
        // way, so the endpoint is in a state no delete runs from — and the endpoint it names
        // is left exactly where the first delete put it.
        let (status, body) = delete(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a repeated delete, got {status} with \
             body {body}"
        );
        assert_eq!(
            super::ec2_query::error_code(&body),
            ec2::state::INCORRECT_STATE,
            "case {case:?}: expected the wrong-state refusal on the repeat, got body {body}"
        );
        assert_eq!(
            described_row(&fixture.router, case, &created.id).await,
            expected,
            "case {case:?}: a refused repeat must leave the deletion the first one started"
        );

        // Only the endpoint the request named moved: the bystander is still in a CREATE
        // state, which is the only kind of state its own clock can give it.
        let other = described_row(&fixture.router, case, &bystander.id).await;
        assert!(
            other.state == STATE_CREATE_IN_PROGRESS || other.state == STATE_CREATE_COMPLETE,
            "case {case:?}: a delete must not move an endpoint it did not name, got {other:?}"
        );

        past_the_transition().await;
        // Twice: the materialised state is stable, not consumed by the read.
        for read in 1..=2 {
            let row = described_row(&fixture.router, case, &created.id).await;
            assert_eq!(
                row.state, STATE_DELETE_COMPLETE,
                "case {case:?} read {read}: expected {} to be {STATE_DELETE_COMPLETE} once \
                 the deletion is due, got {row:?}",
                created.id
            );
        }
        let complete = described_ids(
            &fixture.router,
            case,
            &filters(&[("state", &[STATE_DELETE_COMPLETE])]),
        )
        .await;
        assert!(
            complete.contains(&created.id),
            "case {case:?}: expected {} among the endpoints the \
             state={STATE_DELETE_COMPLETE} filter reports, got {complete:?}",
            created.id
        );
        // The bystander has reached `create-complete` by the clock alone, never a delete
        // state.
        assert_eq!(
            described_row(&fixture.router, case, &bystander.id)
                .await
                .state,
            STATE_CREATE_COMPLETE,
            "case {case:?}: an endpoint no delete named is only ever moved by its own clock"
        );
    }
}

/// RULE 6: an id a delete cannot resolve is REFUSED — malformed before unknown, in the
/// wording EC2 uses for the class — and nothing in the store moves.
///
/// The rows differ along why the id does not resolve: the shape of the text, the length of
/// the suffix, the resource the prefix names, and a well-formed id of each id era the
/// account holds no endpoint for.
#[tokio::test]
async fn a_delete_that_cannot_resolve_its_endpoint_is_refused() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    let rows: Vec<RefusalRow> = vec![
        (
            "an id that is not shaped like an endpoint id",
            vec![p("InstanceConnectEndpointId", "not-an-endpoint")],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "an id with a non-hex suffix",
            vec![p("InstanceConnectEndpointId", "eice-zzzzzzzz")],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "an id whose suffix is of neither published length",
            vec![p("InstanceConnectEndpointId", "eice-0123456789")],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "a well-formed id of another resource",
            vec![p("InstanceConnectEndpointId", "subnet-0123456789abcdef0")],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "a long id no endpoint of the account has",
            vec![p("InstanceConnectEndpointId", UNISSUED_ENDPOINT_ID)],
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "an all-zero long id",
            vec![p("InstanceConnectEndpointId", "eice-00000000000000000")],
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a short id no endpoint of the account has",
            vec![p("InstanceConnectEndpointId", "eice-0a1b2c3d")],
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a dry run of a delete naming an id no endpoint has",
            vec![
                p("InstanceConnectEndpointId", UNISSUED_ENDPOINT_ID),
                p("DryRun", "true"),
            ],
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
    ];
    for (case, params, code) in rows {
        let (status, body) = delete(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        // An id the account holds no endpoint for is reported as the id the REQUEST named,
        // with what the caller has to do about it.
        if code.ends_with(".NotFound") {
            let named = params
                .iter()
                .find(|(key, _)| key == "InstanceConnectEndpointId")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let expected = format!(
                "The Instance Connect Endpoint {named} does not exist. Add an existing \
                 Instance Connect Endpoint ID and retry your request."
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the message {expected:?}, got {actual:?}"
            );
        }
    }

    // Nothing a refused delete named moved, and nothing was removed.
    assert_eq!(
        fixture.stored_endpoints(),
        ids.len(),
        "a refused delete must leave the store as it was"
    );
    assert_eq!(
        described_ids(&fixture.router, "every endpoint after the refusals", &[]).await,
        ids,
        "a refused delete leaves the store exactly as it was"
    );
    for id in &ids {
        let row = described_row(&fixture.router, "a seeded endpoint", id).await;
        assert!(
            row.state == STATE_CREATE_IN_PROGRESS || row.state == STATE_CREATE_COMPLETE,
            "expected {id} to be in a create state still, got {row:?}"
        );
    }
}

/// RULE 6 under `DryRun`: the flag asks whether the delete WOULD have succeeded — HTTP 412
/// `DryRunOperation` — and the endpoint it named keeps the state it had, so no deletion was
/// started.
#[tokio::test]
async fn a_dry_run_delete_starts_no_deletion() {
    let fixture = fixture();
    let created = create_endpoint(
        &fixture.router,
        "the endpoint the dry run names",
        &[p("SubnetId", fixture.dual_subnet.clone())],
    )
    .await;

    // Once inside the create span and once after it: a dry run reports the same thing in
    // either state, and starts nothing in either.
    for (case, wait_first, expected_state) in [
        (
            "a dry run inside the endpoint's create span",
            false,
            STATE_CREATE_IN_PROGRESS,
        ),
        (
            "a dry run once the endpoint is create-complete",
            true,
            STATE_CREATE_COMPLETE,
        ),
    ] {
        if wait_first {
            past_the_transition().await;
        }
        let (status, body) = delete(
            &fixture.router,
            &[
                p("InstanceConnectEndpointId", created.id.clone()),
                p("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        let row = described_row(&fixture.router, case, &created.id).await;
        assert_eq!(
            row.state, expected_state,
            "case {case:?}: expected the endpoint to still be {expected_state}, got {row:?}"
        );
        assert_eq!(
            fixture.stored_endpoints(),
            1,
            "case {case:?}: a dry run changes nothing the store holds"
        );
    }

    // The delete the dry run stood for still works afterwards.
    let deleted = delete_endpoint(
        &fixture.router,
        "the delete the dry runs stood for",
        &[p("InstanceConnectEndpointId", created.id.clone())],
    )
    .await;
    assert_eq!(
        deleted.state, STATE_DELETE_IN_PROGRESS,
        "the delete a dry run reported as workable must work, got {deleted:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 7: a modify replaces exactly the members the request carries
// ---------------------------------------------------------------------------

async fn modify(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ModifyInstanceConnectEndpoint", params).await
}

/// A modify that must succeed: HTTP 200 whose envelope reports `return` true — "Is true if
/// the request succeeds and an error otherwise"
/// (`ModifyInstanceConnectEndpointResult$Return`) is the whole of what the operation answers.
async fn modify_endpoint(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) {
    let (status, body) = modify(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyInstanceConnectEndpoint, \
         got {status} with body {body}"
    );
    assert_eq!(
        envelope_member(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: expected the envelope to report return=true, got {body}"
    );
}

/// `row` with its security-group ids sorted: `SecurityGroupIds` names a SET of associations,
/// so a comparison reads it as one rather than as the order a request happened to name.
fn normalized(row: &EndpointRow) -> EndpointRow {
    let mut row = row.clone();
    row.security_group_ids.sort();
    row
}

/// One row of the member-replacement table: `(the case, what the modify carries beside the
/// id, and the address type, the client-IP flag and the security groups the endpoint must
/// then report)`.
type ModifiedRow = (
    &'static str,
    Params,
    &'static str,
    &'static str,
    Vec<String>,
);

/// One row of the client-IP table: `(the case, the subnet's families, what the create
/// carried, what the modify carries, and the (address type, client-IP flag) the endpoint
/// must then report — `None` when the modify must be refused)`.
type ClientIpRow = (
    &'static str,
    &'static str,
    Params,
    Params,
    Option<(&'static str, &'static str)>,
);

/// The fixture subnet the `families` label of a table row names.
fn subnet_of(fixture: &Fixture, families: &str) -> String {
    match families {
        "ipv4" => fixture.ipv4_subnet.clone(),
        "dual" => fixture.dual_subnet.clone(),
        _ => fixture.ipv6_subnet.clone(),
    }
}

/// RULE 7: a modify replaces exactly the members the request carries and leaves every other
/// member of that endpoint — and every other endpoint — as it was, and the change is read
/// back through `DescribeInstanceConnectEndpoints`.
///
/// The rows differ along what the rule generalises over: which of the three modifiable
/// members the request carries, how many values a list member carries, whether it repeats
/// one, and a request carrying none of them at all.
#[tokio::test]
async fn a_modify_replaces_exactly_the_members_the_request_carries() {
    let fixture = fixture();
    let mut both_groups = vec![
        fixture.default_group.clone(),
        fixture.extra_group.clone(),
    ];
    both_groups.sort();
    // (the case, what the modify carries beside the id, and the address type, the client-IP
    //  flag and the security groups the endpoint must then report)
    let rows: Vec<ModifiedRow> = vec![
        (
            "a request carrying no modifiable member at all",
            Vec::new(),
            IP_ADDRESS_TYPE_IPV4,
            "false",
            vec![fixture.default_group.clone()],
        ),
        (
            "the address type alone",
            vec![p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK)],
            IP_ADDRESS_TYPE_DUALSTACK,
            "false",
            vec![fixture.default_group.clone()],
        ),
        (
            "the client-IP flag alone",
            vec![p("PreserveClientIp", "true")],
            IP_ADDRESS_TYPE_IPV4,
            "true",
            vec![fixture.default_group.clone()],
        ),
        (
            "one security group replacing the current set",
            indexed("SecurityGroupId", &[fixture.extra_group.as_str()]),
            IP_ADDRESS_TYPE_IPV4,
            "false",
            vec![fixture.extra_group.clone()],
        ),
        (
            "two security groups replacing one",
            indexed(
                "SecurityGroupId",
                &[fixture.extra_group.as_str(), fixture.default_group.as_str()],
            ),
            IP_ADDRESS_TYPE_IPV4,
            "false",
            both_groups.clone(),
        ),
        (
            "the same security group named twice",
            indexed(
                "SecurityGroupId",
                &[fixture.extra_group.as_str(), fixture.extra_group.as_str()],
            ),
            IP_ADDRESS_TYPE_IPV4,
            "false",
            vec![fixture.extra_group.clone()],
        ),
        (
            "every modifiable member at once",
            {
                let mut params = vec![
                    p("IpAddressType", IP_ADDRESS_TYPE_IPV6),
                    p("PreserveClientIp", "false"),
                ];
                params.extend(indexed("SecurityGroupId", &[fixture.extra_group.as_str()]));
                params
            },
            IP_ADDRESS_TYPE_IPV6,
            "false",
            vec![fixture.extra_group.clone()],
        ),
    ];

    // The baseline every row starts from: an endpoint of the dual-stack subnet created
    // `ipv4`, without the client-IP flag and holding the VPC's default security group, so a
    // row can move each of the three modifiable members in either direction.
    let baseline = || -> Params {
        let mut params = vec![
            p("SubnetId", fixture.dual_subnet.clone()),
            p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
        ];
        params.extend(tag_params(&[("Name", "modifiable")]));
        params
    };
    // One endpoint per row, all created before a single wait: each row then compares a
    // `create-complete` endpoint before and after its OWN modify, so what the table observes
    // is the modify and not the clock.
    let mut endpoints = Vec::new();
    for (case, ..) in &rows {
        let created = create_endpoint(&fixture.router, case, &baseline()).await;
        endpoints.push(created.id);
    }
    // An endpoint no row names, so each row also shows that a modify touches only the
    // endpoint it names.
    let bystander = create_endpoint(&fixture.router, "the endpoint no row modifies", &baseline())
        .await
        .id;
    past_the_transition().await;
    let bystander_before =
        described_row(&fixture.router, "the endpoint no row modifies", &bystander).await;

    for ((case, extra, ip_address_type, preserve_client_ip, group_ids), endpoint_id) in
        rows.iter().zip(&endpoints)
    {
        let before = normalized(&described_row(&fixture.router, case, endpoint_id).await);
        let mut params = vec![p("InstanceConnectEndpointId", endpoint_id.clone())];
        params.extend(extra.clone());
        modify_endpoint(&fixture.router, case, &params).await;

        // Exactly the three modifiable members move: the id, the ARN, the account, the DNS
        // names, the VPC, the subnet, the zone, the create time, the tags and the state are
        // all still what they were before the call.
        let mut expected_groups = group_ids.clone();
        expected_groups.sort();
        let expected = EndpointRow {
            ip_address_type: (*ip_address_type).to_string(),
            preserve_client_ip: (*preserve_client_ip).to_string(),
            security_group_ids: expected_groups,
            ..before
        };
        let after = normalized(&described_row(&fixture.router, case, endpoint_id).await);
        assert_eq!(
            after, expected,
            "case {case:?}: expected the endpoint to report {expected:?}, got {after:?}"
        );

        let bystander_now = described_row(&fixture.router, case, &bystander).await;
        assert_eq!(
            bystander_now, bystander_before,
            "case {case:?}: a modify must touch only the endpoint it names, \
             got {bystander_now:?}"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        rows.len() + 1,
        "a modify stores no endpoint of its own"
    );
}

/// RULE 7: "The new IP address type for the EC2 Instance Connect Endpoint"
/// (`ModifyInstanceConnectEndpointRequest$IpAddressType`) has to be one the endpoint's own
/// SUBNET has a CIDR block for — the same fact that determines the type a create resolves —
/// so a type the subnet cannot serve is refused and the endpoint keeps the type it had.
///
/// The rows are the subnet's address families crossed with every value of the modelled
/// `IpAddressType` enum (`smithy_query(query="shape",shape="IpAddressType")`), so no value
/// of that catalog is left untested.
#[tokio::test]
async fn the_new_address_type_must_be_served_by_the_endpoints_subnet() {
    let fixture = fixture();
    // (the endpoint's subnet families, the type the modify asks for, the type the endpoint
    //  must then report — None when the request must be refused)
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("ipv4", IP_ADDRESS_TYPE_IPV4, Some(IP_ADDRESS_TYPE_IPV4)),
        ("ipv4", IP_ADDRESS_TYPE_DUALSTACK, None),
        ("ipv4", IP_ADDRESS_TYPE_IPV6, None),
        ("dual", IP_ADDRESS_TYPE_IPV4, Some(IP_ADDRESS_TYPE_IPV4)),
        (
            "dual",
            IP_ADDRESS_TYPE_DUALSTACK,
            Some(IP_ADDRESS_TYPE_DUALSTACK),
        ),
        ("dual", IP_ADDRESS_TYPE_IPV6, Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", IP_ADDRESS_TYPE_IPV4, None),
        ("ipv6", IP_ADDRESS_TYPE_DUALSTACK, None),
        ("ipv6", IP_ADDRESS_TYPE_IPV6, Some(IP_ADDRESS_TYPE_IPV6)),
    ];

    for (families, requested, expected) in rows {
        let subnet_id = subnet_of(&fixture, families);
        let case = format!("a {families} endpoint asked for {requested}");
        let created =
            create_endpoint(&fixture.router, &case, &[p("SubnetId", subnet_id.clone())]).await;
        let (status, body) = modify(
            &fixture.router,
            &[
                p("InstanceConnectEndpointId", created.id.clone()),
                p("IpAddressType", requested),
            ],
        )
        .await;
        let row = match expected {
            Some(reported) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                let row = described_row(&fixture.router, &case, &created.id).await;
                assert_eq!(
                    row.ip_address_type, reported,
                    "case {case:?}: expected ipAddressType {reported}, got {row:?}"
                );
                row
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterCombination",
                    "case {case:?}: expected InvalidParameterCombination, got {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains(requested) && message.contains(subnet_id.as_str()),
                    "case {case:?}: the refusal must name the type asked for and the \
                     endpoint's subnet, got {message:?}"
                );
                let row = described_row(&fixture.router, &case, &created.id).await;
                assert_eq!(
                    row.ip_address_type, created.ip_address_type,
                    "case {case:?}: a refused modify must change nothing, got {row:?}"
                );
                row
            }
        };
        // Whatever the answer, the members the request never named are untouched.
        assert_eq!(
            (
                row.subnet_id.as_str(),
                row.security_group_ids.as_slice(),
                row.preserve_client_ip.as_str()
            ),
            (
                created.subnet_id.as_str(),
                created.security_group_ids.as_slice(),
                created.preserve_client_ip.as_str()
            ),
            "case {case:?}: a modify naming only the address type must move only it, \
             got {row:?}"
        );
    }
}

/// RULE 7: "`PreserveClientIp` is only supported on IPv4 EC2 Instance Connect Endpoints. To
/// use `PreserveClientIp`, the value for `IpAddressType` must be `ipv4`"
/// (`ModifyInstanceConnectEndpointRequest$IpAddressType`) — the combination the endpoint
/// would END UP in is what the rule holds for, so neither changing the address type nor
/// changing the flag can leave the endpoint preserving the client IP on an endpoint that is
/// not `ipv4`, whichever of the two the request moves.
#[tokio::test]
async fn the_client_ip_flag_and_the_address_type_are_checked_together() {
    let fixture = fixture();
    // (the case, the endpoint's subnet families, what its create carried, what the modify
    //  carries, and the (address type, client-IP flag) the endpoint must then report — None
    //  when the modify must be refused)
    let rows: Vec<ClientIpRow> = vec![
        (
            "turning the flag on for an ipv4 endpoint",
            "ipv4",
            Vec::new(),
            vec![p("PreserveClientIp", "true")],
            Some((IP_ADDRESS_TYPE_IPV4, "true")),
        ),
        (
            "turning the flag back off",
            "ipv4",
            vec![p("PreserveClientIp", "true")],
            vec![p("PreserveClientIp", "false")],
            Some((IP_ADDRESS_TYPE_IPV4, "false")),
        ),
        (
            "moving an endpoint that preserves the client IP to dualstack",
            "dual",
            vec![
                p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                p("PreserveClientIp", "true"),
            ],
            vec![p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK)],
            None,
        ),
        (
            "moving it to dualstack and dropping the flag in one request",
            "dual",
            vec![
                p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                p("PreserveClientIp", "true"),
            ],
            vec![
                p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK),
                p("PreserveClientIp", "false"),
            ],
            Some((IP_ADDRESS_TYPE_DUALSTACK, "false")),
        ),
        (
            "turning the flag on for a dualstack endpoint",
            "dual",
            Vec::new(),
            vec![p("PreserveClientIp", "true")],
            None,
        ),
        (
            "turning the flag on while moving to ipv4",
            "dual",
            Vec::new(),
            vec![
                p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                p("PreserveClientIp", "true"),
            ],
            Some((IP_ADDRESS_TYPE_IPV4, "true")),
        ),
        (
            "turning the flag on for an ipv6 endpoint",
            "ipv6",
            Vec::new(),
            vec![p("PreserveClientIp", "true")],
            None,
        ),
    ];

    for (case, families, created_with, modified_with, expected) in rows {
        let mut create_params = vec![p("SubnetId", subnet_of(&fixture, families))];
        create_params.extend(created_with);
        let created = create_endpoint(&fixture.router, case, &create_params).await;
        let mut params = vec![p("InstanceConnectEndpointId", created.id.clone())];
        params.extend(modified_with);
        let (status, body) = modify(&fixture.router, &params).await;
        match expected {
            Some((ip_address_type, preserve_client_ip)) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                let row = described_row(&fixture.router, case, &created.id).await;
                assert_eq!(
                    (
                        row.ip_address_type.as_str(),
                        row.preserve_client_ip.as_str()
                    ),
                    (ip_address_type, preserve_client_ip),
                    "case {case:?}: expected ({ip_address_type}, {preserve_client_ip}), \
                     got {row:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterCombination",
                    "case {case:?}: expected InvalidParameterCombination, got {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains("preserveClientIp"),
                    "case {case:?}: the refusal must name the member it is about, \
                     got {message:?}"
                );
                let row = described_row(&fixture.router, case, &created.id).await;
                assert_eq!(
                    (
                        row.ip_address_type.as_str(),
                        row.preserve_client_ip.as_str()
                    ),
                    (
                        created.ip_address_type.as_str(),
                        created.preserve_client_ip.as_str()
                    ),
                    "case {case:?}: a refused modify must change nothing, got {row:?}"
                );
            }
        }
    }
}

/// RULE 7: a modify that cannot resolve what the request names is REFUSED — the endpoint id
/// malformed before unknown, in the wording EC2 uses for the class, and a security group the
/// endpoint's network cannot hold — and the endpoint it named is left exactly as it was.
///
/// The rows differ along why the request cannot be served: the shape of the id text, the
/// length of its suffix, the resource its prefix names, a well-formed id of each id era the
/// account holds no endpoint for, and a group that is not an id, is not stored, or belongs
/// to another VPC.
#[tokio::test]
async fn a_modify_that_cannot_resolve_what_it_names_is_refused() {
    let fixture = fixture();
    let endpoint = create_endpoint(
        &fixture.router,
        "the endpoint every refusal row leaves alone",
        &[p("SubnetId", fixture.dual_subnet.clone())],
    )
    .await;

    let rows: Vec<RefusalRow> = vec![
        (
            "an id that is not shaped like an endpoint id",
            vec![p("InstanceConnectEndpointId", "not-an-endpoint")],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "an id whose prefix names another resource",
            vec![p("InstanceConnectEndpointId", "subnet-0123456789abcdef0")],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "an endpoint id whose suffix is of no id era",
            vec![p(
                "InstanceConnectEndpointId",
                format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}0123456"),
            )],
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "a well-formed short id the account holds no endpoint for",
            vec![p(
                "InstanceConnectEndpointId",
                format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}0123abcd"),
            )],
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a well-formed long id the account holds no endpoint for",
            vec![p(
                "InstanceConnectEndpointId",
                format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}0123456789abcdef0"),
            )],
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a security group id that is not an id at all",
            vec![
                p("InstanceConnectEndpointId", endpoint.id.clone()),
                p("SecurityGroupId.1", "not-a-group"),
            ],
            "InvalidID",
        ),
        (
            "a security group the account does not hold",
            vec![
                p("InstanceConnectEndpointId", endpoint.id.clone()),
                p("SecurityGroupId.1", "sg-0123456789abcdef0"),
            ],
            "InvalidGroup.NotFound",
        ),
        (
            "a security group of another VPC",
            vec![
                p("InstanceConnectEndpointId", endpoint.id.clone()),
                p("SecurityGroupId.1", fixture.foreign_group.clone()),
            ],
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_code) in rows {
        let (status, body) = modify(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // The refusal must name the value it could not resolve: the group when the row is
        // about a group, otherwise the endpoint id.
        let named = ["SecurityGroupId.1", "InstanceConnectEndpointId"]
            .into_iter()
            .find_map(|key| {
                params
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_default();
        let message = error_message(&body);
        assert!(
            message.contains(&named),
            "case {case:?}: the refusal must name what it could not resolve ({named}), \
             got {message:?}"
        );
        let row = described_row(&fixture.router, case, &endpoint.id).await;
        assert_eq!(
            normalized(&row),
            normalized(&endpoint),
            "case {case:?}: a refused modify must leave the endpoint as it was, got {row:?}"
        );
    }
}

/// RULE 7 under `DryRun`: the flag asks whether the modify WOULD have succeeded — HTTP 412
/// `DryRunOperation` — and the endpoint keeps every member it had, so nothing was written;
/// the modify the dry run stood for then works.
#[tokio::test]
async fn a_dry_run_modify_writes_nothing() {
    let fixture = fixture();
    let created = create_endpoint(
        &fixture.router,
        "the endpoint the dry run names",
        &[
            p("SubnetId", fixture.dual_subnet.clone()),
            p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
        ],
    )
    .await;
    let change = vec![
        p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK),
        p("SecurityGroupId.1", fixture.extra_group.clone()),
    ];

    let mut dry = vec![
        p("InstanceConnectEndpointId", created.id.clone()),
        p("DryRun", "true"),
    ];
    dry.extend(change.clone());
    let (status, body) = modify(&fixture.router, &dry).await;
    assert_eq!(
        status, 412,
        "expected HTTP 412 from a dry-run modify, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "expected DryRunOperation, got {body}"
    );
    let row = described_row(&fixture.router, "after the dry run", &created.id).await;
    assert_eq!(
        normalized(&row),
        normalized(&created),
        "a dry run must write nothing, got {row:?}"
    );

    // A dry run of a request that would have been REFUSED is refused with that reason
    // instead of with `DryRunOperation`.
    let (status, body) = modify(
        &fixture.router,
        &[
            p("InstanceConnectEndpointId", created.id.clone()),
            p("DryRun", "true"),
            p("SecurityGroupId.1", fixture.foreign_group.clone()),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "a dry run of a request that would have been refused must carry that reason, \
         got {status} with body {body}"
    );

    let mut real = vec![p("InstanceConnectEndpointId", created.id.clone())];
    real.extend(change);
    modify_endpoint(
        &fixture.router,
        "the modify the dry run stood for",
        &real,
    )
    .await;
    let row = described_row(&fixture.router, "after the real modify", &created.id).await;
    assert_eq!(
        (
            row.ip_address_type.as_str(),
            row.security_group_ids.as_slice()
        ),
        (
            IP_ADDRESS_TYPE_DUALSTACK,
            [fixture.extra_group.clone()].as_slice()
        ),
        "the modify a dry run reported as workable must work, got {row:?}"
    );
}

/// RULE 7: an endpoint whose deletion has been asked for is being torn down, so it has no
/// configuration left to change: the modify is refused `IncorrectState` naming the state the
/// endpoint IS in, in either delete state, and writes nothing.
#[tokio::test]
async fn a_modify_of_an_endpoint_being_deleted_is_refused() {
    let fixture = fixture();
    let created = create_endpoint(
        &fixture.router,
        "the endpoint the delete names",
        &[
            p("SubnetId", fixture.dual_subnet.clone()),
            p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
        ],
    )
    .await;
    delete_endpoint(
        &fixture.router,
        "the delete that starts the teardown",
        &[p("InstanceConnectEndpointId", created.id.clone())],
    )
    .await;

    for (case, wait_first, expected_state) in [
        (
            "a modify inside the endpoint's delete span",
            false,
            STATE_DELETE_IN_PROGRESS,
        ),
        (
            "a modify once the endpoint is delete-complete",
            true,
            STATE_DELETE_COMPLETE,
        ),
    ] {
        if wait_first {
            past_the_transition().await;
        }
        let before = described_row(&fixture.router, case, &created.id).await;
        assert_eq!(
            before.state, expected_state,
            "case {case:?}: expected the endpoint to be {expected_state}, got {before:?}"
        );
        let (status, body) = modify(
            &fixture.router,
            &[
                p("InstanceConnectEndpointId", created.id.clone()),
                p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&created.id) && message.contains(expected_state),
            "case {case:?}: the refusal must name the endpoint and the state it is in, \
             got {message:?}"
        );
        let after = described_row(&fixture.router, case, &created.id).await;
        assert_eq!(
            after.ip_address_type, before.ip_address_type,
            "case {case:?}: a refused modify must write nothing, got {after:?}"
        );
    }
}

/// RULE 7 end to end over the operations a CLIENT uses, with nothing seeded into the store:
/// the VPC, the subnet and the security group are all minted through the service's own
/// operations, the endpoint is created in them, the clock completes it, the modify moves the
/// members it names, and the describe reads the modified endpoint back.
#[tokio::test]
async fn a_modify_works_end_to_end_over_a_network_the_operations_created() {
    let (router, _engine) = super::ec2_query::emulator();
    /// One call that must succeed, as its response body.
    async fn ok(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> String {
        let (status, body) = call(router, REGION, action, params).await;
        assert_eq!(
            status, 200,
            "expected HTTP 200 from {action}, got {status} with body {body}"
        );
        body
    }
    /// The one `name` element of a response, or a panic naming the action it came from.
    fn minted(action: &str, body: &str, name: &str) -> String {
        element(body, name)
            .unwrap_or_else(|| panic!("expected a <{name}> in the {action} response: {body}"))
    }

    let body = ok(&router, "CreateVpc", &[p("CidrBlock", "10.9.0.0/16")]).await;
    let vpc_id = minted("CreateVpc", &body, "vpcId");
    let body = ok(
        &router,
        "CreateSubnet",
        &[p("VpcId", vpc_id.clone()), p("CidrBlock", "10.9.1.0/24")],
    )
    .await;
    let subnet_id = minted("CreateSubnet", &body, "subnetId");
    let body = ok(
        &router,
        "CreateSecurityGroup",
        &[
            p("VpcId", vpc_id.clone()),
            p("GroupName", "endpoint-access"),
            p("GroupDescription", "the group the modify associates"),
        ],
    )
    .await;
    let group_id = minted("CreateSecurityGroup", &body, "groupId");

    let created = create_endpoint(
        &router,
        "the endpoint of the created network",
        &[p("SubnetId", subnet_id.clone())],
    )
    .await;
    assert_eq!(
        (created.state.as_str(), created.vpc_id.as_str()),
        (STATE_CREATE_IN_PROGRESS, vpc_id.as_str()),
        "a fresh endpoint of the created subnet must be create-in-progress in that VPC, \
         got {created:?}"
    );
    // The default group of the VPC the operations created, since the create named none.
    assert_ne!(
        created.security_group_ids,
        vec![group_id.clone()],
        "the create named no group, so it must not have picked the one made for the modify"
    );
    past_the_transition().await;
    let completed = described_row(&router, "the completed endpoint", &created.id).await;
    assert_eq!(
        completed.state, STATE_CREATE_COMPLETE,
        "the clock must complete the creation, got {completed:?}"
    );

    modify_endpoint(
        &router,
        "the modify of the created endpoint",
        &[
            p("InstanceConnectEndpointId", created.id.clone()),
            p("PreserveClientIp", "true"),
            p("SecurityGroupId.1", group_id.clone()),
        ],
    )
    .await;
    let modified = described_row(&router, "the modified endpoint", &created.id).await;
    assert_eq!(
        (
            modified.preserve_client_ip.as_str(),
            modified.security_group_ids.as_slice(),
            modified.ip_address_type.as_str(),
            modified.state.as_str(),
        ),
        (
            "true",
            [group_id.clone()].as_slice(),
            IP_ADDRESS_TYPE_IPV4,
            STATE_CREATE_COMPLETE,
        ),
        "the modify must replace the members it named and leave the rest, got {modified:?}"
    );

    // The endpoint the modify associated the group with is the one the delete tears down,
    // and a modify of it afterwards is refused rather than written.
    let deleted = delete_endpoint(
        &router,
        "the delete after the modify",
        &[p("InstanceConnectEndpointId", created.id.clone())],
    )
    .await;
    assert_eq!(
        deleted.state, STATE_DELETE_IN_PROGRESS,
        "the delete must start the teardown of the modified endpoint, got {deleted:?}"
    );
    let (status, body) = modify(
        &router,
        &[
            p("InstanceConnectEndpointId", created.id.clone()),
            p("PreserveClientIp", "false"),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectState"),
        "a modify of an endpoint being deleted must be refused, got {status} with body {body}"
    );
}
