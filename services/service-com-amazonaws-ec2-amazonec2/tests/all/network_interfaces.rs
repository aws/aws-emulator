// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateNetworkInterface` and `DescribeNetworkInterfaces`, driven
//! over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! Two rules are under test.
//!
//! * `DescribeNetworkInterfaces` REPORTS THE STORE: the set it answers with is the
//!   account's network interfaces narrowed by the `NetworkInterfaceIds` and `Filters` the
//!   request names and cut into `MaxResults`-sized pages. A fresh account holds none, so
//!   the empty set is one row of that rule rather than a fixed answer — the tables below
//!   vary the filter (each documented name), the number of values and filters, the ids and
//!   the page size.
//! * `CreateNetworkInterface` stores an interface the describe then reports: its subnet,
//!   VPC, Availability Zone, security groups, description, type, tags and private IPv4
//!   addresses round-trip, and the addresses it takes leave the subnet's available count.
//!
//! The ceilings on the filtering work one describe can ask for are covered too: matching
//! cost is the PRODUCT of how many filters a request carries, how many values each
//! carries, how long a value is and how many interfaces the store holds, and the model
//! bounds none of them (`FilterList` and `Filter$Values` carry no `@length`), so a request
//! AT each of this crate's ceilings is answered while one PAST it is refused.

use ec2::ops::types::{
    GroupIdentifier, NetworkInterfaceAssociation, NetworkInterfaceAttachment,
    NetworkInterfaceIpv6Address, OperatorResponse, Tag,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES,
    MAX_FILTERS, MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE, MAX_RESOURCE_IDS, NetworkInterfaceSpec,
    SubnetSpec, availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The stores hold the account's resources
/// rather than one Region's, and the default VPC's default subnets are seeded for the
/// emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// The instant the one seeded attachment was made, as a fixed point so the
/// `attachment.attach-time` filter has a value to compare against. It is a seeded fact,
/// not something the emulator mints.
const SEEDED_ATTACH_TIME_SECS: i64 = 1_767_225_600;

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `DescribeNetworkInterfaces` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeNetworkInterfaces", params).await
}

/// One `CreateNetworkInterface` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateNetworkInterface", params).await
}

/// The interface ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "networkInterfaceId")
}

/// The XML of the one interface `network_interface_id` names.
async fn describe_one(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> String {
    let (status, body) = describe(
        router,
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// A tag, as the seeded interfaces carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// The subnet ids of a store, in the order the subnet store holds them (which is id
/// order, so the default subnets come in zone order).
fn subnet_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .subnets
        .keys()
        .cloned()
        .collect()
}

/// The `GroupId` of the default security group of `vpc_id`.
fn default_group_of(engine: &engine::InMemoryEngine, vpc_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .security_groups
        .values()
        .find(|group| {
            group.vpc_id.as_deref() == Some(vpc_id) && group.group_name.as_deref() == Some("default")
        })
        .and_then(|group| group.group_id.clone())
        .expect("every VPC of this store has its default security group")
}

/// The `VpcId` of the account's default VPC.
fn default_vpc_of(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC")
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account holds NO network interface: AWS creates the default VPC, its subnets
/// and its security group for a new account, but nothing gives it an interface until
/// something asks for one. Describing them is a READ, so two calls report the same empty
/// set and the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_no_network_interfaces_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let first_ids = elements(&first, "networkInterfaceId");
    assert!(
        first_ids.is_empty(),
        "fresh account: expected no network interfaces, got {first_ids:?} from {first}"
    );
    // The account's default network is there all the same, so the empty set is the
    // interface store being empty and not the request failing to find anything.
    assert_eq!(
        engine.state::<Ec2State>().lock().subnets.len(),
        availability_zones_in_region(REGION).len(),
        "fresh account: expected the default subnets to exist beside the empty interface set"
    );

    let second_ids = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second_ids, first_ids,
        "a read must not change what a later caller sees: {first_ids:?} then {second_ids:?}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_interfaces
            .is_empty(),
        "describing network interfaces must not write to the store"
    );
}

/// Every interface the store holds is reported, in store order, and one written after a
/// call shows up in the next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn every_stored_interface_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all interfaces", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all interfaces: expected every stored interface in store order, got {reported:?}"
    );

    // A create through the wire is the later write, so the describe reports a resource the
    // emulator itself minted rather than only seeded records.
    let (status, body) = create(
        &router,
        &[
            param("SubnetId", &seeded.subnet_ids[0]),
            param("Description", "late"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "late create: expected HTTP 200, got {status} with body {body}"
    );
    let late_id = element(&body, "networkInterfaceId").unwrap_or_default();
    assert!(
        late_id.starts_with("eni-"),
        "late create: expected an eni- id, got {late_id:?} from {body}"
    );

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    expected.sort();
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// Describing is a READ whatever the request asks for, so no shape of request may change
/// what a later caller sees. The rows differ along everything a request can name — nothing
/// at all, one id, every id, a filter that selects a subset, a filter that selects nothing,
/// two filters joined, a tag filter, a page smaller than the store, the managed-resource
/// flag, a dry run, and the two refusals — and after each one the store is exactly what it
/// was and a second identical call answers identically. One request reporting the store is
/// not evidence of that: the rule holds over the SET of request shapes.
#[tokio::test]
async fn no_shape_of_describe_changes_the_store_or_a_later_answer() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let every_id: Vec<&str> = seeded.ids.iter().map(String::as_str).collect();

    /// A well-formed interface id the account holds no interface for.
    const UNHELD_ID: &str = "eni-00000000000000fed";

    let shapes: Vec<(&str, Vec<(String, String)>)> = vec![
        ("the whole store", Vec::new()),
        (
            "one named id",
            indexed("NetworkInterfaceId", &[seeded.ids[0].as_str()]),
        ),
        ("every named id", indexed("NetworkInterfaceId", &every_id)),
        (
            "a filter that selects a subset",
            filters(&[("subnet-id", &[seeded.subnet_ids[0].as_str()])]),
        ),
        (
            "a filter that selects nothing",
            filters(&[("description", &["no interface carries this description"])]),
        ),
        (
            "two filters joined with an AND",
            filters(&[
                ("status", &["available"]),
                ("interface-type", &["interface"]),
            ]),
        ),
        ("a tag filter", filters(&[("tag:Name", &["web"])])),
        (
            "a page smaller than the store",
            vec![param("MaxResults", "5")],
        ),
        (
            "the managed-resource flag",
            vec![param("IncludeManagedResources", "true")],
        ),
        ("a dry run", vec![param("DryRun", "true")]),
        (
            "an id the account holds no interface for",
            indexed("NetworkInterfaceId", &[UNHELD_ID]),
        ),
        (
            "a filter name the operation does not document",
            filters(&[("not-a-filter", &["anything"])]),
        ),
    ];

    // The interfaces and the subnets they take their addresses from: what a write by this
    // operation would have to show up in.
    let snapshot = || {
        let state = engine.state::<Ec2State>();
        let store = state.lock();
        format!("{:?}|{:?}", store.network_interfaces, store.subnets)
    };
    let before = snapshot();
    assert!(
        !seeded.ids.is_empty(),
        "the invariant is asserted against a POPULATED store, which holds {} interfaces",
        seeded.ids.len()
    );

    for (case, params) in shapes {
        let (first_status, first_body) = describe(&router, &params).await;
        let (second_status, second_body) = describe(&router, &params).await;
        assert_eq!(
            first_status, second_status,
            "case {case:?}: expected the repeat to answer HTTP {first_status}, got \
             {second_status} with body {second_body}"
        );
        let first_ids = elements(&first_body, "networkInterfaceId");
        let second_ids = elements(&second_body, "networkInterfaceId");
        assert_eq!(
            first_ids, second_ids,
            "case {case:?}: expected the repeat to report {first_ids:?}, got {second_ids:?}"
        );
        assert_eq!(
            element(&first_body, "nextToken"),
            element(&second_body, "nextToken"),
            "case {case:?}: expected the repeat to carry the same token, got {second_body}"
        );
        assert_eq!(
            error_code(&first_body),
            error_code(&second_body),
            "case {case:?}: expected the repeat to carry the same error code, got \
             {second_body}"
        );
        assert_eq!(
            snapshot(),
            before,
            "case {case:?}: describing network interfaces must write nothing, and the \
             answer was HTTP {first_status} with body {first_body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Create, then read it back
// ---------------------------------------------------------------------------

/// One row of the create table: the request's extra form fields, and what the interface
/// the describe reports back has to carry.
struct CreateCase {
    name: &'static str,
    params: Vec<(String, String)>,
    /// The `description` the interface reports.
    description: &'static str,
    /// The `interfaceType` it reports.
    interface_type: &'static str,
    /// How many private IPv4 addresses it holds.
    addresses: usize,
}

/// Creating an interface stores it, and every member the request set round-trips through
/// `DescribeNetworkInterfaces` — the rows differ along what a create varies: the
/// description, the interface type, how the private addresses are chosen and how many,
/// the security groups and the tags.
#[tokio::test]
async fn each_created_interface_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let subnets = subnet_ids(&engine);
    let subnet_id = subnets[0].clone();
    let default_vpc = default_vpc_of(&engine);
    let default_group = default_group_of(&engine, &default_vpc);
    let zones = availability_zones_in_region(REGION);
    // The default subnets are the per-zone /20 blocks of the default VPC's 172.31.0.0/16,
    // so the first one's assignable addresses start at 172.31.0.4.
    let cases = vec![
        CreateCase {
            name: "no optional member",
            params: Vec::new(),
            description: "",
            interface_type: "interface",
            addresses: 1,
        },
        CreateCase {
            name: "a description",
            params: vec![param("Description", "primary interface")],
            description: "primary interface",
            interface_type: "interface",
            addresses: 1,
        },
        CreateCase {
            name: "an explicit primary address",
            params: vec![param("PrivateIpAddress", "172.31.0.42")],
            description: "",
            interface_type: "interface",
            addresses: 1,
        },
        CreateCase {
            name: "three secondary addresses",
            params: vec![param("SecondaryPrivateIpAddressCount", "3")],
            description: "",
            interface_type: "interface",
            addresses: 4,
        },
        CreateCase {
            name: "two addresses named one by one",
            params: vec![
                param("PrivateIpAddresses.1.PrivateIpAddress", "172.31.0.50"),
                param("PrivateIpAddresses.1.Primary", "true"),
                param("PrivateIpAddresses.2.PrivateIpAddress", "172.31.0.51"),
                param("PrivateIpAddresses.2.Primary", "false"),
            ],
            description: "",
            interface_type: "interface",
            addresses: 2,
        },
        CreateCase {
            name: "an efa interface",
            params: vec![param("InterfaceType", "efa")],
            description: "",
            interface_type: "efa",
            addresses: 1,
        },
        CreateCase {
            name: "a trunk interface",
            params: vec![param("InterfaceType", "trunk")],
            description: "",
            interface_type: "trunk",
            addresses: 1,
        },
        CreateCase {
            // "If you specify efa-only, do not assign any IP addresses to the network
            // interface. EFA-only network interfaces do not support IP addresses"
            // (`CreateNetworkInterfaceRequest$InterfaceType`).
            name: "an efa-only interface",
            params: vec![param("InterfaceType", "efa-only")],
            description: "",
            interface_type: "efa-only",
            addresses: 0,
        },
        CreateCase {
            name: "a named security group",
            params: vec![param("SecurityGroupId.1", &default_group)],
            description: "",
            interface_type: "interface",
            addresses: 1,
        },
        CreateCase {
            name: "two tags",
            params: vec![
                param("TagSpecification.1.ResourceType", "network-interface"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "web"),
                param("TagSpecification.1.Tag.2.Key", "Tier"),
                param("TagSpecification.1.Tag.2.Value", "front"),
            ],
            description: "",
            interface_type: "interface",
            addresses: 1,
        },
    ];

    for case in cases {
        let mut params = vec![param("SubnetId", &subnet_id)];
        params.extend(case.params.clone());
        let (status, created) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {created}",
            case.name
        );
        let network_interface_id = element(&created, "networkInterfaceId").unwrap_or_default();
        assert!(
            network_interface_id.starts_with("eni-"),
            "case {:?}: expected an eni- id, got {network_interface_id:?} from {created}",
            case.name
        );
        // The create's OWN answer carries the status it reports for an interface it has just
        // minted: `pending`, while every READ of the interface nothing holds reports the
        // resting `available` — `network_interface_create_comes_up_pending` is the suite for
        // that pair, so the describe below reads every other member.
        assert_eq!(
            element(&created, "status").as_deref(),
            Some("pending"),
            "case {:?}: expected the create to answer the interface pending, got {created}",
            case.name
        );

        // Read it back through the describe: the two must report the same interface.
        let body = describe_one(&router, case.name, &network_interface_id).await;
        let expected_members = [
            ("networkInterfaceId", network_interface_id.clone()),
            ("subnetId", subnet_id.clone()),
            ("vpcId", default_vpc.clone()),
            ("availabilityZone", zones[0].zone_name.clone()),
            ("availabilityZoneId", zones[0].zone_id.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("interfaceType", case.interface_type.to_string()),
            ("sourceDestCheck", "true".to_string()),
            ("requesterManaged", "false".to_string()),
        ];
        for (member, value) in expected_members {
            assert!(
                body.contains(&format!("<{member}>{value}</{member}>")),
                "case {:?}: expected <{member}>{value}</{member}>, got {body}",
                case.name
            );
        }
        let described_description = element(&body, "description").unwrap_or_default();
        assert_eq!(
            described_description, case.description,
            "case {:?}: expected description {:?}, got {described_description:?} from {body}",
            case.name, case.description
        );
        let addresses = elements(&body, "privateIpAddressesSet")
            .first()
            .map(|set| elements(set, "privateIpAddress"))
            .unwrap_or_default();
        assert_eq!(
            addresses.len(),
            case.addresses,
            "case {:?}: expected {} private address(es), got {addresses:?} from {body}",
            case.name,
            case.addresses
        );
        // Every interface that carries addresses at all has exactly one primary, and it is
        // the address the interface itself reports.
        if case.addresses > 0 {
            let primary = element(&body, "privateIpAddress").unwrap_or_default();
            assert_eq!(
                primary, addresses[0],
                "case {:?}: expected the first address {:?} to be the primary one, got \
                 {primary:?} from {body}",
                case.name, addresses[0]
            );
            let primary_flags = elements(&body, "primary");
            assert_eq!(
                primary_flags.iter().filter(|flag| *flag == "true").count(),
                1,
                "case {:?}: expected exactly one primary address, got {primary_flags:?} from \
                 {body}",
                case.name
            );
        }
        // An interface created without groups gets its VPC's default security group.
        let reported_groups = elements(&body, "groupId");
        assert_eq!(
            reported_groups,
            vec![default_group.clone()],
            "case {:?}: expected the default security group, got {reported_groups:?} from {body}",
            case.name
        );
        // Every tag the request named round-trips into the interface's own `tagSet`, and a
        // request that named none leaves it empty — read out of the row's own request
        // fields, so a row that adds a tag is covered without a second list to keep in
        // step.
        let expected_tags: Vec<(String, String)> = (1..)
            .map_while(|index| {
                let field = |suffix: &str| {
                    let name = format!("TagSpecification.1.Tag.{index}.{suffix}");
                    case.params
                        .iter()
                        .find(|(field, _)| *field == name)
                        .map(|(_, value)| value.clone())
                };
                Some((field("Key")?, field("Value").unwrap_or_default()))
            })
            .collect();
        let tag_set = elements(&body, "tagSet").first().cloned().unwrap_or_default();
        let reported_keys = elements(&tag_set, "key");
        let reported_values = elements(&tag_set, "value");
        assert_eq!(
            reported_keys.len(),
            expected_tags.len(),
            "case {:?}: expected {} tag(s), got keys {reported_keys:?} from {body}",
            case.name,
            expected_tags.len()
        );
        for (key, value) in &expected_tags {
            let position = reported_keys.iter().position(|reported| reported == key);
            assert_eq!(
                position.and_then(|at| reported_values.get(at)),
                Some(value),
                "case {:?}: expected the tag {key:?}={value:?}, got keys {reported_keys:?} \
                 with values {reported_values:?} from {body}",
                case.name
            );
        }
    }

    // The addresses the requests named are the addresses the interfaces hold.
    let by_address = described_ids(
        &router,
        "the explicitly named address",
        &filters(&[("private-ip-address", &["172.31.0.42"])]),
    )
    .await;
    assert_eq!(
        by_address.len(),
        1,
        "the explicitly named address: expected the one interface that asked for it, got \
         {by_address:?}"
    );
}

/// The private addresses an interface takes come out of its subnet, so the subnet's
/// `AvailableIpAddressCount` drops by exactly the number assigned — "the number of IPv4
/// addresses in the subnet that are available" (`DescribeSubnets`' `Filters` doc).
#[tokio::test]
async fn created_addresses_leave_the_subnets_available_count() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = subnet_ids(&engine)[0].clone();

    async fn available(router: &MultiServiceRouter, subnet_id: &str) -> i64 {
        let (status, body) =
            super::ec2_query::call(router, REGION, "DescribeSubnets", &indexed("SubnetId", &[subnet_id]))
                .await;
        assert_eq!(
            status, 200,
            "DescribeSubnets: expected HTTP 200, got {status} with body {body}"
        );
        element(&body, "availableIpAddressCount")
            .unwrap_or_default()
            .parse()
            .expect("the subnet reports a number of available addresses")
    }

    // Each row asks for a different number of addresses, so the count tracks what was
    // taken rather than a fixed step.
    for (case, params, taken) in [
        ("one address", Vec::new(), 1),
        (
            "four addresses",
            vec![param("SecondaryPrivateIpAddressCount", "3")],
            4,
        ),
        (
            "no address at all",
            vec![param("InterfaceType", "efa-only")],
            0,
        ),
    ] {
        let before = available(&router, &subnet_id).await;
        let mut fields = vec![param("SubnetId", &subnet_id)];
        fields.extend(params);
        let (status, body) = create(&router, &fields).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = available(&router, &subnet_id).await;
        assert_eq!(
            before - after,
            taken,
            "case {case:?}: expected the available count to drop by {taken}, got {before} then \
             {after}"
        );
    }
}

/// A repeat of a create carrying the same `ClientToken` answers with the interface the
/// first call created and creates nothing: "Unique, case-sensitive identifier that you
/// provide to ensure the idempotency of the request"
/// (`CreateNetworkInterfaceRequest$ClientToken`).
#[tokio::test]
async fn a_repeated_client_token_creates_one_interface() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = subnet_ids(&engine)[0].clone();

    let params = vec![
        param("SubnetId", &subnet_id),
        param("Description", "idempotent"),
        param("ClientToken", "token-abc"),
    ];
    let (first_status, first_body) = create(&router, &params).await;
    assert_eq!(
        first_status, 200,
        "first call: expected HTTP 200, got {first_status} with body {first_body}"
    );
    let first_id = element(&first_body, "networkInterfaceId").unwrap_or_default();
    assert_eq!(
        element(&first_body, "clientToken").as_deref(),
        Some("token-abc"),
        "first call: expected the client token echoed back, got {first_body}"
    );

    let (second_status, second_body) = create(&router, &params).await;
    assert_eq!(
        second_status, 200,
        "repeat: expected HTTP 200, got {second_status} with body {second_body}"
    );
    let second_id = element(&second_body, "networkInterfaceId").unwrap_or_default();
    assert_eq!(
        second_id, first_id,
        "repeat: expected the interface the first call created, got {second_id:?} after \
         {first_id:?}"
    );
    let reported = described_ids(&router, "after the repeat", &[]).await;
    assert_eq!(
        reported,
        vec![first_id.clone()],
        "after the repeat: expected one interface in the store, got {reported:?}"
    );

    // A dry run never makes the request, and an idempotent repeat is still the request, so
    // a repeat carrying the token AND `DryRun` is answered the way a first dry run would
    // be — HTTP 412 `DryRunOperation` — rather than with the stored interface.
    let mut dry_run_params = params.clone();
    dry_run_params.push(param("DryRun", "true"));
    let (dry_run_status, dry_run_body) = create(&router, &dry_run_params).await;
    assert_eq!(
        dry_run_status, 412,
        "a dry-run repeat: expected HTTP 412, got {dry_run_status} with body {dry_run_body}"
    );
    let code = error_code(&dry_run_body);
    assert_eq!(
        code, "DryRunOperation",
        "a dry-run repeat: expected error code \"DryRunOperation\", got {:?} from \
         {dry_run_body}",
        unescaped(&code)
    );
    let after_dry_run = described_ids(&router, "after the dry-run repeat", &[]).await;
    assert_eq!(
        after_dry_run,
        vec![first_id.clone()],
        "after the dry-run repeat: expected the one interface still in the store, got \
         {after_dry_run:?}"
    );

    // A token is idempotent for the REQUEST it was first given with: replayed with any
    // member the caller chooses changed, it names an interface that request did not ask
    // for, so EC2 refuses it rather than answering with the stored one. The rows differ
    // along which member moved.
    let other_subnet = subnet_ids(&engine)[1].clone();
    let default_group = default_group_of(&engine, &default_vpc_of(&engine));
    let mismatches: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "another description",
            vec![
                param("SubnetId", &subnet_id),
                param("Description", "something else"),
                param("ClientToken", "token-abc"),
            ],
        ),
        (
            "no description at all",
            vec![
                param("SubnetId", &subnet_id),
                param("ClientToken", "token-abc"),
            ],
        ),
        (
            "another subnet",
            vec![
                param("SubnetId", &other_subnet),
                param("Description", "idempotent"),
                param("ClientToken", "token-abc"),
            ],
        ),
        (
            "another interface type",
            vec![
                param("SubnetId", &subnet_id),
                param("Description", "idempotent"),
                param("InterfaceType", "efa"),
                param("ClientToken", "token-abc"),
            ],
        ),
        (
            "a named private address",
            vec![
                param("SubnetId", &subnet_id),
                param("Description", "idempotent"),
                param("PrivateIpAddress", "172.31.0.90"),
                param("ClientToken", "token-abc"),
            ],
        ),
        (
            "more addresses asked for",
            vec![
                param("SubnetId", &subnet_id),
                param("Description", "idempotent"),
                param("SecondaryPrivateIpAddressCount", "2"),
                param("ClientToken", "token-abc"),
            ],
        ),
        (
            // The first call named no group, so it got the VPC's default one; naming that
            // same group explicitly asks for the same set, which is NOT a mismatch — the
            // row that moves is a tag, added where the first call carried none.
            "a tag the first call did not carry",
            vec![
                param("SubnetId", &subnet_id),
                param("Description", "idempotent"),
                param("TagSpecification.1.ResourceType", "network-interface"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "web"),
                param("ClientToken", "token-abc"),
            ],
        ),
    ];
    for (case, mismatched) in mismatches {
        let (status, body) = create(&router, &mismatched).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "IdempotentParameterMismatch",
            "case {case:?}: expected error code \"IdempotentParameterMismatch\", got {:?} \
             from {body}",
            unescaped(&code)
        );
        let after = described_ids(&router, case, &[]).await;
        assert_eq!(
            after,
            vec![first_id.clone()],
            "case {case:?}: a refused replay must store nothing, got {after:?}"
        );
    }

    // Naming the group the first call was given by default asks for the same set, so it is
    // the same request and answers with the same interface.
    let same_request_spelled_out = vec![
        param("SubnetId", &subnet_id),
        param("Description", "idempotent"),
        param("SecurityGroupId.1", &default_group),
        param("ClientToken", "token-abc"),
    ];
    let (status, body) = create(&router, &same_request_spelled_out).await;
    assert_eq!(
        status, 200,
        "the default group named explicitly: expected HTTP 200, got {status} with body {body}"
    );
    let replayed_id = element(&body, "networkInterfaceId").unwrap_or_default();
    assert_eq!(
        replayed_id, first_id,
        "the default group named explicitly: expected the interface the first call created, \
         got {replayed_id:?} after {first_id:?}"
    );

    // A token names an interface only for as long as that interface exists: once a delete
    // takes it out of the store the token names nothing, so the same request is a FRESH
    // create rather than a replay that resolves to a record the store no longer holds.
    let (deleted_status, deleted_body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteNetworkInterface",
        &[param("NetworkInterfaceId", &first_id)],
    )
    .await;
    assert_eq!(
        deleted_status, 200,
        "deleting the token's interface: expected HTTP 200, got {deleted_status} with body \
         {deleted_body}"
    );
    let (after_delete_status, after_delete_body) = create(&router, &params).await;
    assert_eq!(
        after_delete_status, 200,
        "the token after its interface was deleted: expected HTTP 200, got \
         {after_delete_status} with body {after_delete_body}"
    );
    let fresh_id = element(&after_delete_body, "networkInterfaceId").unwrap_or_default();
    assert_ne!(
        fresh_id, first_id,
        "the token after its interface was deleted: expected a new interface, got the \
         deleted {first_id:?}"
    );
    let after_delete = described_ids(&router, "after the token's interface was deleted", &[]).await;
    assert_eq!(
        after_delete,
        vec![fresh_id.clone()],
        "after the token's interface was deleted: expected only the interface the fresh \
         create made, got {after_delete:?}"
    );
}

/// One row of the refusal table: the case name, the request's extra form fields, and the
/// HTTP status and error code that request earns.
type RefusalCase = (&'static str, Vec<(String, String)>, u16, &'static str);

/// Every way a create can be refused, each with the error code the real service answers:
/// a resource that does not exist, an address outside the subnet or already held, members
/// that cannot hold together, and the members this slice holds no state for (501).
#[tokio::test]
async fn a_create_is_refused_with_the_code_the_request_earns() {
    let (router, engine) = super::ec2_query::emulator();
    let subnets = subnet_ids(&engine);
    let subnet_id = subnets[0].clone();
    let default_vpc = default_vpc_of(&engine);

    // A second VPC, so a group of another VPC is a group that cannot be used here.
    let (other_vpc, other_group) = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let vpc = guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
        let group = guard.insert_default_security_group(&vpc, REGION);
        (vpc, group)
    };
    assert_ne!(
        other_vpc, default_vpc,
        "the fixture VPC must not be the default one"
    );

    // An address the store already holds, taken by an interface created here.
    let (status, body) = create(
        &router,
        &[
            param("SubnetId", &subnet_id),
            param("PrivateIpAddress", "172.31.0.9"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the in-use address: expected HTTP 200, got {status} with body {body}"
    );

    let cases: Vec<RefusalCase> = vec![
        (
            "a subnet that does not exist",
            vec![param("SubnetId", "subnet-00000000000000fff")],
            400,
            "InvalidSubnetID.NotFound",
        ),
        (
            "an address outside the subnet's range",
            vec![
                param("SubnetId", &subnet_id),
                param("PrivateIpAddress", "10.9.9.9"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an address AWS reserves in every subnet",
            vec![
                param("SubnetId", &subnet_id),
                param("PrivateIpAddress", "172.31.0.1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an address that is not an address",
            vec![
                param("SubnetId", &subnet_id),
                param("PrivateIpAddress", "not-an-address"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an address another interface holds",
            vec![
                param("SubnetId", &subnet_id),
                param("PrivateIpAddress", "172.31.0.9"),
            ],
            400,
            "InvalidIPAddress.InUse",
        ),
        (
            "a security group that does not exist",
            vec![
                param("SubnetId", &subnet_id),
                param("SecurityGroupId.1", "sg-00000000000000fff"),
            ],
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "a security group of another VPC",
            vec![
                param("SubnetId", &subnet_id),
                param("SecurityGroupId.1", &other_group),
            ],
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "an efa-only interface with an address",
            vec![
                param("SubnetId", &subnet_id),
                param("InterfaceType", "efa-only"),
                param("PrivateIpAddress", "172.31.0.60"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a secondary count beside two named addresses",
            vec![
                param("SubnetId", &subnet_id),
                param("SecondaryPrivateIpAddressCount", "1"),
                param("PrivateIpAddresses.1.PrivateIpAddress", "172.31.0.61"),
                param("PrivateIpAddresses.2.PrivateIpAddress", "172.31.0.62"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "two addresses designated primary",
            vec![
                param("SubnetId", &subnet_id),
                param("PrivateIpAddresses.1.PrivateIpAddress", "172.31.0.63"),
                param("PrivateIpAddresses.1.Primary", "true"),
                param("PrivateIpAddresses.2.PrivateIpAddress", "172.31.0.64"),
                param("PrivateIpAddresses.2.Primary", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a primary address beside an explicit primary",
            vec![
                param("SubnetId", &subnet_id),
                param("PrivateIpAddress", "172.31.0.65"),
                param("PrivateIpAddresses.1.PrivateIpAddress", "172.31.0.66"),
                param("PrivateIpAddresses.1.Primary", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            // One past the per-interface address ceiling the crate publishes
            // (`state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE`): refused on the list's size
            // alone, before any address in it is resolved.
            "one address more than an interface may hold",
            {
                let mut params = vec![param("SubnetId", &subnet_id)];
                for index in 0..=MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE {
                    params.push(param(
                        &format!("PrivateIpAddresses.{}.PrivateIpAddress", index + 1),
                        &format!("172.31.1.{}", index + 1),
                    ));
                }
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming another resource type",
            vec![
                param("SubnetId", &subnet_id),
                param("TagSpecification.1.ResourceType", "subnet"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a reserved tag key",
            vec![
                param("SubnetId", &subnet_id),
                param("TagSpecification.1.ResourceType", "network-interface"),
                param("TagSpecification.1.Tag.1.Key", "aws:created-by"),
                param("TagSpecification.1.Tag.1.Value", "me"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // "Amazon EC2 automatically selects the IPv6 addresses from the subnet range"
            // (`Ipv6AddressCount`), and this suite's subnet has no IPv6 range at all — so
            // the request names space that does not exist, which is a refusal about the
            // request and not about what this emulator has written.
            "an IPv6 address count in a subnet with no IPv6 range",
            vec![
                param("SubnetId", &subnet_id),
                param("Ipv6AddressCount", "1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // The primary IPv6 address is one of the subnet's IPv6 addresses, so it is
            // refused for the same missing range.
            "a primary IPv6 address in a subnet with no IPv6 range",
            vec![
                param("SubnetId", &subnet_id),
                param("EnablePrimaryIpv6", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // Every member below names state this slice does not hold, so the honest
            // answer is "nobody has written this yet" and not a refusal that blames the
            // caller.
            "an IPv4 prefix count",
            vec![
                param("SubnetId", &subnet_id),
                param("Ipv4PrefixCount", "1"),
            ],
            501,
            "NotImplemented",
        ),
        (
            "a connection tracking specification",
            vec![
                param("SubnetId", &subnet_id),
                param(
                    "ConnectionTrackingSpecification.TcpEstablishedTimeout",
                    "60",
                ),
            ],
            501,
            "NotImplemented",
        ),
        (
            "a dry run",
            vec![param("SubnetId", &subnet_id), param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];

    let before = described_ids(&router, "before the refusals", &[]).await;
    for (case, params, expected_status, expected_code) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {:?} from {body}",
            unescaped(&code)
        );
    }
    let after = described_ids(&router, "after the refusals", &[]).await;
    assert_eq!(
        after, before,
        "a refused create must store nothing: {before:?} then {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The seeded store the filter and paging tables are written against
// ---------------------------------------------------------------------------

/// What one seeded interface is created with, beyond the ids the service mints itself.
struct InterfaceFixture {
    /// The index into the account's DEFAULT subnets of the subnet it sits in, or `None`
    /// for the subnet this suite created in its own VPC.
    subnet_index: Option<usize>,
    description: &'static str,
    interface_type: &'static str,
    addresses: usize,
    tags: Vec<Tag>,
}

/// The seeded store: the interfaces, and the facts the filter table compares against.
struct Seeded {
    /// Every interface id, in store order.
    ids: Vec<String>,
    /// Every subnet id, in store order: the account's default subnets — one per zone the
    /// Region publishes — then the fixture subnet.
    subnet_ids: Vec<String>,
    /// The VPC the fixture subnet is in.
    fixture_vpc_id: String,
    /// A security group of that VPC.
    fixture_group_id: String,
    /// The attachment seeded on interface 1.
    attachment_id: String,
    /// The instance that attachment names.
    instance_id: String,
    /// The Elastic IP association seeded on interface 2.
    association_id: String,
    /// Its allocation id.
    allocation_id: String,
}

/// The seeded interfaces: they differ along every attribute the documented filters select
/// on — subnet and VPC, zone, description, type, address count, tags, and whether they
/// are attached, associated with an Elastic IP, IPv6-addressed or service-managed.
fn interface_fixtures() -> Vec<InterfaceFixture> {
    vec![
        InterfaceFixture {
            subnet_index: Some(0),
            description: "web",
            interface_type: "interface",
            addresses: 1,
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        },
        InterfaceFixture {
            subnet_index: Some(0),
            description: "batch",
            interface_type: "interface",
            addresses: 2,
            tags: vec![tag("Name", "batch")],
        },
        InterfaceFixture {
            subnet_index: Some(1),
            description: "public",
            interface_type: "interface",
            addresses: 1,
            tags: Vec::new(),
        },
        InterfaceFixture {
            subnet_index: None,
            description: "nat",
            interface_type: "efa",
            addresses: 1,
            tags: vec![tag("Name", "nat")],
        },
        InterfaceFixture {
            subnet_index: None,
            description: "managed",
            interface_type: "interface",
            addresses: 1,
            tags: Vec::new(),
        },
    ]
}

/// Write the seeded interfaces into the engine's store, the way a creating operation
/// would, and report the facts the tables compare against. The attachment, the Elastic IP
/// associations, the IPv6 address and the operator are set here, so every documented
/// filter has something to select on.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let zones = availability_zones_in_region(REGION);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let fixture_vpc_id =
        guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    let fixture_group_id = guard.insert_default_security_group(&fixture_vpc_id, REGION);
    guard.insert_subnet(SubnetSpec {
        vpc_id: fixture_vpc_id.clone(),
        cidr_block: "10.0.0.0/24".to_string(),
        availability_zone: zones[0].zone_name.clone(),
        availability_zone_id: zones[0].zone_id.clone(),
        region: REGION.to_string(),
        state: "available".to_string(),
        default_for_az: false,
        map_public_ip_on_launch: false,
        tags: Vec::new(),
    });
    let subnets: Vec<String> = guard.subnets.keys().cloned().collect();
    let default_group_id = guard
        .security_groups
        .values()
        .find(|group| {
            group.vpc_id.as_deref()
                != Some(fixture_vpc_id.as_str())
        })
        .and_then(|group| group.group_id.clone())
        .expect("the default VPC has its default security group");

    let mut ids = Vec::new();
    for fixture in interface_fixtures() {
        // The fixture subnet follows the account's default subnets — one per zone the
        // Region publishes — so it is the LAST one in store order.
        let subnet_id = match fixture.subnet_index {
            Some(index) => subnets[index].clone(),
            None => subnets
                .last()
                .cloned()
                .expect("the fixture subnet was written above"),
        };
        let subnet = guard
            .subnets
            .get(&subnet_id)
            .expect("a seeded subnet")
            .clone();
        let cidr_block = subnet.cidr_block.clone().unwrap_or_default();
        // The addresses come out of the store's own one scan of the subnet
        // (`mint_private_ip_in_cidr`), which also takes them, so a seeded interface holds
        // addresses nothing else in the account holds — the same way the create does.
        let addresses: Vec<String> = (0..fixture.addresses)
            .map(|index| {
                guard
                    .mint_private_ip_in_cidr(&cidr_block)
                    .unwrap_or_else(|| {
                        panic!("subnet {subnet_id} has a free address for seed {index}")
                    })
            })
            .collect();
        let group_id = if fixture.subnet_index.is_none() {
            fixture_group_id.clone()
        } else {
            default_group_id.clone()
        };
        let group_name = guard
            .security_groups
            .get(&group_id)
            .and_then(|group| group.group_name.clone());
        ids.push(
            guard
                .insert_network_interface(NetworkInterfaceSpec {
                    subnet_id: subnet_id.clone(),
                    vpc_id: subnet.vpc_id.clone().unwrap_or_default(),
                    availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
                    availability_zone_id: subnet
                        .availability_zone_id
                        .clone()
                        .unwrap_or_default(),
                    region: REGION.to_string(),
                    description: fixture.description.to_string(),
                    interface_type: fixture.interface_type.to_string(),
                    private_ip_addresses: addresses,
                    groups: vec![GroupIdentifier {
                        group_id: Some(group_id),
                        group_name,
                    }],
                    tags: fixture.tags,
                    ..NetworkInterfaceSpec::default()
                })
                .expect("the store holds the seeded network interface"),
        );
    }

    // Interface 1 is attached to an instance, so its status is `in-use` and the
    // `attachment.*` filters have something to select on: "if a network interface is
    // attached to an instance the status is in-use" (the `status` filter's own doc).
    let attachment_id = guard.next_id("eni-attach");
    let instance_id = guard.mint_instance_id();
    let owner_id = engine::emulator_account_id().to_string();
    if let Some(interface) = guard.network_interfaces.get_mut(&ids[1]) {
        interface.status = Some("in-use".to_string());
        interface.attachment = Some(NetworkInterfaceAttachment {
            attach_time: Some(::aws_smithy_types::DateTime::from_secs(
                SEEDED_ATTACH_TIME_SECS,
            )),
            attachment_id: Some(attachment_id.clone()),
            delete_on_termination: Some(true),
            device_index: Some(1),
            network_card_index: Some(0),
            instance_id: Some(instance_id.clone()),
            instance_owner_id: Some(owner_id.clone()),
            status: Some("attached".to_string()),
            ena_srd_specification: None,
            ena_queue_count: None,
        });
    }

    // Interface 2 carries an Elastic IP address, both on the interface and on its primary
    // private address, which is where the `association.*` and `addresses.association.*`
    // filters read from.
    let association_id = guard.next_id("eipassoc");
    let allocation_id = guard.next_id("eipalloc");
    if let Some(interface) = guard.network_interfaces.get_mut(&ids[2]) {
        interface.association = Some(NetworkInterfaceAssociation {
            allocation_id: Some(allocation_id.clone()),
            association_id: Some(association_id.clone()),
            ip_owner_id: Some(owner_id.clone()),
            public_dns_name: Some("ec2-198-51-100-7.compute-1.amazonaws.com".to_string()),
            public_ip: Some("198.51.100.7".to_string()),
            customer_owned_ip: None,
            carrier_ip: None,
        });
        if let Some(address) = interface
            .private_ip_addresses
            .as_mut()
            .and_then(|addresses| addresses.first_mut())
        {
            address.association = Some(NetworkInterfaceAssociation {
                allocation_id: Some(allocation_id.clone()),
                association_id: Some(association_id.clone()),
                ip_owner_id: Some(owner_id.clone()),
                public_dns_name: None,
                public_ip: Some("198.51.100.8".to_string()),
                customer_owned_ip: None,
                carrier_ip: None,
            });
        }
    }

    // Interface 3 carries an IPv6 address.
    if let Some(interface) = guard.network_interfaces.get_mut(&ids[3]) {
        interface.ipv6_addresses = Some(vec![NetworkInterfaceIpv6Address {
            ipv6_address: Some("2600:1f14:abc:1200::1".to_string()),
            public_ipv6_dns_name: None,
            is_primary_ipv6: Some(false),
        }]);
    }

    // Interface 4 is one a service provider manages for the account, which is what the
    // `requester-*` and `operator.*` filters select on. It is NOT hidden by default: a
    // hidden interface has a rule of its own, covered by its own test.
    if let Some(interface) = guard.network_interfaces.get_mut(&ids[4]) {
        interface.requester_managed = Some(true);
        interface.requester_id = Some("amazon-elb".to_string());
        interface.operator = Some(OperatorResponse {
            managed: Some(true),
            principal: Some("elasticloadbalancing.amazonaws.com".to_string()),
            hidden_by_default: Some(false),
        });
    }

    Seeded {
        ids,
        subnet_ids: subnets,
        fixture_vpc_id,
        fixture_group_id,
        attachment_id,
        instance_id,
        association_id,
        allocation_id,
    }
}

/// The ids at `indices` of a seeded store, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded.ids[*index].clone()).collect()
}

/// The stored value of `member` on the interface at `index`, read straight out of the
/// store so a filter row compares against what the service itself derived.
fn stored_primary_address(engine: &engine::InMemoryEngine, id: &str, position: usize) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .get(id)
        .and_then(|interface| interface.private_ip_addresses.clone())
        .and_then(|addresses| addresses.get(position).cloned())
        .and_then(|address| address.private_ip_address)
        .unwrap_or_else(|| panic!("interface {id} has an address at position {position}"))
}

/// The stored MAC address of one interface.
fn stored_mac(engine: &engine::InMemoryEngine, id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .get(id)
        .and_then(|interface| interface.mac_address.clone())
        .unwrap_or_else(|| panic!("interface {id} has a MAC address"))
}

/// The stored private DNS name of one interface.
fn stored_private_dns_name(engine: &engine::InMemoryEngine, id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .get(id)
        .and_then(|interface| interface.private_dns_name.clone())
        .unwrap_or_else(|| panic!("interface {id} has a private DNS name"))
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Every filter `DescribeNetworkInterfaces` documents selects the interfaces whose own
/// attribute matches, one table row per documented name — "if you specify multiple values
/// for a filter, the values are joined with an OR" (`Filter$Values`) and "if you specify
/// multiple filters, the filters are joined with an AND" (`Filter`).
#[tokio::test]
async fn every_documented_filter_selects_on_the_stored_interface() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let zones = availability_zones_in_region(REGION);
    let owner_id = engine::emulator_account_id().to_string();
    let attach_time = ::aws_smithy_types::DateTime::from_secs(SEEDED_ATTACH_TIME_SECS)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the seeded attach time renders");
    let primary_of_0 = stored_primary_address(&engine, &seeded.ids[0], 0);
    let secondary_of_1 = stored_primary_address(&engine, &seeded.ids[1], 1);
    let mac_of_0 = stored_mac(&engine, &seeded.ids[0]);
    let dns_of_0 = stored_private_dns_name(&engine, &seeded.ids[0]);

    // (case, filter name, values, the indices of the interfaces it must select)
    let cases: Vec<(&str, &str, Vec<String>, Vec<usize>)> = vec![
        (
            "the id of one interface",
            "network-interface-id",
            vec![seeded.ids[0].clone()],
            vec![0],
        ),
        (
            "two ids at once",
            "network-interface-id",
            vec![seeded.ids[0].clone(), seeded.ids[2].clone()],
            vec![0, 2],
        ),
        (
            "a subnet",
            "subnet-id",
            vec![seeded.subnet_ids[0].clone()],
            vec![0, 1],
        ),
        (
            "a VPC",
            "vpc-id",
            vec![seeded.fixture_vpc_id.clone()],
            vec![3, 4],
        ),
        (
            "an Availability Zone",
            "availability-zone",
            vec![zones[1].zone_name.clone()],
            vec![2],
        ),
        (
            "an Availability Zone id",
            "availability-zone-id",
            vec![zones[0].zone_id.clone()],
            vec![0, 1, 3, 4],
        ),
        (
            "a description",
            "description",
            vec!["nat".to_string()],
            vec![3],
        ),
        (
            "a description with a wildcard",
            "description",
            vec!["ba*".to_string()],
            vec![1],
        ),
        (
            "a group id",
            "group-id",
            vec![seeded.fixture_group_id.clone()],
            vec![3, 4],
        ),
        (
            "an interface type",
            "interface-type",
            vec!["efa".to_string()],
            vec![3],
        ),
        (
            "the other interface type",
            "interface-type",
            vec!["interface".to_string()],
            vec![0, 1, 2, 4],
        ),
        (
            "a MAC address",
            "mac-address",
            vec![mac_of_0.clone()],
            vec![0],
        ),
        (
            "the owning account",
            "owner-id",
            vec![owner_id.clone()],
            vec![0, 1, 2, 3, 4],
        ),
        (
            "a private DNS name",
            "private-dns-name",
            vec![dns_of_0.clone()],
            vec![0],
        ),
        (
            "a primary private address",
            "private-ip-address",
            vec![primary_of_0.clone()],
            vec![0],
        ),
        (
            "a secondary private address",
            "private-ip-address",
            vec![secondary_of_1.clone()],
            vec![1],
        ),
        (
            "an address of the addresses set",
            "addresses.private-ip-address",
            vec![secondary_of_1.clone()],
            vec![1],
        ),
        (
            "an address that is not primary",
            "addresses.primary",
            vec!["false".to_string()],
            vec![1],
        ),
        (
            "the requester of a managed interface",
            "requester-id",
            vec!["amazon-elb".to_string()],
            vec![4],
        ),
        (
            "a managed interface",
            "requester-managed",
            vec!["true".to_string()],
            vec![4],
        ),
        (
            "an unmanaged interface",
            "requester-managed",
            vec!["false".to_string()],
            vec![0, 1, 2, 3],
        ),
        (
            "source/destination checking",
            "source-dest-check",
            vec!["true".to_string()],
            vec![0, 1, 2, 3, 4],
        ),
        (
            "an interface in use",
            "status",
            vec!["in-use".to_string()],
            vec![1],
        ),
        (
            "a tag value",
            "tag:Name",
            vec!["web".to_string()],
            vec![0],
        ),
        (
            "a tag key",
            "tag-key",
            vec!["Owner".to_string()],
            vec![0],
        ),
        (
            "an attachment id",
            "attachment.attachment-id",
            vec![seeded.attachment_id.clone()],
            vec![1],
        ),
        (
            "the instance an interface is attached to",
            "attachment.instance-id",
            vec![seeded.instance_id.clone()],
            vec![1],
        ),
        (
            "the owner of that instance",
            "attachment.instance-owner-id",
            vec![owner_id.clone()],
            vec![1],
        ),
        (
            "a device index",
            "attachment.device-index",
            vec!["1".to_string()],
            vec![1],
        ),
        (
            "an attachment status",
            "attachment.status",
            vec!["attached".to_string()],
            vec![1],
        ),
        (
            "delete on termination",
            "attachment.delete-on-termination",
            vec!["true".to_string()],
            vec![1],
        ),
        (
            "an attach time",
            "attachment.attach-time",
            vec![attach_time.clone()],
            vec![1],
        ),
        (
            "an Elastic IP allocation",
            "association.allocation-id",
            vec![seeded.allocation_id.clone()],
            vec![2],
        ),
        (
            "an Elastic IP association",
            "association.association-id",
            vec![seeded.association_id.clone()],
            vec![2],
        ),
        (
            "the owner of an Elastic IP",
            "association.ip-owner-id",
            vec![owner_id.clone()],
            vec![2],
        ),
        (
            "an Elastic IP address",
            "association.public-ip",
            vec!["198.51.100.7".to_string()],
            vec![2],
        ),
        (
            "a public DNS name",
            "association.public-dns-name",
            vec!["ec2-198-51-100-7.compute-1.amazonaws.com".to_string()],
            vec![2],
        ),
        (
            "the owner of an address's Elastic IP",
            "addresses.association.owner-id",
            vec![owner_id.clone()],
            vec![2],
        ),
        (
            "an address's Elastic IP",
            "addresses.association.public-ip",
            vec!["198.51.100.8".to_string()],
            vec![2],
        ),
        (
            "an IPv6 address",
            "ipv6-addresses.ipv6-address",
            vec!["2600:1f14:abc:1200::1".to_string()],
            vec![3],
        ),
        (
            "a managed resource's operator",
            "operator.managed",
            vec!["true".to_string()],
            vec![4],
        ),
        (
            "that operator's principal",
            "operator.principal",
            vec!["elasticloadbalancing.amazonaws.com".to_string()],
            vec![4],
        ),
        (
            "a value nothing carries",
            "description",
            vec!["nothing-matches-this".to_string()],
            Vec::new(),
        ),
    ];

    for (case, name, values, expected_indices) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?} (filter {name:?} = {values:?}): expected {expected:?}, got {reported:?}"
        );
    }

    // The `status` filter over the interfaces nothing holds, read against the status they
    // REPORT rather than against a value this row names: an interface nothing holds is the one
    // its create left coming up (`pending`) until the clock settles it (`available`), so which
    // of the two it is depends on when this row runs, while the rule — the filter selects
    // exactly the interfaces reporting the value it names — does not.
    // (`network_interface_create_comes_up_pending` is the suite for that move.)
    for index in [0_usize, 2, 3, 4] {
        let id = &seeded.ids[index];
        let case = format!("the status an unheld interface reports (interface {index})");
        let body = describe_one(&router, &case, id).await;
        let status = element(&body, "status")
            .unwrap_or_else(|| panic!("case {case:?}: the interface reports no status: {body}"));
        let selected = described_ids(&router, &case, &filters(&[("status", &[status.as_str()])]))
            .await;
        assert!(
            selected.contains(id),
            "case {case:?}: the status filter {status:?} must select the interface reporting \
             it, got {selected:?}"
        );
        assert!(
            !selected.contains(&seeded.ids[1]),
            "case {case:?}: the status filter {status:?} must not select the interface an \
             attachment holds, got {selected:?}"
        );
    }

    // "If you specify multiple filters, the filters are joined with an AND": the two
    // together select only what both select, which is less than either alone.
    let both = described_ids(
        &router,
        "two filters",
        &filters(&[
            ("subnet-id", &[seeded.subnet_ids[0].as_str()]),
            ("description", &["web"]),
        ]),
    )
    .await;
    assert_eq!(
        both,
        ids_at(&seeded, &[0]),
        "two filters: expected only the interface both select, got {both:?}"
    );

    // A filter name this operation does not document is refused, whether or not anything
    // would have matched it, so it can never read as "no results".
    for name in ["bogus-filter", "Subnet-Id", "tag:", "attachment.bogus"] {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "filter {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "filter {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "filter {name:?}: expected the message to name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Ids, pagination, dry run and managed resources
// ---------------------------------------------------------------------------

/// `NetworkInterfaceIds` narrows the report to the ids named, and an id naming no
/// interface of the account is an error rather than an absent row.
#[tokio::test]
async fn named_ids_select_and_an_unknown_id_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (case, indices) in [
        ("one id", vec![0]),
        ("two ids", vec![1, 3]),
        ("every id", vec![0, 1, 2, 3, 4]),
        // Repeats of one id name one interface, so the report carries it once, and the
        // order the ids are named in does not change the report's own id order.
        ("one id twice", vec![2, 2]),
        ("two ids, out of order, one repeated", vec![3, 1, 3]),
    ] {
        let wanted = ids_at(&seeded, &indices);
        let borrowed: Vec<&str> = wanted.iter().map(String::as_str).collect();
        let reported = described_ids(&router, case, &indexed("NetworkInterfaceId", &borrowed)).await;
        let mut expected = wanted.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    let (status, body) = describe(
        &router,
        &indexed("NetworkInterfaceId", &["eni-00000000000000fff"]),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkInterfaceID.NotFound",
        "an unknown id: expected InvalidNetworkInterfaceID.NotFound, got {body}"
    );

    // A dry run reports the outcome and reads nothing: "If you have the required
    // permissions, the error response is DryRunOperation" (`DryRun`).
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run: expected DryRunOperation, got {body}"
    );
}

/// The report is cut into `MaxResults`-sized pages in interface-id order, and the
/// `NextToken` of one page is where the next one starts: "This value is null when there
/// are no more items to return" (`DescribeNetworkInterfacesResult$NextToken`).
#[tokio::test]
async fn pagination_walks_the_store_in_id_order() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let subnet_id = subnet_ids(&engine)[0].clone();

    // The model's `DescribeNetworkInterfacesMaxResults` is `range 5..1000`, so 5 is the
    // SMALLEST page a request may ask for — a store of 5 could never page at all. Interfaces
    // are added past the fixtures until it holds 13, which is 5 + 5 + 3: a page size of 5
    // then mints a token TWICE and ends on a partial page.
    const STORED: usize = 13;
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let subnet = guard
            .subnets
            .get(&subnet_id)
            .cloned()
            .expect("a seeded subnet");
        let cidr_block = subnet.cidr_block.clone().unwrap_or_default();
        for index in 0..STORED.saturating_sub(seeded.ids.len()) {
            let address = guard
                .mint_private_ip_in_cidr(&cidr_block)
                .unwrap_or_else(|| panic!("subnet {subnet_id} has a free address for {index}"));
            guard
                .insert_network_interface(NetworkInterfaceSpec {
                    subnet_id: subnet_id.clone(),
                    vpc_id: subnet.vpc_id.clone().unwrap_or_default(),
                    availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
                    availability_zone_id: subnet
                        .availability_zone_id
                        .clone()
                        .unwrap_or_default(),
                    region: REGION.to_string(),
                    description: format!("one more interface to page over ({index})"),
                    interface_type: "interface".to_string(),
                    private_ip_addresses: vec![address],
                    ..NetworkInterfaceSpec::default()
                })
                .unwrap_or_else(|| panic!("the store holds interface {index}"));
        }
    }
    // What the whole report is, in the order the store holds it: the describe pages over
    // interface ids, and the store is keyed by them.
    let expected: Vec<String> = engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        expected.len(),
        STORED,
        "the store must hold more interfaces than the smallest legal page, holds {}",
        expected.len()
    );

    // Rows: the smallest page the model admits, one above it, a size that does not divide
    // the store evenly, and one larger than the store holds — so the walk ends both on a
    // full page followed by a token and on a partial page carrying none.
    for page_size in [5usize, 6, 7, 1000] {
        let case = format!("page size {page_size}");
        let expected_pages = expected.len().div_ceil(page_size);
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        let mut tokens = 0usize;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.clone() {
                params.push(param("NextToken", &next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "networkInterfaceId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} interfaces in a page, got {page:?}"
            );
            pages += 1;
            token = element(&body, "nextToken");
            if token.is_some() {
                tokens += 1;
                // "This value is null when there are no more items to return": a page that
                // carries a token is a FULL page, so a caller paging on it always advances.
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case:?}: a page carrying a token must be full, got {page:?}"
                );
            }
            walked.extend(page);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages over {} interfaces, walked {pages}",
            expected.len()
        );
        assert_eq!(
            tokens,
            expected_pages - 1,
            "case {case:?}: expected a token on every page but the last, got {tokens}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the whole store in id order exactly once, got {walked:?}"
        );
    }

    // A token names a POSITION in that order, not a page size: handed back with the largest
    // page, it reports exactly the interfaces that follow the page it came from — which is
    // the cursor being minted, read back and resumed from rather than a page count.
    let (status, first) = describe(&router, &[param("MaxResults", "5")]).await;
    assert_eq!(status, 200, "the first page: got {status} with body {first}");
    let token = element(&first, "nextToken")
        .unwrap_or_else(|| panic!("5 of {STORED} interfaces mints a token, got {first}"));
    assert_eq!(
        elements(&first, "networkInterfaceId"),
        expected[..5].to_vec(),
        "the first page: expected the first five interfaces, got {first}"
    );
    let (status, rest) = describe(
        &router,
        &[param("MaxResults", "1000"), param("NextToken", &token)],
    )
    .await;
    assert_eq!(status, 200, "the token's page: got {status} with body {rest}");
    assert_eq!(
        elements(&rest, "networkInterfaceId"),
        expected[5..].to_vec(),
        "the token's page: expected every interface after the first five, got {rest}"
    );
    assert!(
        element(&rest, "nextToken").is_none(),
        "the token's page: the last page carries no token, got {rest}"
    );

    // A token this service did not mint is refused rather than read as a cursor.
    for token in ["not-a-token", "6e6f70652d6e6f742d612d746f6b656e"] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "token {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "token {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // "You cannot specify this parameter and the network interface IDs parameter in the
    // same request" (`MaxResults`).
    let mut params = vec![param("MaxResults", "5")];
    params.extend(indexed("NetworkInterfaceId", &[seeded.ids[0].as_str()]));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "MaxResults with an id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombination",
        "MaxResults with an id: expected InvalidParameterCombination, got {body}"
    );
}

/// An interface the account's managed resource visibility settings hide — one whose
/// `Operator.HiddenByDefault` is `true` — is left out of the report unless the request
/// asks for it: "if this parameter is set to true, the output includes resources that are
/// managed by Amazon Web Services services, even if managed resource visibility is set to
/// hidden" (`IncludeManagedResources`). An interface the request names by id is reported
/// either way.
#[tokio::test]
async fn a_hidden_managed_interface_is_reported_only_when_the_request_asks_for_it() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = subnet_ids(&engine)[0].clone();

    let (visible_id, hidden_id) = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let subnet = guard.subnets.get(&subnet_id).cloned().expect("a subnet");
        let spec = |addresses: Vec<String>, description: &str| NetworkInterfaceSpec {
            subnet_id: subnet_id.clone(),
            vpc_id: subnet.vpc_id.clone().unwrap_or_default(),
            availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
            availability_zone_id: subnet.availability_zone_id.clone().unwrap_or_default(),
            region: REGION.to_string(),
            description: description.to_string(),
            interface_type: "interface".to_string(),
            private_ip_addresses: addresses,
            groups: Vec::new(),
            tags: Vec::new(),
            ..NetworkInterfaceSpec::default()
        };
        let visible = guard
            .insert_network_interface(spec(vec!["172.31.0.10".to_string()], "mine"))
            .expect("the store holds the visible interface");
        let hidden = guard
            .insert_network_interface(spec(vec!["172.31.0.11".to_string()], "theirs"))
            .expect("the store holds the hidden interface");
        if let Some(interface) = guard.network_interfaces.get_mut(&hidden) {
            interface.requester_managed = Some(true);
            interface.operator = Some(OperatorResponse {
                managed: Some(true),
                principal: Some("lambda.amazonaws.com".to_string()),
                hidden_by_default: Some(true),
            });
        }
        (visible, hidden)
    };

    for (case, params, expected) in [
        ("by default", Vec::new(), vec![visible_id.clone()]),
        (
            "hidden resources excluded explicitly",
            vec![param("IncludeManagedResources", "false")],
            vec![visible_id.clone()],
        ),
        (
            "hidden resources asked for",
            vec![param("IncludeManagedResources", "true")],
            vec![visible_id.clone(), hidden_id.clone()],
        ),
    ] {
        let mut reported = described_ids(&router, case, &params).await;
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?}, got {reported:?}"
        );
    }

    // Named by id, the hidden interface is reported: the caller asked for that interface.
    let by_id = described_ids(
        &router,
        "the hidden interface by id",
        &indexed("NetworkInterfaceId", &[hidden_id.as_str()]),
    )
    .await;
    assert_eq!(
        by_id,
        vec![hidden_id.clone()],
        "the hidden interface by id: expected it to be reported, got {by_id:?}"
    );
}

// ---------------------------------------------------------------------------
// The ceilings on one request's filtering work
// ---------------------------------------------------------------------------

/// A request AT each published ceiling is answered: the ceilings bound the work, they do
/// not refuse a request that merely reaches them. Each row is a distinct factor of the
/// matching product, named by the constant it is derived from rather than by a literal.
#[tokio::test]
async fn a_request_at_each_filtering_ceiling_is_answered() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `MAX_FILTERS` filters, each selecting everything, so the AND of them all is the
    // whole store: the ceiling itself is accepted and nothing is lost.
    let at_filter_ceiling: Vec<(&str, &[&str])> = (0..MAX_FILTERS)
        .map(|_| ("description", &["*"][..]))
        .collect();
    let reported = described_ids(&router, "MAX_FILTERS filters", &filters(&at_filter_ceiling)).await;
    assert_eq!(
        reported, seeded.ids,
        "case \"MAX_FILTERS filters\": expected every stored interface, got {reported:?}"
    );

    let many_values: Vec<String> = (0..MAX_FILTER_VALUES).map(|n| format!("eni-{n}")).collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);
    let many_ids = vec![seeded.ids[0].as_str(); MAX_RESOURCE_IDS];
    for (case, params) in [
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("description", &many_values)]),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("description", &[long_value.as_str()])]),
        ),
        (
            "MAX_RESOURCE_IDS ids",
            indexed("NetworkInterfaceId", &many_ids),
        ),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at the ceiling, got {status} with body {body}"
        );
    }
}

/// One step past any ceiling is refused with `InvalidParameterValue`, and reports no
/// interface: the request asked for more work than one call will do, and is told so rather
/// than served a partial answer.
#[tokio::test]
async fn a_request_one_step_past_a_filtering_ceiling_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("description", &["*"][..]))
        .collect();
    let past_value_ceiling: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|n| format!("eni-{n}"))
        .collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let past_byte_ceiling = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let past_id_ceiling = vec![seeded.ids[0].as_str(); MAX_RESOURCE_IDS + 1];

    for (case, params) in [
        ("one filter past MAX_FILTERS", filters(&past_filter_ceiling)),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("description", &past_value_ceiling)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("description", &[past_byte_ceiling.as_str()])]),
        ),
        (
            "one id past MAX_RESOURCE_IDS",
            indexed("NetworkInterfaceId", &past_id_ceiling),
        ),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            elements(&body, "networkInterfaceId").is_empty(),
            "case {case:?}: a refused request reports no interface, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// One in-use address pool, not two
// ---------------------------------------------------------------------------

/// A private IPv4 address a network interface holds is OUT of the pool the instance path
/// allocates from, and the reverse holds too: neither path can hand out or accept an
/// address the other already holds, because both read the one in-use set. The rows differ
/// along who chose the address — the request or the service.
#[tokio::test]
async fn an_address_an_interface_holds_is_refused_to_a_later_launch() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = subnet_ids(&engine)[0].clone();
    // One of the AMIs a fresh account can see, so the launch gets as far as the address.
    let image_id = engine
        .state::<Ec2State>()
        .lock()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account sees the published images");

    for (case, create_params) in [
        (
            "an address the request named",
            vec![param("PrivateIpAddress", "172.31.0.77")],
        ),
        ("an address the service chose", Vec::new()),
    ] {
        let mut params = vec![param("SubnetId", &subnet_id)];
        params.extend(create_params);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let held = element(&body, "privateIpAddress").unwrap_or_default();
        assert!(
            !held.is_empty(),
            "case {case:?}: expected the interface to report its private address, got {body}"
        );

        let (launch_status, launch_body) = super::ec2_query::call(
            &router,
            REGION,
            "RunInstances",
            &[
                param("ImageId", &image_id),
                param("MinCount", "1"),
                param("MaxCount", "1"),
                param("PrivateIpAddress", &held),
            ],
        )
        .await;
        assert_eq!(
            launch_status, 400,
            "case {case:?}: expected HTTP 400 for a launch onto {held}, got {launch_status} with \
             body {launch_body}"
        );
        assert_eq!(
            error_code(&launch_body),
            "InvalidIPAddress.InUse",
            "case {case:?}: expected InvalidIPAddress.InUse for {held}, got {launch_body}"
        );
    }

    // And the other direction: an address an INSTANCE holds is refused to an interface,
    // including one the instance path allocated for a launch that named no subnet.
    for (case, launch_params) in [
        (
            "an address the launch named",
            vec![param("PrivateIpAddress", "172.31.0.99")],
        ),
        ("an address the launch was given", Vec::new()),
    ] {
        let mut params = vec![
            param("ImageId", &image_id),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ];
        params.extend(launch_params);
        let (status, body) = super::ec2_query::call(&router, REGION, "RunInstances", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from RunInstances, got {status} with body {body}"
        );
        let held = element(&body, "privateIpAddress").unwrap_or_default();
        assert!(
            !held.is_empty(),
            "case {case:?}: expected the instance to report its private address, got {body}"
        );

        let (create_status, create_body) = create(
            &router,
            &[param("SubnetId", &subnet_id), param("PrivateIpAddress", &held)],
        )
        .await;
        assert_eq!(
            create_status, 400,
            "case {case:?}: expected HTTP 400 for an interface on {held}, got {create_status} \
             with body {create_body}"
        );
        assert_eq!(
            error_code(&create_body),
            "InvalidIPAddress.InUse",
            "case {case:?}: expected InvalidIPAddress.InUse for {held}, got {create_body}"
        );
    }
}
