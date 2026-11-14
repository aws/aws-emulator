// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayVpcAttachments`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeTransitGatewayVpcAttachments`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one every EC2 describe follows: it REPORTS THE RESOURCES THE
//! STORE HOLDS, each with every member it carries, narrowed by the id list and by every
//! filter the operation documents (joined with AND), in resource-id order, paged by
//! `MaxResults` with a token naming the id the next page starts at — and it refuses what it
//! cannot resolve rather than answering an absent row: an unknown id with that resource's own
//! `NotFound` code, a value that is not an id of the kind with its `Malformed` code, a filter
//! name the operation documents none of, and a token the service did not mint.
//!
//! Every table row below is a distinct input along what that generalises over: which transit
//! gateway the VPC is attached to, which VPC and which account owns it, which state the
//! attachment is in, how many subnets it names (none, one, two, three), whether it carries an
//! options block and which value each of that block's four members holds, whether it carries
//! tags, which selector the request uses, how many ids it names, and how large a page it asks
//! for (both published bounds of `TransitGatewayMaxResults` and one past each). The
//! attachments are seeded through the store's own writer
//! (`insert_transit_gateway_attachment`), because `CreateTransitGatewayVpcAttachment` is
//! still a scaffold stub — and one of them is then moved by
//! `AcceptTransitGatewayVpcAttachment`, so what this describe reports is also shown to be
//! what another operation wrote.

use ec2::ops::types::{
    APPLIANCE_MODE_SUPPORT_VALUE_VALUES, DNS_SUPPORT_VALUE_VALUES, IPV6_SUPPORT_VALUE_VALUES,
    PeeringTgwInfo, SECURITY_GROUP_REFERENCING_SUPPORT_VALUE_VALUES,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES, Tag, TransitGatewayPeeringAttachment,
    TransitGatewayVpcAttachment, TransitGatewayVpcAttachmentOptions,
};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, TransitGatewayAttachmentView};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, filters, indexed, unescaped};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The ec2Query `Action` under test.
const ACTION: &str = "DescribeTransitGatewayVpcAttachments";

/// An account other than the one this emulator serves — "the ID of the Amazon Web Services
/// account that owns the VPC" (`TransitGatewayVpcAttachment$VpcOwnerId`) for the shared VPCs
/// among the fixtures.
const PEER_ACCOUNT: &str = "210987654321";

/// A third account, so more than one foreign VPC owner is seeded.
const THIRD_ACCOUNT: &str = "111122223333";

/// The value `name` names in the modelled catalog `catalog`, or a panic — so every enum value
/// written into a fixture comes from the model rather than from this file.
///
/// The catalogs are the model's own, re-exported from `ec2::ops::types`
/// (`smithy_query(query="shape",shape="TransitGatewayAttachmentState,DnsSupportValue,
/// Ipv6SupportValue,ApplianceModeSupportValue,SecurityGroupReferencingSupportValue")`).
fn catalog_value(catalog: &'static [&'static str], name: &str) -> &'static str {
    catalog
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no member of the modelled catalog {catalog:?}"))
}

/// A member of `TransitGatewayAttachmentState`, from the modelled catalog.
fn attachment_state(name: &str) -> &'static str {
    catalog_value(TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES, name)
}

/// The four members of `TransitGatewayVpcAttachmentOptions`, each a value of its own modelled
/// catalog.
struct Options {
    /// "Indicates whether DNS support is enabled" — a `DnsSupportValue`.
    dns_support: &'static str,
    /// "Enables you to reference a security group across VPCs attached to a transit gateway" —
    /// a `SecurityGroupReferencingSupportValue`.
    security_group_referencing_support: &'static str,
    /// "Indicates whether IPv6 support is disabled" — an `Ipv6SupportValue`.
    ipv6_support: &'static str,
    /// "Indicates whether appliance mode support is enabled" — an `ApplianceModeSupportValue`.
    appliance_mode_support: &'static str,
}

/// An options block whose four members hold the values named, each checked against its own
/// modelled catalog.
fn options(
    dns_support: &str,
    security_group_referencing_support: &str,
    ipv6_support: &str,
    appliance_mode_support: &str,
) -> Options {
    Options {
        dns_support: catalog_value(DNS_SUPPORT_VALUE_VALUES, dns_support),
        security_group_referencing_support: catalog_value(
            SECURITY_GROUP_REFERENCING_SUPPORT_VALUE_VALUES,
            security_group_referencing_support,
        ),
        ipv6_support: catalog_value(IPV6_SUPPORT_VALUE_VALUES, ipv6_support),
        appliance_mode_support: catalog_value(
            APPLIANCE_MODE_SUPPORT_VALUE_VALUES,
            appliance_mode_support,
        ),
    }
}

/// One seeded VPC attachment: the facts that distinguish it along what the rule generalises
/// over.
struct Fixture {
    /// "The ID of the transit gateway" the VPC is attached to.
    transit_gateway_id: &'static str,
    /// "The ID of the VPC".
    vpc_id: &'static str,
    /// "The ID of the Amazon Web Services account that owns the VPC".
    vpc_owner_id: String,
    /// The attachment's state, a member of `TransitGatewayAttachmentState`.
    state: &'static str,
    /// "The IDs of the subnets" — empty for an attachment that records none.
    subnet_ids: &'static [&'static str],
    /// "The VPC attachment options" — `None` for an attachment carrying no options block.
    options: Option<Options>,
    /// "The creation time", as seconds since the epoch.
    created_at: i64,
    /// "The tags for the VPC attachment", as (key, value) pairs — empty for an untagged one.
    tags: &'static [(&'static str, &'static str)],
}

/// This emulator's own account id, which the VPCs of this account carry as their owner.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
}

/// The seven VPC attachments every test here is written against: three transit gateways with
/// one, two and four attachments on them, VPCs of this account and of two others, seven
/// distinct states out of the modelled catalog with `available` twice (so a state filter
/// selects more than one row), zero, one, two and three subnets, two attachments carrying an
/// options block with different values in each of its four members and five carrying none, and
/// tagged and untagged attachments.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            vpc_id: "vpc-0000000000000000a",
            vpc_owner_id: local_account(),
            state: attachment_state("available"),
            subnet_ids: &["subnet-0000000000000000a", "subnet-0000000000000000b"],
            options: Some(options("enable", "enable", "disable", "disable")),
            created_at: 1_760_000_000,
            tags: &[("Name", "north"), ("Team", "network")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            vpc_id: "vpc-0000000000000000b",
            vpc_owner_id: PEER_ACCOUNT.to_string(),
            state: attachment_state("pendingAcceptance"),
            subnet_ids: &["subnet-0000000000000000c"],
            options: Some(options("disable", "disable", "enable", "enable")),
            created_at: 1_760_000_100,
            tags: &[("Name", "south")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            vpc_id: "vpc-0000000000000000c",
            vpc_owner_id: local_account(),
            state: attachment_state("available"),
            subnet_ids: &[
                "subnet-0000000000000000d",
                "subnet-0000000000000000e",
                "subnet-0000000000000000f",
            ],
            options: None,
            created_at: 1_760_000_200,
            tags: &[],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            vpc_id: "vpc-0000000000000000d",
            vpc_owner_id: local_account(),
            state: attachment_state("deleting"),
            subnet_ids: &[],
            options: None,
            created_at: 1_760_000_300,
            tags: &[("Team", "network")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            vpc_id: "vpc-0000000000000000e",
            vpc_owner_id: THIRD_ACCOUNT.to_string(),
            state: attachment_state("rejected"),
            subnet_ids: &["subnet-00000000000000011"],
            options: None,
            created_at: 1_760_000_400,
            tags: &[],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            vpc_id: "vpc-0000000000000000f",
            vpc_owner_id: PEER_ACCOUNT.to_string(),
            state: attachment_state("failed"),
            subnet_ids: &["subnet-00000000000000012", "subnet-00000000000000013"],
            options: None,
            created_at: 1_760_000_500,
            tags: &[("Name", "north")],
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            vpc_id: "vpc-00000000000000011",
            vpc_owner_id: local_account(),
            state: attachment_state("modifying"),
            subnet_ids: &["subnet-00000000000000014"],
            options: None,
            created_at: 1_760_000_600,
            tags: &[("Name", "east")],
        },
    ]
}

/// The modelled VPC attachment one fixture is stored as.
fn stored_attachment(fixture: &Fixture) -> TransitGatewayVpcAttachment {
    TransitGatewayVpcAttachment {
        // The store mints the id; what a writer puts here is replaced by it.
        transit_gateway_attachment_id: None,
        transit_gateway_id: Some(fixture.transit_gateway_id.to_string()),
        vpc_id: Some(fixture.vpc_id.to_string()),
        vpc_owner_id: Some(fixture.vpc_owner_id.clone()),
        state: Some(fixture.state.to_string()),
        subnet_ids: (!fixture.subnet_ids.is_empty()).then(|| {
            fixture
                .subnet_ids
                .iter()
                .map(|subnet_id| (*subnet_id).to_string())
                .collect()
        }),
        creation_time: Some(::aws_smithy_types::DateTime::from_secs(fixture.created_at)),
        options: fixture
            .options
            .as_ref()
            .map(|block| TransitGatewayVpcAttachmentOptions {
                dns_support: Some(block.dns_support.to_string()),
                security_group_referencing_support: Some(
                    block.security_group_referencing_support.to_string(),
                ),
                ipv6_support: Some(block.ipv6_support.to_string()),
                appliance_mode_support: Some(block.appliance_mode_support.to_string()),
            }),
        tags: (!fixture.tags.is_empty()).then(|| {
            fixture
                .tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect()
        }),
    }
}

/// A peering attachment, seeded beside the VPC attachments so every assertion about what this
/// describe reports is also an assertion that it reports only the VPC attachments of the one
/// id space every kind of attachment shares.
fn other_kind_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
        requester_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-0000000000000000a".to_string()),
            owner_id: Some(local_account()),
            region: Some(REGION.to_string()),
            ..Default::default()
        }),
        accepter_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-00000000000000021".to_string()),
            owner_id: Some(PEER_ACCOUNT.to_string()),
            region: Some("us-west-2".to_string()),
            ..Default::default()
        }),
        state: Some(attachment_state("available").to_string()),
        ..Default::default()
    })
}

/// Seed [`fixtures`] into the store through its own writer and answer the ids it minted, in
/// fixture order, beside the id of the attachment of another kind seeded with them.
fn seed(engine: &engine::InMemoryEngine) -> (Vec<String>, String) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let ids = fixtures()
        .iter()
        .map(|fixture| {
            guard
                .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
                    stored_attachment(fixture),
                ))
                .expect("the store holds the seeded VPC attachment")
        })
        .collect();
    let other_kind = guard
        .insert_transit_gateway_attachment(other_kind_attachment())
        .expect("the store holds the seeded peering attachment");
    (ids, other_kind)
}

/// The emulator with the attachments above seeded: the router to call, the engine, the VPC
/// attachment ids in fixture order, and the id of the attachment of another kind.
fn seeded() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    Vec<String>,
    String,
) {
    let (router, engine) = super::ec2_query::emulator();
    let (ids, other_kind) = seed(&engine);
    (router, engine, ids, other_kind)
}

/// One `DescribeTransitGatewayVpcAttachments` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The same call, requiring HTTP 200, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// The ids of every VPC attachment a response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "transitGatewayAttachmentId")
}

/// `TransitGatewayAttachmentIds.N` form fields for `ids`.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("TransitGatewayAttachmentIds", ids)
}

/// Read something off the one VPC attachment a response carries — the element that has a
/// `transitGatewayAttachmentId` child.
fn with_attachment<T>(
    case: &str,
    body: &str,
    read: impl FnOnce(roxmltree::Node<'_, '_>) -> T,
) -> T {
    let parsed = document(case, body);
    let attachment = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayAttachmentId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no VPC attachment in {body}"));
    read(attachment)
}

/// A member of the one VPC attachment a response carries, or `None` when it carries no such
/// member.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    with_attachment(case, body, |attachment| child_text(attachment, name))
}

/// A member of a nested element (`options/dnsSupport`) of the one VPC attachment a response
/// carries.
fn nested(case: &str, body: &str, outer: &str, inner: &str) -> Option<String> {
    with_attachment(case, body, |attachment| {
        child(attachment, outer).and_then(|node| child_text(node, inner))
    })
}

/// Whether the one VPC attachment a response carries has a `name` child ELEMENT at all.
///
/// The absence check for a member nothing recorded: [`member`] reads an element's TEXT, which
/// a present-but-empty container (`<tagSet/>`) does not carry, so only element absence shows
/// that nothing was reported for it.
fn has_member(case: &str, body: &str, name: &str) -> bool {
    with_attachment(case, body, |attachment| {
        child(attachment, name).is_some()
    })
}

/// The subnet ids of the one VPC attachment a response carries, in document order.
fn reported_subnets(case: &str, body: &str) -> Vec<String> {
    with_attachment(case, body, |attachment| {
        super::wire::set_items(attachment, "subnetIds")
            .into_iter()
            .map(|item| item.text().unwrap_or_default().to_string())
            .collect()
    })
}

/// The (key, value) pairs of the one VPC attachment a response carries.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    with_attachment(case, body, |attachment| {
        super::wire::set_items(attachment, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect()
    })
}

/// Every fact a stored VPC attachment carries reads back: the id the store minted, the transit
/// gateway it is attached to, the VPC and the account that owns it, its state, its subnets, its
/// options block and the instant it was created, with the tags on it. One row per seeded
/// attachment, so no single attachment is the specification — and the rows carrying no subnets,
/// no options block and no tags assert that a member the store holds no value for is reported
/// absent rather than invented.
#[tokio::test]
async fn every_fact_a_stored_vpc_attachment_carries_reads_back() {
    let (router, _engine, ids, _other_kind) = seeded();

    for (fixture, attachment_id) in fixtures().iter().zip(&ids) {
        let case = format!(
            "VPC attachment {attachment_id} of {} on {}",
            fixture.vpc_id, fixture.transit_gateway_id
        );
        let body = describe_ok(&case, &router, &id_params(&[attachment_id.as_str()])).await;

        assert_eq!(
            reported_ids(&body),
            vec![attachment_id.clone()],
            "case {case:?}: expected exactly the attachment named, got {body}"
        );
        for (name, expected) in [
            ("transitGatewayId", fixture.transit_gateway_id.to_string()),
            ("vpcId", fixture.vpc_id.to_string()),
            ("vpcOwnerId", fixture.vpc_owner_id.clone()),
            ("state", fixture.state.to_string()),
        ] {
            let actual = member(&case, &body, name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
            );
        }

        let expected_subnets: Vec<String> = fixture
            .subnet_ids
            .iter()
            .map(|subnet_id| (*subnet_id).to_string())
            .collect();
        if expected_subnets.is_empty() {
            assert!(
                !has_member(&case, &body, "subnetIds"),
                "case {case:?}: an attachment recording no subnets carries no <subnetIds>, got \
                 {body}"
            );
        } else {
            let actual = reported_subnets(&case, &body);
            assert_eq!(
                actual, expected_subnets,
                "case {case:?}: expected the subnets stored, got {actual:?} in {body}"
            );
        }

        match fixture.options.as_ref() {
            None => assert!(
                !has_member(&case, &body, "options"),
                "case {case:?}: an attachment carrying no options block reports no <options>, \
                 got {body}"
            ),
            Some(block) => {
                for (name, expected) in [
                    ("dnsSupport", block.dns_support),
                    (
                        "securityGroupReferencingSupport",
                        block.security_group_referencing_support,
                    ),
                    ("ipv6Support", block.ipv6_support),
                    ("applianceModeSupport", block.appliance_mode_support),
                ] {
                    let actual = nested(&case, &body, "options", name);
                    assert_eq!(
                        actual.as_deref(),
                        Some(expected),
                        "case {case:?}: expected <options>/<{name}> {expected:?}, got \
                         {actual:?} in {body}"
                    );
                }
            }
        }

        let reported_creation = member(&case, &body, "creationTime");
        let seconds = reported_creation.as_deref().and_then(as_timestamp);
        let expected_seconds =
            ::aws_smithy_types::DateTime::from_secs(fixture.created_at).as_secs_f64();
        assert_eq!(
            seconds,
            Some(expected_seconds),
            "case {case:?}: expected the instant the attachment was created, got \
             {reported_creation:?} in {body}"
        );

        let expected_tags: Vec<(String, String)> = fixture
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        if expected_tags.is_empty() {
            assert!(
                !has_member(&case, &body, "tagSet"),
                "case {case:?}: an untagged attachment carries no <tagSet>, got {body}"
            );
        } else {
            let actual = reported_tags(&case, &body);
            assert_eq!(
                actual, expected_tags,
                "case {case:?}: expected the tags stored, got {actual:?} in {body}"
            );
        }
    }
}

/// A request that names no selector reports every VPC attachment the store holds, in
/// `TransitGatewayAttachmentId` order — and no attachment of another kind, though every kind
/// shares the one id space this store is keyed by.
#[tokio::test]
async fn an_unselective_describe_reports_every_stored_vpc_attachment_in_id_order() {
    let (router, _engine, ids, other_kind) = seeded();
    let case = "no selector";

    let body = describe_ok(case, &router, &[]).await;
    let mut expected = ids.clone();
    expected.sort();
    let actual = reported_ids(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?} in id order, got {actual:?} in {body}"
    );
    assert!(
        !actual.contains(&other_kind),
        "case {case:?}: a peering attachment is no VPC attachment, got {actual:?} in {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: a complete answer carries no nextToken, got {body}"
    );
}

/// An empty store is an empty answer, not a refusal: a describe of a resource kind nothing has
/// created reports no VPC attachments at all.
#[tokio::test]
async fn an_empty_store_reports_no_vpc_attachments() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "empty store";

    let body = describe_ok(case, &router, &[]).await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no VPC attachments, got {actual:?} in {body}"
    );
}

/// Each filter selects exactly the attachments carrying its value, and several filters are
/// joined with AND. The rows span every filter name the operation documents, a value nothing
/// carries, a wildcard value, two values in one filter, and two-filter combinations.
#[tokio::test]
async fn each_filter_selects_the_attachments_that_carry_its_value() {
    let (router, _engine, ids, _other_kind) = seeded();
    let rows = fixtures();

    // (case, the filters as sent, the fixture positions expected back)
    let cases: Vec<(&str, Vec<Param>, Vec<usize>)> = vec![
        (
            "state available",
            filters(&[("state", &[attachment_state("available")])]),
            vec![0, 2],
        ),
        (
            "state pendingAcceptance",
            filters(&[("state", &[attachment_state("pendingAcceptance")])]),
            vec![1],
        ),
        (
            "two states in one filter",
            filters(&[(
                "state",
                &[attachment_state("deleting"), attachment_state("rejected")],
            )]),
            vec![3, 4],
        ),
        (
            "state nothing carries",
            filters(&[("state", &[attachment_state("rollingBack")])]),
            Vec::new(),
        ),
        (
            "transit-gateway-id of four attachments",
            filters(&[("transit-gateway-id", &[rows[0].transit_gateway_id])]),
            vec![0, 1, 5, 6],
        ),
        (
            "transit-gateway-id of two attachments",
            filters(&[("transit-gateway-id", &[rows[2].transit_gateway_id])]),
            vec![2, 3],
        ),
        (
            "transit-gateway-id of one attachment",
            filters(&[("transit-gateway-id", &[rows[4].transit_gateway_id])]),
            vec![4],
        ),
        (
            // "Filter values support wildcards" is EC2's filter contract: `*` stands for zero
            // or more characters, so a prefix pattern selects every transit gateway.
            "transit-gateway-id as a wildcard",
            filters(&[("transit-gateway-id", &["tgw-*"])]),
            (0..rows.len()).collect(),
        ),
        (
            "transit-gateway-id nothing carries",
            filters(&[("transit-gateway-id", &["tgw-00000000000000fff"])]),
            Vec::new(),
        ),
        (
            "vpc-id of one attachment",
            filters(&[("vpc-id", &[rows[0].vpc_id])]),
            vec![0],
        ),
        (
            "two vpc-ids in one filter",
            filters(&[("vpc-id", &[rows[2].vpc_id, rows[3].vpc_id])]),
            vec![2, 3],
        ),
        (
            "vpc-id as a wildcard",
            filters(&[("vpc-id", &["vpc-*"])]),
            (0..rows.len()).collect(),
        ),
        (
            "vpc-id nothing carries",
            filters(&[("vpc-id", &["vpc-00000000000000fff"])]),
            Vec::new(),
        ),
        (
            "transit-gateway-id and state joined with AND",
            filters(&[
                ("transit-gateway-id", &[rows[0].transit_gateway_id]),
                ("state", &[attachment_state("available")]),
            ]),
            vec![0],
        ),
        (
            "vpc-id and transit-gateway-id joined with AND",
            filters(&[
                ("vpc-id", &[rows[3].vpc_id]),
                ("transit-gateway-id", &[rows[3].transit_gateway_id]),
            ]),
            vec![3],
        ),
        (
            "a vpc-id and a state that no one attachment carries together",
            filters(&[
                ("vpc-id", &[rows[3].vpc_id]),
                ("state", &[attachment_state("available")]),
            ]),
            Vec::new(),
        ),
    ];

    for (case, params, expected_positions) in cases {
        let body = describe_ok(case, &router, &params).await;
        let mut expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        expected.sort();
        let actual = reported_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }

    // The id filter is the id selector's twin, so it is asserted over every seeded attachment
    // rather than over one row.
    for attachment_id in &ids {
        let case = format!("transit-gateway-attachment-id {attachment_id}");
        let body = describe_ok(
            &case,
            &router,
            &filters(&[("transit-gateway-attachment-id", &[attachment_id.as_str()])]),
        )
        .await;
        let actual = reported_ids(&body);
        assert_eq!(
            actual,
            vec![attachment_id.clone()],
            "case {case:?}: expected exactly that attachment, got {actual:?} in {body}"
        );
    }
}

/// A filter that names the attachment of ANOTHER kind sharing this store selects nothing: this
/// operation reports VPC attachments, so the peering attachment beside them is not one of its
/// rows however it is asked for — not by its own id, and not by the transit gateway it shares
/// with the VPC attachments.
#[tokio::test]
async fn an_attachment_of_another_kind_is_not_selected_by_any_filter() {
    let (router, _engine, ids, other_kind) = seeded();

    let case = format!("transit-gateway-attachment-id {other_kind} of a peering attachment");
    let body = describe_ok(
        &case,
        &router,
        &filters(&[("transit-gateway-attachment-id", &[other_kind.as_str()])]),
    )
    .await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no VPC attachments, got {actual:?} in {body}"
    );

    // The peering attachment is on the same transit gateway as four of the VPC attachments, so
    // a wildcard over that gateway must still report exactly those four.
    let case = "transit-gateway-id shared with a peering attachment";
    let body = describe_ok(
        case,
        &router,
        &filters(&[("transit-gateway-id", &[fixtures()[0].transit_gateway_id])]),
    )
    .await;
    let mut expected: Vec<String> = [0, 1, 5, 6].iter().map(|row| ids[*row].clone()).collect();
    expected.sort();
    let actual = reported_ids(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
    );
}

/// One row of the id-list table: the case, the ids the request names, the filters it carries
/// beside them, and the fixture positions the answer must carry.
type IdListCase<'a> = (&'static str, Vec<&'a str>, Vec<Param>, Vec<usize>);

/// The id list selects exactly the attachments it names, whatever its length and however often
/// an id is repeated, and it narrows what a filter would have selected.
#[tokio::test]
async fn an_id_list_selects_exactly_the_attachments_it_names() {
    let (router, _engine, ids, _other_kind) = seeded();
    let rows = fixtures();

    let cases: Vec<IdListCase<'_>> = vec![
        ("one id", vec![ids[3].as_str()], Vec::new(), vec![3]),
        (
            "three ids, named out of order",
            vec![ids[5].as_str(), ids[0].as_str(), ids[2].as_str()],
            Vec::new(),
            vec![0, 2, 5],
        ),
        (
            "the same id twice",
            vec![ids[1].as_str(), ids[1].as_str()],
            Vec::new(),
            vec![1],
        ),
        (
            "every id",
            ids.iter().map(String::as_str).collect(),
            Vec::new(),
            (0..ids.len()).collect(),
        ),
        (
            "two ids narrowed by a state filter",
            vec![ids[0].as_str(), ids[1].as_str()],
            filters(&[("state", &[attachment_state("pendingAcceptance")])]),
            vec![1],
        ),
        (
            "two ids narrowed by a vpc-id filter",
            vec![ids[2].as_str(), ids[3].as_str()],
            filters(&[("vpc-id", &[rows[2].vpc_id])]),
            vec![2],
        ),
        (
            "two ids narrowed by a transit-gateway-id filter",
            vec![ids[4].as_str(), ids[6].as_str()],
            filters(&[("transit-gateway-id", &[rows[6].transit_gateway_id])]),
            vec![6],
        ),
    ];

    for (case, named, extra, expected_positions) in cases {
        let mut params = id_params(&named);
        params.extend(extra);
        let body = describe_ok(case, &router, &params).await;
        let mut expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        expected.sort();
        let actual = reported_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }
}

/// A well-formed id naming no VPC attachment is refused with the attachment's own `NotFound`
/// code — including the id of an attachment of another kind, which names no VPC attachment —
/// and a value that is not an attachment id at all with its `Malformed` code, so a client can
/// tell the two apart and neither is reported as an absent row. Both are asserted beside an id
/// that DOES name a VPC attachment, so the refusal does not depend on the request naming
/// nothing real.
#[tokio::test]
async fn an_unknown_id_is_refused_not_found_and_a_value_that_is_no_id_malformed() {
    let (router, _engine, ids, other_kind) = seeded();
    let prefix = ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX;

    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "one unknown id",
            vec![format!("{prefix}-00000000000000fff")],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        ),
        (
            "a known id beside an unknown one",
            vec![ids[0].clone(), format!("{prefix}-00000000000000eee")],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        ),
        (
            "the id of an attachment of another kind",
            vec![other_kind.clone()],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        ),
        (
            "a suffix that is not hexadecimal",
            vec![format!("{prefix}-not-hexadecimal")],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "an id of another resource of the same family",
            vec!["tgw-0000000000000000a".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "an id of the VPC the attachment attaches",
            vec!["vpc-0000000000000000a".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "a known id beside a value that is no id",
            vec![ids[0].clone(), "not-an-id".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        ),
    ];

    for (case, named, expected) in cases {
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        let (status, body) = describe(&router, &id_params(&borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The refusal names the first id the CALLER listed that could not be resolved, not
    // whichever of them sorts first: the two rows name the same pair of unknown ids in the two
    // possible orders, so an implementation that walked a sorted set would fail one.
    let lower = format!("{prefix}-00000000000000aaa");
    let higher = format!("{prefix}-00000000000000bbb");
    for (case, first, second) in [
        ("the lower id listed first", &lower, &higher),
        ("the higher id listed first", &higher, &lower),
    ] {
        let (status, body) = describe(&router, &id_params(&[first.as_str(), second.as_str()])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(first.as_str()),
            "case {case:?}: the refusal must name {first:?}, got {message:?}"
        );
    }
}

/// A filter name this operation documents none of is refused, naming the filter — never read as
/// "no results". The rows span a misspelling, a filter of ANOTHER describe of the same family
/// (the peering attachment's own `local-owner-id`, and the tag filters this operation's
/// `Filters` member documents none of), this operation's own names in the wrong case ("Filter
/// names are case-sensitive"), and an empty name.
#[tokio::test]
async fn a_filter_this_operation_does_not_document_is_refused() {
    let (router, _engine, _ids, _other_kind) = seeded();

    for name in [
        "owner-id",
        "resource-type",
        "local-owner-id",
        "remote-owner-id",
        "vpc-owner-id",
        "subnet-id",
        "tag:Name",
        "tag-key",
        "transit-gateway-vpc-attachment-id",
        "State",
        "Vpc-Id",
        "",
    ] {
        let case = format!("filter {name:?}");
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name),
            "case {case:?}: the refusal must name the filter, got {message:?}"
        );
    }
}

/// The ceilings this handler puts on the SHAPE of a request, which the model bounds nowhere
/// (`FilterList`, `Filter$Values` and `TransitGatewayAttachmentIdStringList` carry no
/// `@length`), so the handler is what enforces them. Every row is asserted AT the published
/// ceiling — where the request is answered, not refused for its size — and one past it, where
/// it is refused `InvalidParameterValue`: a ceiling that drifted either way fails here.
///
/// At the id ceiling the ids still name no attachment, so that row is answered with the
/// attachment's own `NotFound` code: proof the SIZE check passed and the request went on to be
/// resolved.
#[tokio::test]
async fn a_request_past_a_published_shape_ceiling_is_refused() {
    let (router, _engine, _ids, _other_kind) = seeded();
    let prefix = ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX;
    // A state no attachment carries, so an in-bounds filter row selects nothing and the
    // assertion is about the ceiling rather than about what matched.
    let unmatched = attachment_state("rollingBack");
    let one_unmatched: [&str; 1] = [unmatched];
    let at_byte_ceiling = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    let past_byte_ceiling = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);

    // (case, the request's fields, the error code expected — `None` where HTTP 200 is)
    let mut cases: Vec<(String, Vec<Param>, Option<&str>)> = Vec::new();
    for (case, count, expected) in [
        ("filters at the ceiling", ec2::state::MAX_FILTERS, None),
        (
            "one filter past the ceiling",
            ec2::state::MAX_FILTERS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let entries: Vec<(&str, &[&str])> =
            (0..count).map(|_| ("state", &one_unmatched[..])).collect();
        cases.push((case.to_string(), filters(&entries), expected));
    }
    for (case, count, expected) in [
        (
            "filter values at the ceiling",
            ec2::state::MAX_FILTER_VALUES,
            None,
        ),
        (
            "one filter value past the ceiling",
            ec2::state::MAX_FILTER_VALUES + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let values: Vec<&str> = (0..count).map(|_| unmatched).collect();
        cases.push((case.to_string(), filters(&[("state", &values)]), expected));
    }
    for (case, value, expected) in [
        ("a filter value at the byte ceiling", &at_byte_ceiling, None),
        (
            "a filter value one byte past the ceiling",
            &past_byte_ceiling,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        cases.push((
            case.to_string(),
            filters(&[("state", &[value.as_str()])]),
            expected,
        ));
    }
    for (case, count, expected) in [
        (
            "ids at the ceiling",
            ec2::state::MAX_RESOURCE_IDS,
            Some(ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND),
        ),
        (
            "one id past the ceiling",
            ec2::state::MAX_RESOURCE_IDS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let named: Vec<String> = (0..count)
            .map(|index| format!("{prefix}-{:017x}", index + 1))
            .collect();
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        cases.push((case.to_string(), id_params(&borrowed), expected));
    }

    for (case, params, expected) in cases {
        let (status, body) = describe(&router, &params).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a request at a ceiling is answered, got {status} with body \
                     {body}"
                );
                let reported = reported_ids(&body);
                assert!(
                    reported.is_empty(),
                    "case {case:?}: no attachment carries {unmatched:?}, got {reported:?} in \
                     {body}"
                );
            }
            Some(code) => {
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
        }
    }
}

/// The answer pages in id order: a page carries at most `MaxResults` attachments and a token
/// naming the id the next page starts at, the token walks the whole set exactly once, and the
/// last page carries none. Asserted at the published floor of `TransitGatewayMaxResults`, at a
/// size between the bounds, and at a size that holds everything.
#[tokio::test]
async fn the_answer_pages_in_id_order_with_a_token_that_advances() {
    let (router, _engine, ids, _other_kind) = seeded();
    let mut all = ids.clone();
    all.sort();

    for page_size in [5_usize, 6, 1000] {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One iteration per page, plus one to catch a token that never clears.
        for _ in 0..=all.len() {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(param("NextToken", cursor));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size}, got {page:?}"
            );
            walked.extend(page.clone());
            token = element(&body, "nextToken");
            match token.as_deref() {
                None => break,
                Some(cursor) => assert!(
                    !cursor.is_empty() && page.len() == page_size,
                    "case {case:?}: a token is minted only for a full page, got {body}"
                ),
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end with no token, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must cover every attachment once, in id order, got \
             {walked:?}"
        );
    }
}

/// The wire error code the boundary validator refuses a request with when a modelled
/// constraint rejects it before the handler runs — as `MaxResults` ABOVE
/// `TransitGatewayMaxResults`' published maximum is.
const BOUNDARY_VALIDATION_ERROR: &str = "ValidationError";

/// EC2's own code for a page size below the smallest the operation serves, which this service
/// answers in place of the boundary validator's generic one (the service's
/// `INVALID_MAX_RESULTS`).
const INVALID_MAX_RESULTS: &str = "InvalidMaxResults";

/// A `MaxResults` outside the range the model publishes (`TransitGatewayMaxResults`,
/// `range 5..1000`) is refused at the boundary, one past each published bound, while each bound
/// itself is served. A page size below the smallest the operation serves carries EC2's own
/// `InvalidMaxResults`; one above the largest keeps the boundary validator's `ValidationError`.
/// Either way the message names the member it rejected — asserted, so a 400 for some other
/// reason could not pass for it.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _engine, _ids, _other_kind) = seeded();

    for (case, page_size, expected_status, expected_code) in [
        (
            "one below the published minimum",
            4,
            400,
            INVALID_MAX_RESULTS,
        ),
        ("the published minimum", 5, 200, ""),
        ("the published maximum", 1000, 200, ""),
        (
            "one above the published maximum",
            1001,
            400,
            BOUNDARY_VALIDATION_ERROR,
        ),
    ] {
        let (status, body) = describe(&router, &[param("MaxResults", &page_size.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert!(
                message.contains("maxResults"),
                "case {case:?}: the refusal must name the member it rejected, got {message:?}"
            );
        }
    }
}

/// A `NextToken` this service did not mint is refused rather than read as an id, whatever else
/// the request names, and the refusal carries EC2's own sentence for an unusable token.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let (router, _engine, ids, _other_kind) = seeded();

    for (case, token) in [
        ("plain text", "not-a-token"),
        ("an id of the kind being paged", ids[0].as_str()),
        ("odd-length hex", "abc"),
    ] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {case:?}: expected EC2's own pagination-token sentence, got {message:?}"
        );
    }
}

/// An authorized dry run reports the permission outcome and describes nothing, whichever
/// selector the request carries — "checks whether you have the required permissions for the
/// action, WITHOUT ACTUALLY MAKING THE REQUEST".
#[tokio::test]
async fn a_dry_run_reports_the_permission_outcome_and_describes_nothing() {
    let (router, _engine, ids, _other_kind) = seeded();

    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("no selector", Vec::new()),
        (
            "an id that names an attachment",
            id_params(&[ids[0].as_str()]),
        ),
        (
            "a filter that selects attachments",
            filters(&[("state", &[attachment_state("available")])]),
        ),
    ];

    for (case, selector) in cases {
        let mut params = selector;
        params.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: a dry run describes nothing, got {body}"
        );
    }
}

/// Describing changes nothing a later caller sees: the same request answers the same
/// attachments twice over, and the store holds exactly the attachments it held before — a read
/// neither mints nor moves a resource.
#[tokio::test]
async fn describing_does_not_change_what_a_later_caller_sees() {
    let (router, engine, ids, _other_kind) = seeded();
    let case = "the same request twice";

    let stored_before = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_attachments
        .len();
    let first = describe_ok(case, &router, &[]).await;
    let second = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(&first),
        reported_ids(&second),
        "case {case:?}: two reads must report the same attachments, got {first} then {second}"
    );
    for name in [
        "state",
        "vpcId",
        "vpcOwnerId",
        "transitGatewayId",
        "creationTime",
        "key",
        "value",
    ] {
        assert_eq!(
            elements(&first, name),
            elements(&second, name),
            "case {case:?}: <{name}> must not change between reads, got {first} then {second}"
        );
    }
    let stored_after = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_attachments
        .len();
    assert_eq!(
        stored_after, stored_before,
        "case {case:?}: the store held {stored_before} attachments, now holds {stored_after}"
    );
    assert_eq!(
        stored_after,
        ids.len() + 1,
        "case {case:?}: the store must still hold every seeded attachment"
    );
}

/// What this describe reports is what another operation WROTE, and the move the clock owes is
/// visible through it: accepting the one attachment seeded in `pendingAcceptance` puts it in
/// the transient state of the acceptance, which the describe reports straight away, and once
/// the span has passed the describe reports the terminal `available` — for that attachment
/// alone, every other row keeping the state it was stored in.
#[tokio::test]
async fn an_acceptance_is_visible_through_this_describe_and_so_is_the_move_it_owes() {
    let (router, _engine, ids, _other_kind) = seeded();
    let rows = fixtures();
    // The one fixture seeded waiting for the transit gateway owner's decision.
    let waiting = rows
        .iter()
        .position(|row| row.state == attachment_state("pendingAcceptance"))
        .expect("one fixture waits in pendingAcceptance");
    let attachment_id = ids[waiting].clone();
    let case = format!("accepting {attachment_id}");

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "AcceptTransitGatewayVpcAttachment",
        &[param("TransitGatewayAttachmentId", &attachment_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the acceptance, got {status} with body {body}"
    );

    // The transient state of the acceptance: the span is never zero, so a caller reading
    // straight after the acceptance still sees it.
    let body = describe_ok(&case, &router, &id_params(&[attachment_id.as_str()])).await;
    let actual = member(&case, &body, "state");
    assert_eq!(
        actual.as_deref(),
        Some(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING),
        "case {case:?}: expected the transient state of an acceptance, got {actual:?} in {body}"
    );

    tokio::time::sleep(ec2::state::TRANSITION_DELAY + std::time::Duration::from_millis(250)).await;

    let body = describe_ok(&case, &router, &id_params(&[attachment_id.as_str()])).await;
    let actual = member(&case, &body, "state");
    assert_eq!(
        actual.as_deref(),
        Some(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE),
        "case {case:?}: expected the terminal state the clock owed, got {actual:?} in {body}"
    );

    // Only the accepted attachment moved: every other row still reports the state it was
    // stored in.
    for (position, row) in rows.iter().enumerate() {
        if position == waiting {
            continue;
        }
        let other = format!("{case}, unaffected row {position}");
        let body = describe_ok(&other, &router, &id_params(&[ids[position].as_str()])).await;
        let actual = member(&other, &body, "state");
        assert_eq!(
            actual.as_deref(),
            Some(row.state),
            "case {other:?}: expected the state it was stored in, got {actual:?} in {body}"
        );
    }
}
