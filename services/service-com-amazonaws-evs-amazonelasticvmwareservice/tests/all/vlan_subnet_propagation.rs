// SPDX-License-Identifier: CC0-1.0

//! The ten VLAN subnets `CreateEnvironment` provisions are REAL subnets of the
//! caller's VPC, in the account's own Amazon EC2 store.
//!
//! The rule under test, one level above the example: a resource a service PROVISIONS in
//! another service's namespace is created in that other service's own store, so every
//! service reads the same resource, and it is removed from that store when the resource
//! that provisioned it is deleted. For Amazon EVS: "Amazon EVS provisions only the VLAN
//! subnets" when `vcfVersion` is `SELF_DEPLOYED` and provisions them plus VCF
//! otherwise, each inside the VPC the request names, and "environment deletion also
//! deletes the associated Amazon EVS VLAN subnets" (`CreateEnvironment` /
//! `DeleteEnvironment` docs). So, for every VLAN the environment reports:
//!
//! * `ec2 DescribeSubnets` finds `Vlan$subnetId` in the request's `vpcId`, with the
//!   CIDR block the request gave that VLAN, marked `Elastic VMware Service` — the one
//!   value `Subnet$Type` takes for "a subnet used with Amazon Elastic VMware Service
//!   (EVS)" (that member's own doc in EC2's model);
//! * `Vlan$availabilityZone` is the zone of that subnet, which is the zone of the
//!   service access subnet: "Amazon EVS requires that all VPC subnets exist in a single
//!   Availability Zone in a Region where the service is available" (`vpcId` doc);
//! * `DeleteEnvironment` removes exactly those subnets from EC2 and leaves every other
//!   subnet of the VPC alone.
//!
//! Each row of the table below differs along what the rule generalises over — the
//! Region and so the zone, the zone's position in the Region, the VPC's address space,
//! the SIZE of the VLAN blocks across the published /24../28 range, and whether the HCX
//! VLAN is public — because none of those is what makes a VLAN subnet a real subnet.
//! The recorded case (ten /28 blocks of a `10.0.0.0/16` VPC in `us-west-2`) is one row.
//!
//! The network every row deploys into is created through Amazon EC2 ITSELF over the
//! wire (`CreateVpc`, `CreateSubnet`), so what the rows assert is that the two services
//! read and write ONE store.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{call_in_region, PAST_THE_TRANSITION};

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The `Subnet$Type` EC2 reports for a subnet Amazon EVS provisioned: "Possible values
/// are `Elastic VMware Service` or no value" (`Subnet$Type` doc, EC2's model).
const EVS_SUBNET_TYPE: &str = "Elastic VMware Service";

/// The ten `InitialVlans` members, in the order the model declares them — the order the
/// VLANs are reported in, and the order this suite expects CIDR blocks to be handed to.
const VLAN_MEMBERS: [&str; 10] = [
    "vmkManagement",
    "vmManagement",
    "vMotion",
    "vSan",
    "vTep",
    "edgeVTep",
    "nsxUplink",
    "hcx",
    "expansionVlan1",
    "expansionVlan2",
];

/// One emulator: a router carrying Amazon EVS AND Amazon EC2 over ONE shared engine,
/// which is the whole point — the VLAN subnets have to be visible through the service
/// that owns subnets. Shared with the sibling suites that assert the same about another
/// EC2 resource an Amazon EVS operation changes (`eip_ec2_propagation.rs`).
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise behaviour, not signing: every request below is unsigned.
    router.set_request_authentication(false);
    // An environment is created only for an account holding a Premium Support
    // subscription (`tests/all/premium_support_entitlement.rs` owns that rule).
    crate::premium_support_entitlement::grant_premium_support(&engine);
    router.register_service(
        service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(engine.clone()),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body, leaving a CIDR block's `/` as a client's
/// own encoder would.
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Drive one ec2Query action as a client signing for `region`: `POST /` with the
/// parameters in the form body. Answers the HTTP status and the XML text.
///
/// Shared with the sibling suites that have to ask Amazon EC2 what it holds for a
/// resource Amazon EVS created or changed in the account (`eip_ec2_propagation.rs`).
pub(crate) async fn call_ec2(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", form_value(value)));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{region}.amazonaws.com/"))
        .header("host", format!("ec2.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        // The SigV4 credential scope is what routes the request to `ec2` and what names
        // the Region its zones are resolved in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(::bytes::Bytes::from(body))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<name>…</name>` element of an XML body.
pub(crate) fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// Every `<name>…</name>` element of an XML body, in document order.
pub(crate) fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let start = at + open.len();
        let Some(end) = rest[start..].find(&close) else {
            break;
        };
        found.push(rest[start..start + end].to_string());
        rest = &rest[start + end..];
    }
    found
}

/// The text of `<child>` inside the FIRST `<parent>` block of an XML body — an
/// interface's `attachment/deviceIndex`, a subnet's association, and so on: a member whose
/// name is only unambiguous inside the block that carries it.
///
/// Beside [`element`] and [`elements`], so the suites that ask Amazon EC2 what it holds
/// read one nested member one way.
pub(crate) fn nested(xml: &str, parent: &str, child: &str) -> Option<String> {
    let open = format!("<{parent}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{parent}>"))? + start;
    element(&xml[start..end], child)
}

/// One `<item>` of a `DescribeSubnets` `subnetSet`, as the raw XML of that subnet. The
/// lists nested inside a subnet (`tagSet`, the IPv6 association set) are empty for the
/// subnets under test, so splitting on the item tag gives one chunk per subnet. The tag
/// carries the response's XML namespace, so the split is on its opening bytes.
fn subnet_items(xml: &str) -> Vec<&str> {
    xml.split("<item").skip(1).collect()
}

/// Every subnet id EC2 holds in `vpc_id`, read through `DescribeSubnets` with EC2's own
/// `vpc-id` filter.
async fn subnet_ids_in_vpc(
    router: &MultiServiceRouter,
    region: &str,
    vpc_id: &str,
) -> BTreeSet<String> {
    let (status, xml) = call_ec2(
        router,
        region,
        "DescribeSubnets",
        &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeSubnets in {vpc_id}: expected 200, got {status} {xml}"
    );
    elements(&xml, "subnetId").into_iter().collect()
}

/// One row of the table: a network, and the VLAN subnet plan deployed into it.
struct Row {
    /// What the row is, for every assertion message.
    case: &'static str,
    /// The Region the client signs for, and so the Region the zone belongs to.
    region: &'static str,
    /// The zone the service access subnet is created in — the environment's single
    /// Availability Zone.
    zone: &'static str,
    /// The first two octets of the row's address space: the VPC is `<base>.0.0/16`.
    base: &'static str,
    /// The prefix length of each VLAN subnet block, across the published /24../28 range.
    /// A PUBLIC HCX VLAN subnet is the one exception: it "Must have a /28 netmask"
    /// (`InitialVlans$hcx` doc), so [`Row::vlan_prefix_len`] gives that member its /28.
    prefix_len: u8,
    /// The host part the row sets in each requested block's last octet. Non-zero makes
    /// the request's blocks NON-canonical (`10.9.3.5/28`), which EC2 stores in canonical
    /// form ("We modify the specified CIDR block to its canonical form",
    /// `CreateSubnetRequest$CidrBlock`) — so the VLAN and its subnet must still report
    /// ONE block. Kept below the smallest block's size, so the canonical form is the
    /// same block.
    host_part: u8,
    /// Whether the HCX VLAN is provisioned public (which needs a network ACL id).
    public_hcx: bool,
}

impl Row {
    /// The VPC's own IPv4 CIDR block.
    fn vpc_cidr(&self) -> String {
        format!("{}.0.0/16", self.base)
    }

    /// The service access subnet's block: high in the VPC's range, so it overlaps none
    /// of the VLAN blocks below.
    fn service_access_cidr(&self) -> String {
        format!("{}.200.0/24", self.base)
    }

    /// The prefix length of the `index`-th VLAN's block: the row's own, except for an
    /// HCX VLAN the row provisions PUBLIC, which "Must have a /28 netmask"
    /// (`InitialVlans$hcx` doc).
    fn vlan_prefix_len(&self, index: usize) -> u8 {
        if self.public_hcx && VLAN_MEMBERS[index] == "hcx" {
            28
        } else {
            self.prefix_len
        }
    }

    /// The CIDR block the row REQUESTS for the `index`-th VLAN. Each sits in its own
    /// `/24`, so the blocks never overlap whatever prefix length the row uses.
    fn requested_vlan_cidr(&self, index: usize) -> String {
        format!(
            "{}.{index}.{}/{}",
            self.base,
            self.host_part,
            self.vlan_prefix_len(index)
        )
    }

    /// That block as a subnet of the VPC holds it: the canonical form, which is what
    /// both `Vlan$cidr` and the subnet's `CidrBlock` must report.
    fn stored_vlan_cidr(&self, index: usize) -> String {
        format!("{}.{index}.0/{}", self.base, self.vlan_prefix_len(index))
    }

    /// The row's `initialVlans`. A row whose HCX VLAN is public still needs the network
    /// ACL that VLAN uses — "Required when isHcxPublic is set to true"
    /// (`hcxNetworkAclId` doc) — and that id names a network ACL of the caller's own
    /// account, so the test creates one in the VPC through Amazon EC2 itself and sets it
    /// here ([`create_network_acl`]).
    fn initial_vlans(&self) -> Value {
        let mut vlans = json!({});
        for (index, member) in VLAN_MEMBERS.iter().enumerate() {
            vlans[*member] = json!({ "cidr": self.requested_vlan_cidr(index) });
        }
        if self.public_hcx {
            vlans["isHcxPublic"] = json!(true);
        }
        vlans
    }
}

/// The rows: the same rule over Regions, zones, address spaces, block sizes, the
/// canonical form of the requested block, and a public HCX VLAN.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "ten /28 blocks of a 10.0.0.0/16 VPC in us-west-2a",
            region: "us-west-2",
            zone: "us-west-2a",
            base: "10.0",
            prefix_len: 28,
            host_part: 0,
            public_hcx: false,
        },
        Row {
            case: "ten /24 blocks — the published maximum — in the Region's third zone",
            region: "us-west-2",
            zone: "us-west-2c",
            base: "172.31",
            prefix_len: 24,
            host_part: 0,
            public_hcx: false,
        },
        Row {
            case: "/26 blocks in another Region, with a public HCX VLAN on its own /28",
            region: "eu-west-1",
            zone: "eu-west-1b",
            base: "192.168",
            prefix_len: 26,
            host_part: 0,
            public_hcx: true,
        },
        Row {
            case: "ten /28 blocks requested with host bits set, so not in canonical form",
            region: "us-west-2",
            zone: "us-west-2b",
            base: "10.9",
            prefix_len: 28,
            host_part: 5,
            public_hcx: false,
        },
    ]
}

/// Create a network access control list of `vpc_id` through Amazon EC2 itself, and answer
/// the `NetworkAclId` EC2 minted for it.
///
/// `CreateEnvironment` resolves the `initialVlans.hcxNetworkAclId` a request carries in
/// Amazon EC2's own store, in the VPC the environment is deployed into, so a public HCX
/// VLAN names a list the account really holds — minted here the way a caller would mint
/// it. `tests/all/hcx_network_acl_references.rs` owns that rule.
async fn create_network_acl(router: &MultiServiceRouter, row: &Row, vpc_id: &str) -> String {
    let (status, xml) = call_ec2(
        router,
        row.region,
        "CreateNetworkAcl",
        &[("VpcId", vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateNetworkAcl expected 200, got {status} {xml}",
        case = row.case
    );
    element(&xml, "networkAclId")
        .unwrap_or_else(|| panic!("case {case:?}: no networkAclId in {xml}", case = row.case))
}

/// Create the row's network through Amazon EC2 itself, and answer the VPC and service
/// access subnet ids EC2 minted.
async fn create_network(router: &MultiServiceRouter, row: &Row) -> (String, String) {
    let (status, xml) = call_ec2(
        router,
        row.region,
        "CreateVpc",
        &[("CidrBlock", &row.vpc_cidr())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpc expected 200, got {status} {xml}",
        case = row.case
    );
    let vpc_id = element(&xml, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: no vpcId in {xml}", case = row.case));

    let (status, xml) = call_ec2(
        router,
        row.region,
        "CreateSubnet",
        &[
            ("VpcId", &vpc_id),
            ("CidrBlock", &row.service_access_cidr()),
            ("AvailabilityZone", row.zone),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSubnet expected 200, got {status} {xml}",
        case = row.case
    );
    let subnet_id = element(&xml, "subnetId")
        .unwrap_or_else(|| panic!("case {case:?}: no subnetId in {xml}", case = row.case));
    (vpc_id, subnet_id)
}

#[tokio::test]
async fn the_vlan_subnets_an_environment_provisions_exist_in_ec2_and_go_away_with_it() {
    for row in rows() {
        // A fresh emulator per row, so what EC2 holds is exactly what this row made.
        let router = emulator();
        let (vpc_id, service_access_subnet_id) = create_network(&router, &row).await;
        let case = row.case;

        let before = subnet_ids_in_vpc(&router, row.region, &vpc_id).await;
        assert_eq!(
            before,
            BTreeSet::from([service_access_subnet_id.clone()]),
            "case {case:?}: before the create the VPC holds only the service access \
             subnet, got {before:?}"
        );

        let mut initial_vlans = row.initial_vlans();
        if row.public_hcx {
            // A public HCX VLAN uses a network ACL of the account, and the create resolves
            // that id in Amazon EC2's own store: one of the VPC's own, minted by EC2.
            initial_vlans["hcxNetworkAclId"] =
                json!(create_network_acl(&router, &row, &vpc_id).await);
        }

        let (status, created) = call_in_region(
            &router,
            row.region,
            "CreateEnvironment",
            &json!({
                "environmentName": "vlan-subnets",
                "vpcId": vpc_id,
                "serviceAccessSubnetId": service_access_subnet_id,
                "vcfVersion": "SELF_DEPLOYED",
                "termsAccepted": true,
                "initialVlans": initial_vlans,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEnvironment expected 200, got {status} {created}"
        );
        let environment_id = created["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {created}"))
            .to_string();

        // Once the create has settled, the environment is no longer initializing and
        // its VLANs are listable.
        std::thread::sleep(PAST_THE_TRANSITION);
        let (status, listed) = call_in_region(
            &router,
            row.region,
            "ListEnvironmentVlans",
            &json!({ "environmentId": environment_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListEnvironmentVlans expected 200, got {status} {listed}"
        );
        let vlans = listed["environmentVlans"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentVlans in {listed}"))
            .clone();
        assert_eq!(
            vlans.len(),
            VLAN_MEMBERS.len(),
            "case {case:?}: expected one VLAN per InitialVlans member, got {}",
            vlans.len()
        );

        // EVERY VLAN names a subnet EC2 holds, in the VPC the request named, with the
        // block the request gave that VLAN and the environment's single zone.
        let mut provisioned: BTreeSet<String> = BTreeSet::new();
        for (index, vlan) in vlans.iter().enumerate() {
            let function = vlan["functionName"].as_str().unwrap_or("<none>");
            let expected_cidr = row.stored_vlan_cidr(index);
            let subnet_id = vlan["subnetId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {function}: no subnetId in {vlan}")
            });
            assert!(
                provisioned.insert(subnet_id.to_string()),
                "case {case:?}: {function}: subnetId {subnet_id} is used by another VLAN"
            );
            assert_ne!(
                subnet_id, service_access_subnet_id,
                "case {case:?}: {function}: a VLAN subnet is its own subnet, not the \
                 service access subnet"
            );
            assert_eq!(
                vlan["availabilityZone"],
                json!(row.zone),
                "case {case:?}: {function}: expected the zone of the environment's \
                 subnets ({zone}), got {got}",
                zone = row.zone,
                got = vlan["availabilityZone"]
            );
            // The VLAN and its subnet report ONE block, whatever form the request
            // asked for it in.
            assert_eq!(
                vlan["cidr"],
                json!(expected_cidr),
                "case {case:?}: {function}: expected the block of the VLAN's subnet \
                 ({expected_cidr}), got {got}",
                got = vlan["cidr"]
            );

            let (status, xml) = call_ec2(
                &router,
                row.region,
                "DescribeSubnets",
                &[("SubnetId.1", subnet_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {function}: ec2 DescribeSubnets must find {subnet_id}, \
                 got {status} {xml}"
            );
            let items = subnet_items(&xml);
            assert_eq!(
                items.len(),
                1,
                "case {case:?}: {function}: expected exactly one subnet for \
                 {subnet_id}, got {} in {xml}",
                items.len()
            );
            let item = items[0];
            for (field, expected) in [
                ("subnetId", subnet_id.to_string()),
                ("vpcId", vpc_id.clone()),
                ("cidrBlock", expected_cidr.clone()),
                ("availabilityZone", row.zone.to_string()),
                ("type", EVS_SUBNET_TYPE.to_string()),
            ] {
                let got = element(item, field);
                assert_eq!(
                    got.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: {function}: expected the subnet's {field} to be \
                     {expected:?}, got {got:?} in {item}"
                );
            }
        }

        let during = subnet_ids_in_vpc(&router, row.region, &vpc_id).await;
        let mut expected_during = provisioned.clone();
        expected_during.insert(service_access_subnet_id.clone());
        assert_eq!(
            during, expected_during,
            "case {case:?}: the VPC must hold the service access subnet and the ten \
             VLAN subnets and nothing else, got {during:?}"
        );

        // "Environment deletion also deletes the associated Amazon EVS VLAN subnets" —
        // and nothing else of the VPC.
        let (status, deleted) = call_in_region(
            &router,
            row.region,
            "DeleteEnvironment",
            &json!({ "environmentId": environment_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEnvironment expected 200, got {status} {deleted}"
        );
        let after = subnet_ids_in_vpc(&router, row.region, &vpc_id).await;
        assert_eq!(
            after,
            BTreeSet::from([service_access_subnet_id.clone()]),
            "case {case:?}: after the delete the VPC holds only the service access \
             subnet again, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_vlan_block_that_cannot_be_a_subnet_of_the_vpc_is_refused_and_nothing_is_created() {
    // A VLAN subnet block "must not overlap with other subnets in the VPC"
    // (`InitialVlans` doc) and is provisioned INSIDE that VPC, so a block that overlaps
    // a subnet the VPC already holds, or that is not a block of the VPC's own address
    // space, cannot become a subnet of it. Each row names the members the refusal must
    // report; the control row is the same plan left alone.
    let row = Row {
        case: "the baseline plan",
        region: "us-west-2",
        zone: "us-west-2b",
        base: "10.7",
        prefix_len: 28,
        host_part: 0,
        public_hcx: false,
    };

    /// One refusal row: the case, the `(member index, CIDR block)` pairs it overrides
    /// in the baseline plan, and the `fieldList` members the refusal must name.
    struct Case {
        case: &'static str,
        overrides: Vec<(usize, &'static str)>,
        expected_members: Vec<&'static str>,
    }

    let cases: Vec<Case> = vec![
        Case {
            case: "every block fits the VPC",
            overrides: vec![],
            expected_members: vec![],
        },
        Case {
            case: "one block overlaps the service access subnet",
            // The service access subnet is 10.7.200.0/24.
            overrides: vec![(3, "10.7.200.16/28")],
            expected_members: vec!["initialVlans.vSan"],
        },
        Case {
            case: "one block is not a block of the VPC's CIDR",
            overrides: vec![(0, "10.8.0.0/28")],
            expected_members: vec!["initialVlans.vmkManagement"],
        },
        Case {
            case: "two blocks are outside the VPC and one overlaps another subnet of it",
            overrides: vec![
                (0, "10.8.0.0/28"),
                (9, "192.0.2.0/28"),
                (7, "10.7.200.32/28"),
            ],
            expected_members: vec![
                "initialVlans.vmkManagement",
                "initialVlans.expansionVlan2",
                "initialVlans.hcx",
            ],
        },
    ];

    for Case {
        case,
        overrides,
        expected_members,
    } in cases
    {
        let router = emulator();
        let (vpc_id, service_access_subnet_id) = create_network(&router, &row).await;
        let mut initial_vlans = row.initial_vlans();
        for (index, cidr) in &overrides {
            initial_vlans[VLAN_MEMBERS[*index]] = json!({ "cidr": cidr });
        }

        let (status, body) = call_in_region(
            &router,
            row.region,
            "CreateEnvironment",
            &json!({
                "vpcId": vpc_id,
                "serviceAccessSubnetId": service_access_subnet_id,
                "vcfVersion": "VCF-5.2.1",
                "termsAccepted": true,
                "initialVlans": initial_vlans,
            }),
        )
        .await;

        if expected_members.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        let names: BTreeSet<&str> = body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"))
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        let expected: BTreeSet<&str> = expected_members.iter().copied().collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected one fieldList entry per unplaceable block \
             ({expected:?}), got {names:?} in {body}"
        );

        // A refused create provisions nothing: the VPC still holds only the subnet the
        // test created in it.
        let held = subnet_ids_in_vpc(&router, row.region, &vpc_id).await;
        assert_eq!(
            held,
            BTreeSet::from([service_access_subnet_id.clone()]),
            "case {case:?}: a refused create must leave the VPC's subnets as they were, \
             got {held:?}"
        );
    }
}
