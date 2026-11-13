// SPDX-License-Identifier: CC0-1.0

//! What a SUCCESSFUL ec2Query body carries: the members the stored record holds, and the
//! envelope's `<return>` success flag — driven over the wire the service actually speaks,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! Two rules, both about a 200 response:
//!
//! 1. A REPORTED RESOURCE IS THE PROJECTION OF THE STORED RECORD: every member the service's
//!    state holds a value for is written on EVERY read that reports that resource, whichever
//!    way the request selected it, and that operation's filters select on those same stored
//!    values. For an Elastic IP address that carries an association, the members an
//!    association writes are its `associationId`, `networkInterfaceId`,
//!    `networkInterfaceOwnerId` and `privateIpAddress`, beside the `instanceId` of the
//!    instance holding the interface — "the ID of the network interface that the address is
//!    associated with, if any" and the rest of `Address`
//!    (`smithy_query(query="shape",shape="Address")`).
//! 2. An operation whose AWS-documented response elements include `return` answers
//!    `<return>true</return>` as a DIRECT child of its envelope, exactly once, BESIDE
//!    whatever its modelled output reports — and one whose modelled output declares the
//!    member itself answers that one and no second, where a read answers no flag at all.
//!
//! Each table row is a distinct input along what its rule generalises over: for rule 1, how
//! the request selects the address (not at all, by allocation id, by public address, by each
//! association filter, by two joined with AND, by a value nothing carries) and which way the
//! association was made (an interface nothing holds, an instance's own interface); for rule
//! 2, which operation answers and what its modelled output reports beside the flag. No row
//! is the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::ec2_query_envelope::RESPONSE_CONTENT_TYPE;
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image the one launch here uses; `RunInstances` only needs one that resolves.
const AMI: &str = "ami-0123456789abcdef0";

/// An association id of the long resource-id form that no call here ever mints, for the row
/// that filters on a value nothing carries.
const UNHELD_ASSOCIATION_ID: &str = "eipassoc-0123456789abcdef0";

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    ec2_query::call(router, REGION, action, params).await
}

/// One call that must answer HTTP 200, as its body.
async fn served(router: &MultiServiceRouter, case: &str, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One Elastic IP address as a read of it reports it: the two members every allocation has,
/// and the ones an association writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AddressRow {
    public_ip: Option<String>,
    association_id: Option<String>,
    instance_id: Option<String>,
    network_interface_id: Option<String>,
    network_interface_owner_id: Option<String>,
    private_ip_address: Option<String>,
}

/// Every address one `DescribeAddresses` body reports, by allocation id.
///
/// Read off the `addressesSet` the operation's output is modelled as
/// (`DescribeAddressesResult$Addresses`, `xmlName` `addressesSet`), so an element nested in
/// some other part of the envelope can never be read as an address's own.
fn reported_addresses(case: &str, body: &str) -> BTreeMap<String, AddressRow> {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let items = set_items(root, "addressesSet");
    items
        .into_iter()
        .map(|item| {
            let allocation_id = child_text(item, "allocationId").unwrap_or_else(|| {
                panic!("case {case:?}: an address with no <allocationId> in {body}")
            });
            let row = AddressRow {
                public_ip: child_text(item, "publicIp"),
                association_id: child_text(item, "associationId"),
                instance_id: child_text(item, "instanceId"),
                network_interface_id: child_text(item, "networkInterfaceId"),
                network_interface_owner_id: child_text(item, "networkInterfaceOwnerId"),
                private_ip_address: child_text(item, "privateIpAddress"),
            };
            (allocation_id, row)
        })
        .collect()
}

/// Allocate one Elastic IP address and answer `(allocation id, public ip)`.
async fn allocate(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let body = served(router, case, "AllocateAddress", &[]).await;
    let allocation_id = ec2_query::element(&body, "allocationId").unwrap_or_else(|| {
        panic!("case {case:?}: AllocateAddress minted no allocation id: {body}")
    });
    let public_ip = ec2_query::element(&body, "publicIp")
        .unwrap_or_else(|| panic!("case {case:?}: AllocateAddress minted no address: {body}"));
    (allocation_id, public_ip)
}

/// A subnet the account really holds: the first one `DescribeSubnets` reports, which is a
/// default subnet of the Region's default VPC.
async fn any_subnet(router: &MultiServiceRouter, case: &str) -> String {
    let body = served(router, case, "DescribeSubnets", &[]).await;
    ec2_query::element(&body, "subnetId")
        .unwrap_or_else(|| panic!("case {case:?}: the account holds no subnet: {body}"))
}

/// Create one network interface in `subnet_id` and answer `(its id, its primary private
/// address)`.
async fn create_interface(
    router: &MultiServiceRouter,
    case: &str,
    subnet_id: &str,
) -> (String, String) {
    let params = [param("SubnetId", subnet_id)];
    let body = served(router, case, "CreateNetworkInterface", &params).await;
    let network_interface_id = ec2_query::element(&body, "networkInterfaceId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateNetworkInterface minted no interface: {body}")
    });
    let parsed = document(case, &body);
    let interface = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "networkInterfaceId").as_deref()
                    == Some(network_interface_id.as_str())
        })
        .unwrap_or_else(|| panic!("case {case:?}: no interface element in {body}"));
    let primary = set_items(interface, "privateIpAddressesSet")
        .into_iter()
        .find(|item| child_text(*item, "primary").as_deref() == Some("true"))
        .and_then(|item| child_text(item, "privateIpAddress"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the interface must hold a primary private address: {body}")
        });
    (network_interface_id, primary)
}

/// Launch one instance and answer `(instance id, its primary private address)`.
async fn launch(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let params = [
        param("ImageId", AMI),
        param("MinCount", "1"),
        param("MaxCount", "1"),
    ];
    let body = served(router, case, "RunInstances", &params).await;
    let instance_id = ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instance: {body}"));
    let private_ip = ec2_query::element(&body, "privateIpAddress").unwrap_or_else(|| {
        panic!("case {case:?}: RunInstances reported no private address: {body}")
    });
    (instance_id, private_ip)
}

/// Associate one address and answer the `associationId` the service minted.
async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
    target: Param,
) -> String {
    let params = [param("AllocationId", allocation_id), target];
    let body = served(router, case, "AssociateAddress", &params).await;
    ec2_query::element(&body, "associationId")
        .unwrap_or_else(|| panic!("case {case:?}: AssociateAddress minted no association: {body}"))
}

/// RULE 1: an address that carries an association reports the members the association wrote
/// on EVERY read that reports it, and the documented association filters select on exactly
/// those stored values.
#[tokio::test]
async fn an_associated_address_reports_its_association_through_every_read() {
    let router = emulator();
    let account_id = engine::emulator_account_id().to_string();
    let setup = "the two associations every row reads";
    let subnet_id = any_subnet(&router, setup).await;

    // Association one: an interface nothing holds, named directly.
    let (free_allocation, free_public_ip) = allocate(&router, setup).await;
    let (free_interface, free_private_ip) = create_interface(&router, setup, &subnet_id).await;
    let free_association = associate(
        &router,
        setup,
        &free_allocation,
        param("NetworkInterfaceId", &free_interface),
    )
    .await;

    // Association two: an instance, which binds the address to the interface its launch
    // created — so the stored record names an instance as well.
    let (held_allocation, held_public_ip) = allocate(&router, setup).await;
    let (instance_id, instance_private_ip) = launch(&router, setup).await;
    let instance_interface =
        ec2_query::primary_network_interface(&router, REGION, setup, &instance_id).await;
    let held_association = associate(
        &router,
        setup,
        &held_allocation,
        param("InstanceId", &instance_id),
    )
    .await;

    // What every read of each address must report, whichever way it was selected.
    let free_row = AddressRow {
        public_ip: Some(free_public_ip.clone()),
        association_id: Some(free_association.clone()),
        instance_id: None,
        network_interface_id: Some(free_interface.clone()),
        network_interface_owner_id: Some(account_id.clone()),
        private_ip_address: Some(free_private_ip.clone()),
    };
    let held_row = AddressRow {
        public_ip: Some(held_public_ip.clone()),
        association_id: Some(held_association.clone()),
        instance_id: Some(instance_id.clone()),
        network_interface_id: Some(instance_interface.clone()),
        network_interface_owner_id: Some(account_id.clone()),
        private_ip_address: Some(instance_private_ip.clone()),
    };
    let expected = |ids: &[&String]| -> BTreeMap<String, AddressRow> {
        ids.iter()
            .map(|id| {
                let row = if *id == &free_allocation {
                    free_row.clone()
                } else {
                    held_row.clone()
                };
                ((*id).clone(), row)
            })
            .collect()
    };

    // Rows differing along HOW the request selects the address: not at all, by either of the
    // two members that name one, by each documented association filter, by two filters
    // joined with AND, and by a filter value nothing carries.
    let rows: Vec<(&str, Vec<Param>, BTreeMap<String, AddressRow>)> = vec![
        (
            "no narrowing at all",
            Vec::new(),
            expected(&[&free_allocation, &held_allocation]),
        ),
        (
            "both allocation ids",
            ec2_query::indexed(
                "AllocationId",
                &[free_allocation.as_str(), held_allocation.as_str()],
            ),
            expected(&[&free_allocation, &held_allocation]),
        ),
        (
            "one allocation id",
            ec2_query::indexed("AllocationId", &[free_allocation.as_str()]),
            expected(&[&free_allocation]),
        ),
        (
            "the public address of the instance's association",
            ec2_query::indexed("PublicIp", &[held_public_ip.as_str()]),
            expected(&[&held_allocation]),
        ),
        (
            "the association-id filter",
            ec2_query::filters(&[("association-id", &[free_association.as_str()])]),
            expected(&[&free_allocation]),
        ),
        (
            "the network-interface-id filter",
            ec2_query::filters(&[("network-interface-id", &[instance_interface.as_str()])]),
            expected(&[&held_allocation]),
        ),
        (
            "the network-interface-owner-id filter, which both associations share",
            ec2_query::filters(&[("network-interface-owner-id", &[account_id.as_str()])]),
            expected(&[&free_allocation, &held_allocation]),
        ),
        (
            "the private-ip-address filter",
            ec2_query::filters(&[("private-ip-address", &[free_private_ip.as_str()])]),
            expected(&[&free_allocation]),
        ),
        (
            "the instance-id filter",
            ec2_query::filters(&[("instance-id", &[instance_id.as_str()])]),
            expected(&[&held_allocation]),
        ),
        (
            "the network-interface-id and private-ip-address filters joined with AND",
            ec2_query::filters(&[
                ("network-interface-id", &[free_interface.as_str()]),
                ("private-ip-address", &[free_private_ip.as_str()]),
            ]),
            expected(&[&free_allocation]),
        ),
        (
            "an association-id filter on an id no address carries",
            ec2_query::filters(&[("association-id", &[UNHELD_ASSOCIATION_ID])]),
            BTreeMap::new(),
        ),
    ];

    for (case, params, want) in &rows {
        let body = served(&router, case, "DescribeAddresses", params).await;
        let reported = reported_addresses(case, &body);
        assert_eq!(
            &reported, want,
            "case {case:?}: DescribeAddresses must report the stored association for every \
             address it selects, got {reported:?} in {body}"
        );
    }
}

/// One row of the envelope table.
struct FlagRow {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
    /// The element the operation's MODELLED output reports, which the flag may not displace.
    reported: &'static str,
    /// How many `<return>` elements the envelope carries: one for a mutation whose documented
    /// response elements include it, and none for a read.
    flags: usize,
}

/// RULE 2: a mutation's envelope carries `<return>true</return>` exactly once beside its
/// modelled output, and a read's carries none.
#[tokio::test]
async fn a_mutation_envelope_carries_one_return_flag_beside_its_modelled_output() {
    let router = emulator();
    let setup = "the resources the envelope rows answer about";

    // The group one row deletes: `DeleteSecurityGroupResult` declares a `Return` member of
    // its own (`xmlName` `return`), so that row pins that the envelope writes no second one.
    let vpc = ec2_query::element(
        &served(&router, setup, "CreateVpc", &[param("CidrBlock", "10.12.0.0/16")]).await,
        "vpcId",
    )
    .unwrap_or_else(|| panic!("case {setup:?}: CreateVpc minted no VPC"));
    let doomed_group = ec2_query::element(
        &served(
            &router,
            setup,
            "CreateSecurityGroup",
            &[
                param("GroupName", "envelope-deleted-group"),
                param("GroupDescription", "the group one row deletes"),
                param("VpcId", &vpc),
            ],
        )
        .await,
        "groupId",
    )
    .unwrap_or_else(|| panic!("case {setup:?}: CreateSecurityGroup minted no group"));
    let subnet_id = any_subnet(&router, setup).await;
    let (allocation_id, _public_ip) = allocate(&router, setup).await;
    let (network_interface_id, _private_ip) = create_interface(&router, setup, &subnet_id).await;

    let rows = vec![
        FlagRow {
            case: "a cluster placement group, which reports the group it minted",
            action: "CreatePlacementGroup",
            params: vec![
                param("GroupName", "envelope-cluster"),
                param("Strategy", "cluster"),
            ],
            reported: "placementGroup",
            flags: 1,
        },
        FlagRow {
            case: "a spread placement group, the same operation on another strategy",
            action: "CreatePlacementGroup",
            params: vec![
                param("GroupName", "envelope-spread"),
                param("Strategy", "spread"),
            ],
            reported: "placementGroup",
            flags: 1,
        },
        FlagRow {
            case: "a security group, which reports the id it minted",
            action: "CreateSecurityGroup",
            params: vec![
                param("GroupName", "envelope-created-group"),
                param("GroupDescription", "the group one row creates"),
                param("VpcId", &vpc),
            ],
            reported: "groupId",
            flags: 1,
        },
        FlagRow {
            case: "an address association, which reports the association it minted",
            action: "AssociateAddress",
            params: vec![
                param("AllocationId", &allocation_id),
                param("NetworkInterfaceId", &network_interface_id),
            ],
            reported: "associationId",
            flags: 1,
        },
        FlagRow {
            case: "a delete whose own modelled output declares the member",
            action: "DeleteSecurityGroup",
            params: vec![param("GroupId", &doomed_group)],
            reported: "return",
            flags: 1,
        },
        FlagRow {
            case: "a read, which reports addresses and no flag",
            action: "DescribeAddresses",
            params: vec![],
            reported: "addressesSet",
            flags: 0,
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, content_type, body) =
            ec2_query::call_with_content_type(&router, REGION, row.action, &row.params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {}, got {status} with body {body}",
            row.action
        );
        assert_eq!(
            content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the media type {RESPONSE_CONTENT_TYPE:?} from {}, got \
             {content_type:?}",
            row.action
        );
        let flags = ec2_query::envelope_children(case, &body, row.action, "return");
        assert_eq!(
            flags.len(),
            row.flags,
            "case {case:?}: expected {} <return> element(s) as a direct child of the {} \
             envelope, got {flags:?} in {body}",
            row.flags,
            row.action
        );
        assert!(
            flags.iter().all(|flag| flag == "true"),
            "case {case:?}: every <return> the {} envelope carries must report the success, \
             got {flags:?} in {body}",
            row.action
        );
        let reported = ec2_query::envelope_children(case, &body, row.action, row.reported);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the flag must not displace the modelled <{}>, got {reported:?} \
             in {body}",
            row.reported
        );
        let request_ids = ec2_query::envelope_children(case, &body, row.action, "requestId");
        assert_eq!(
            request_ids.len(),
            1,
            "case {case:?}: the {} envelope must carry exactly one <requestId>, got \
             {request_ids:?} in {body}",
            row.action
        );
    }
}
