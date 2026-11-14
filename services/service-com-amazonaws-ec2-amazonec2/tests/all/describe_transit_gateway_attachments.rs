// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayAttachments`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeTransitGatewayAttachments`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that this describe REPORTS WHAT THE STORE HOLDS: every attachment
//! of the account, WHATEVER resource it attaches, as the kind-independent
//! `TransitGatewayAttachment` shape — each member projected from the stored attachment and
//! from the records it points at (the transit gateway's own record names the account that
//! owns the gateway, the policy table store names the association) — narrowed by the ids and
//! the documented filters the request names and paged in `TransitGatewayAttachmentId` order.
//! Each table row is a distinct input along what that generalises over: which KIND of
//! attachment (and so which resource type, resource id and resource owner the report
//! describes), which state, whether the gateway the attachment names is one this store holds,
//! whether the attachment is tagged, which filter, how many values, how many filters at once,
//! a wildcard value, the id selector alone and combined with a filter, and a page size at the
//! published bounds.
//!
//! Every create of an attachment is still a scaffold, so the attachments themselves are
//! seeded straight into the store the way the Connect-attachment and attachment-acceptance
//! suites seed the attachments their operations read; the transit gateways are created over
//! the wire, so the gateway ids the attachments name — and the owner the report reads off
//! those gateway records — are ones that operation really stored. The association comes from
//! `AssociateTransitGatewayPolicyTable`, which IS implemented, so what the report carries for
//! it is what a writer really wrote.

use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, Tag, TransitGatewayClientVpnAttachment, TransitGatewayConnect,
    TransitGatewayConnectOptions, TransitGatewayPeeringAttachment, TransitGatewayPolicyTable,
    TransitGatewayVpcAttachment,
};
use ec2::state::{
    CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE, ComAmazonawsEc2Amazonec2State as Ec2State,
    PROTOCOL_VALUE_GRE, TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
    TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
    TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE, TRANSITION_DELAY, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The id prefix every transit gateway attachment is minted with.
const ATTACHMENT_ID_PREFIX: &str = "tgw-attach";

/// The account that owns the resources attached from outside this account, so
/// `ResourceOwnerId` is a member that differs per attachment rather than one shared value.
const OTHER_ACCOUNT: &str = "210987654321";

/// A transit gateway id this store holds no gateway for: the gateway of an attachment whose
/// owner therefore cannot be read off a gateway record.
const UNKNOWN_GATEWAY: &str = "tgw-0fedcba9876543210";

/// The peer gateway of the seeded peering attachment — the resource that attachment attaches.
const PEER_GATEWAY: &str = "tgw-0abcdef1234567890";

/// The Region the peer gateway is in.
const PEER_REGION: &str = "us-west-2";

/// The instant the first seeded attachment was created at; each later one is a second after
/// the one before, so `creationTime` is a member that differs per attachment and cannot be
/// read back from one shared value.
const BASE_SECS: i64 = 1_760_000_000;

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// `secs` as the ISO 8601 UTC timestamp the wire carries a `DateTime` member as.
fn timestamp(secs: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an ISO 8601 rendering of the seeded creation time")
}

/// One reported `TransitGatewayAttachment`, read out of the XML by the document rather than
/// by string surgery. An absent member is the empty string: the report of an attachment that
/// records nothing for it must carry no element at all.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    attachment_id: String,
    transit_gateway_id: String,
    transit_gateway_owner_id: String,
    resource_owner_id: String,
    resource_type: String,
    resource_id: String,
    state: String,
    creation_time: String,
    association_policy_table_id: String,
    association_route_table_id: String,
    association_state: String,
    tags: Vec<(String, String)>,
}

/// One attachment to seed: what goes into the store, and the report the describe must then
/// answer for it, written out beside it rather than recomputed by the test's own copy of the
/// projection.
struct Spec {
    label: &'static str,
    view: TransitGatewayAttachmentView,
    expected: Reported,
}

/// One seeded attachment: the id the store minted for it, and the report it must be given.
#[derive(Debug, Clone)]
struct Seeded {
    label: &'static str,
    id: String,
    expected: Reported,
}

/// A tag list, as a seeded attachment carries one.
fn tag_list(tags: &[(&str, &str)]) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some((*key).to_string()),
            value: Some((*value).to_string()),
        })
        .collect()
}

/// The same tags as the pairs a report of them is compared against.
fn tag_pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The attachments every test of this suite reads, on the two transit gateways the fixture
/// created over the wire.
///
/// The rows span the four kinds of attachment the store holds — each with the resource type,
/// resource id and resource owner its own shape records — several of the states
/// `smithy_query(query="shape",shape="TransitGatewayAttachmentState")` publishes (including
/// two sharing the `pending` prefix a wildcard spans, and the hyphenated spelling
/// `TransitGatewayAttachmentStatusType` gives a Client VPN attachment), an attachment onto a
/// gateway this store does not hold, attachments with no tags and with one and two, and a
/// creation time per attachment.
fn specs(gateway_a: &str, gateway_b: &str, owner: &str) -> Vec<Spec> {
    vec![
        Spec {
            label: "vpc, gateway A, available, one tag",
            view: TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
                transit_gateway_id: Some(gateway_a.to_string()),
                vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                vpc_owner_id: Some(OTHER_ACCOUNT.to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(BASE_SECS)),
                tags: Some(tag_list(&[("Name", "vpc-a")])),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: gateway_a.to_string(),
                transit_gateway_owner_id: owner.to_string(),
                resource_owner_id: OTHER_ACCOUNT.to_string(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC.to_string(),
                resource_id: "vpc-0123456789abcdef0".to_string(),
                state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string(),
                creation_time: timestamp(BASE_SECS),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                tags: tag_pairs(&[("Name", "vpc-a")]),
            },
        },
        Spec {
            label: "vpc, gateway B, pendingAcceptance, no tags",
            view: TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
                transit_gateway_id: Some(gateway_b.to_string()),
                vpc_id: Some("vpc-0b1c2d3e4f5061728".to_string()),
                vpc_owner_id: Some(owner.to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(BASE_SECS + 1)),
                tags: Some(Vec::new()),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: gateway_b.to_string(),
                transit_gateway_owner_id: owner.to_string(),
                resource_owner_id: owner.to_string(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC.to_string(),
                resource_id: "vpc-0b1c2d3e4f5061728".to_string(),
                state: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string(),
                creation_time: timestamp(BASE_SECS + 1),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                tags: Vec::new(),
            },
        },
        Spec {
            label: "peering, gateway A, available, two tags",
            view: TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
                requester_tgw_info: Some(PeeringTgwInfo {
                    transit_gateway_id: Some(gateway_a.to_string()),
                    owner_id: Some(owner.to_string()),
                    region: Some(REGION.to_string()),
                    ..Default::default()
                }),
                accepter_tgw_info: Some(PeeringTgwInfo {
                    transit_gateway_id: Some(PEER_GATEWAY.to_string()),
                    owner_id: Some(OTHER_ACCOUNT.to_string()),
                    region: Some(PEER_REGION.to_string()),
                    ..Default::default()
                }),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(BASE_SECS + 2)),
                tags: Some(tag_list(&[("Name", "peering"), ("Stage", "test")])),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: gateway_a.to_string(),
                transit_gateway_owner_id: owner.to_string(),
                resource_owner_id: OTHER_ACCOUNT.to_string(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING.to_string(),
                resource_id: PEER_GATEWAY.to_string(),
                state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string(),
                creation_time: timestamp(BASE_SECS + 2),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                tags: tag_pairs(&[("Name", "peering"), ("Stage", "test")]),
            },
        },
        Spec {
            label: "client-vpn, gateway B, pending-acceptance",
            view: TransitGatewayAttachmentView::ClientVpn(TransitGatewayClientVpnAttachment {
                transit_gateway_id: Some(gateway_b.to_string()),
                client_vpn_endpoint_id: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
                client_vpn_owner_id: Some(OTHER_ACCOUNT.to_string()),
                state: Some(CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string()),
                // The one kind whose `CreationTime` is modelled as a STRING; the report
                // carries the same instant as the modelled timestamp.
                creation_time: Some(timestamp(BASE_SECS + 3)),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: gateway_b.to_string(),
                transit_gateway_owner_id: owner.to_string(),
                resource_owner_id: OTHER_ACCOUNT.to_string(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN.to_string(),
                resource_id: "cvpn-endpoint-0123456789abcdef0".to_string(),
                state: CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string(),
                creation_time: timestamp(BASE_SECS + 3),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                // `TransitGatewayClientVpnAttachment` models no tags at all.
                tags: Vec::new(),
            },
        },
        Spec {
            label: "connect, gateway A, pending",
            view: TransitGatewayAttachmentView::Connect(TransitGatewayConnect {
                transit_gateway_id: Some(gateway_a.to_string()),
                transport_transit_gateway_attachment_id: Some(
                    "tgw-attach-0a1b2c3d4e5f60718".to_string(),
                ),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(BASE_SECS + 4)),
                options: Some(TransitGatewayConnectOptions {
                    protocol: Some(PROTOCOL_VALUE_GRE.to_string()),
                }),
                tags: Some(Vec::new()),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: gateway_a.to_string(),
                transit_gateway_owner_id: owner.to_string(),
                // `TransitGatewayConnect` records no owner for the attachment it was made
                // from, so the report carries none rather than an invented one.
                resource_owner_id: String::new(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT.to_string(),
                resource_id: "tgw-attach-0a1b2c3d4e5f60718".to_string(),
                state: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING.to_string(),
                creation_time: timestamp(BASE_SECS + 4),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                tags: Vec::new(),
            },
        },
        Spec {
            label: "connect, gateway B, failed",
            view: TransitGatewayAttachmentView::Connect(TransitGatewayConnect {
                transit_gateway_id: Some(gateway_b.to_string()),
                transport_transit_gateway_attachment_id: Some(
                    "tgw-attach-0b2c3d4e5f6071829".to_string(),
                ),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(BASE_SECS + 5)),
                tags: Some(tag_list(&[("Stage", "test")])),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: gateway_b.to_string(),
                transit_gateway_owner_id: owner.to_string(),
                resource_owner_id: String::new(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT.to_string(),
                resource_id: "tgw-attach-0b2c3d4e5f6071829".to_string(),
                state: TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED.to_string(),
                creation_time: timestamp(BASE_SECS + 5),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                tags: tag_pairs(&[("Stage", "test")]),
            },
        },
        Spec {
            label: "vpc, a gateway this store does not hold, deleting",
            view: TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
                transit_gateway_id: Some(UNKNOWN_GATEWAY.to_string()),
                vpc_id: Some("vpc-0c3d4e5f607182939".to_string()),
                vpc_owner_id: Some(OTHER_ACCOUNT.to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(BASE_SECS + 6)),
                ..Default::default()
            }),
            expected: Reported {
                attachment_id: String::new(),
                transit_gateway_id: UNKNOWN_GATEWAY.to_string(),
                // Nothing records who owns a gateway this store does not hold, so the report
                // carries no owner for it.
                transit_gateway_owner_id: String::new(),
                resource_owner_id: OTHER_ACCOUNT.to_string(),
                resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC.to_string(),
                resource_id: "vpc-0c3d4e5f607182939".to_string(),
                state: TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING.to_string(),
                creation_time: timestamp(BASE_SECS + 6),
                association_policy_table_id: String::new(),
                association_route_table_id: String::new(),
                association_state: String::new(),
                tags: Vec::new(),
            },
        },
    ]
}

/// The emulator, two transit gateways created over the wire, and the attachments [`specs`]
/// describes seeded on them.
struct Fixture {
    router: MultiServiceRouter,
    engine: std::sync::Arc<engine::InMemoryEngine>,
    seeded: Vec<Seeded>,
    gateways: Vec<String>,
}

impl Fixture {
    /// The fixture, with every seeded attachment's minted id resolved.
    async fn new() -> Self {
        let (router, engine) = super::ec2_query::emulator();
        let mut gateways = Vec::new();
        for index in 0..2 {
            let (status, body) = call(&router, REGION, "CreateTransitGateway", &[]).await;
            assert_eq!(
                status, 200,
                "the fixture's transit gateway {index} must be created, got {status} with \
                 body {body}"
            );
            gateways.push(
                element(&body, "transitGatewayId")
                    .unwrap_or_else(|| panic!("no transitGatewayId in {body}")),
            );
        }
        let owner = element_of_gateway(&router, &gateways[0]).await;

        let mut seeded = Vec::new();
        for spec in specs(&gateways[0], &gateways[1], &owner) {
            let id = engine
                .state::<Ec2State>()
                .lock()
                .insert_transit_gateway_attachment(spec.view)
                .unwrap_or_else(|| {
                    panic!("the attachment store has room for the seed {:?}", spec.label)
                });
            let expected = Reported {
                attachment_id: id.clone(),
                ..spec.expected
            };
            seeded.push(Seeded {
                label: spec.label,
                id,
                expected,
            });
        }

        Self {
            router,
            engine,
            seeded,
            gateways,
        }
    }

    /// The seeded attachment `label` names.
    fn seed(&self, label: &str) -> &Seeded {
        self.seeded
            .iter()
            .find(|seed| seed.label == label)
            .unwrap_or_else(|| panic!("no seeded attachment labelled {label:?}"))
    }

    /// The ids of the seeded attachments `labels` names, sorted the way a reported set is.
    fn ids(&self, labels: &[&str]) -> Vec<String> {
        let mut ids: Vec<String> = labels.iter().map(|label| self.seed(label).id.clone()).collect();
        ids.sort();
        ids
    }

    /// Every seeded attachment's id, sorted.
    fn all_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.seeded.iter().map(|seed| seed.id.clone()).collect();
        ids.sort();
        ids
    }

    /// A policy table of the first gateway, seeded the way the acceptance suites seed one
    /// (`CreateTransitGatewayPolicyTable` is still a scaffold), ready to be associated with.
    fn policy_table(&self) -> String {
        self.engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_policy_table(TransitGatewayPolicyTable {
                transit_gateway_id: Some(self.gateways[0].clone()),
                state: Some(TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE.to_string()),
                ..Default::default()
            })
            .expect("the policy table store has room for one table")
    }
}

/// The account `DescribeTransitGateways` reports as the owner of `gateway_id`, which is the
/// account a report of an attachment onto it must name as its `TransitGatewayOwnerId`.
async fn element_of_gateway(router: &MultiServiceRouter, gateway_id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeTransitGateways",
        &indexed("TransitGatewayIds", &[gateway_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture's gateway must be describable, got {status} with body {body}"
    );
    element(&body, "ownerId").unwrap_or_else(|| panic!("no ownerId in {body}"))
}

/// One `DescribeTransitGatewayAttachments` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeTransitGatewayAttachments", params).await
}

/// Every attachment a response reports, in document order.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "transitGatewayAttachments")
    else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let association = child(item, "association");
            Reported {
                attachment_id: child_text(item, "transitGatewayAttachmentId").unwrap_or_default(),
                transit_gateway_id: child_text(item, "transitGatewayId").unwrap_or_default(),
                transit_gateway_owner_id: child_text(item, "transitGatewayOwnerId")
                    .unwrap_or_default(),
                resource_owner_id: child_text(item, "resourceOwnerId").unwrap_or_default(),
                resource_type: child_text(item, "resourceType").unwrap_or_default(),
                resource_id: child_text(item, "resourceId").unwrap_or_default(),
                state: child_text(item, "state").unwrap_or_default(),
                creation_time: child_text(item, "creationTime").unwrap_or_default(),
                association_policy_table_id: association
                    .and_then(|node| child_text(node, "transitGatewayPolicyTableId"))
                    .unwrap_or_default(),
                association_route_table_id: association
                    .and_then(|node| child_text(node, "transitGatewayRouteTableId"))
                    .unwrap_or_default(),
                association_state: association
                    .and_then(|node| child_text(node, "state"))
                    .unwrap_or_default(),
                tags: child(item, "tagSet")
                    .map(|tag_set| {
                        tag_set
                            .children()
                            .filter(|node| node.is_element() && node.tag_name().name() == "item")
                            .map(|tag| {
                                (
                                    child_text(tag, "key").unwrap_or_default(),
                                    child_text(tag, "value").unwrap_or_default(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The ids a response reports, sorted.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let mut ids: Vec<String> = reported(case, body)
        .into_iter()
        .map(|attachment| attachment.attachment_id)
        .collect();
    ids.sort();
    ids
}

/// The report of the attachment `id` names, out of a response that must carry one.
fn report_of(case: &str, body: &str, id: &str) -> Reported {
    reported(case, body)
        .into_iter()
        .find(|report| report.attachment_id == id)
        .unwrap_or_else(|| panic!("case {case:?}: no report of {id:?} in {body}"))
}

/// Every stored attachment is reported, whatever resource it attaches, with every member the
/// store holds for it: the gateway it is on, the account that owns that gateway, the resource
/// it attaches with that resource's type and owner, its state, its creation time and its
/// tags.
#[tokio::test]
async fn the_describe_reports_every_stored_attachment_of_every_kind() {
    let fixture = Fixture::new().await;
    let case = "no selector";

    let (status, body) = describe(&fixture.router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let got = reported_ids(case, &body);
    assert_eq!(
        got,
        fixture.all_ids(),
        "case {case:?}: expected every seeded attachment, got {got:?}"
    );

    for seed in &fixture.seeded {
        let label = seed.label;
        let report = report_of(label, &body, &seed.id);
        assert_eq!(
            report, seed.expected,
            "case {label:?}: expected the stored attachment reported as {expected:?}, \
             got {report:?}",
            expected = seed.expected,
        );
        assert!(
            is_long_id(ATTACHMENT_ID_PREFIX, &seed.id),
            "case {label:?}: the store must mint a long-form attachment id, got {id:?}",
            id = seed.id
        );
    }
}

/// One narrowing of the describe and the attachments it must then report.
struct Narrowing {
    label: &'static str,
    params: Vec<Param>,
    expected: Vec<String>,
}

/// The ids and the documented filters narrow the report: each documented filter on its own, a
/// filter whose values are joined with an OR, two filters joined with an AND, wildcard values,
/// a filter on an attribute an attachment does not carry, the id selector alone, and the id
/// selector together with a filter that excludes one of the ids it named.
#[tokio::test]
async fn the_ids_and_the_filters_narrow_what_is_reported() {
    let fixture = Fixture::new().await;
    let gateway_a = fixture.gateways[0].clone();
    let gateway_b = fixture.gateways[1].clone();
    let owner = fixture
        .seed("vpc, gateway A, available, one tag")
        .expected
        .transit_gateway_owner_id
        .clone();
    let connect_a = fixture.seed("connect, gateway A, pending").id.clone();
    let deleting = fixture
        .seed("vpc, a gateway this store does not hold, deleting")
        .id
        .clone();

    let narrowings = vec![
        Narrowing {
            label: "no selector at all",
            params: Vec::new(),
            expected: fixture.all_ids(),
        },
        Narrowing {
            label: "the resource-type filter",
            params: filters(&[(
                "resource-type",
                &[TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC],
            )]),
            expected: fixture.ids(&[
                "vpc, gateway A, available, one tag",
                "vpc, gateway B, pendingAcceptance, no tags",
                "vpc, a gateway this store does not hold, deleting",
            ]),
        },
        Narrowing {
            label: "the resource-type filter with two values, joined with an OR",
            params: filters(&[(
                "resource-type",
                &[
                    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT,
                    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
                ],
            )]),
            expected: fixture.ids(&[
                "connect, gateway A, pending",
                "connect, gateway B, failed",
                "client-vpn, gateway B, pending-acceptance",
            ]),
        },
        Narrowing {
            label: "the resource-id filter",
            params: filters(&[("resource-id", &["vpc-0123456789abcdef0"])]),
            expected: fixture.ids(&["vpc, gateway A, available, one tag"]),
        },
        Narrowing {
            label: "the resource-id filter with a wildcard spanning the VPCs",
            params: filters(&[("resource-id", &["vpc-*"])]),
            expected: fixture.ids(&[
                "vpc, gateway A, available, one tag",
                "vpc, gateway B, pendingAcceptance, no tags",
                "vpc, a gateway this store does not hold, deleting",
            ]),
        },
        Narrowing {
            label: "the resource-owner-id filter",
            params: filters(&[("resource-owner-id", &[OTHER_ACCOUNT])]),
            expected: fixture.ids(&[
                "vpc, gateway A, available, one tag",
                "peering, gateway A, available, two tags",
                "client-vpn, gateway B, pending-acceptance",
                "vpc, a gateway this store does not hold, deleting",
            ]),
        },
        Narrowing {
            label: "the state filter",
            params: filters(&[("state", &[TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE])]),
            expected: fixture.ids(&[
                "vpc, gateway A, available, one tag",
                "peering, gateway A, available, two tags",
            ]),
        },
        Narrowing {
            label: "the state filter with a wildcard spanning two spellings of pending",
            params: filters(&[("state", &["pending*"])]),
            expected: fixture.ids(&[
                "vpc, gateway B, pendingAcceptance, no tags",
                "client-vpn, gateway B, pending-acceptance",
                "connect, gateway A, pending",
            ]),
        },
        Narrowing {
            label: "the state filter on a state no attachment is in",
            params: filters(&[("state", &["rejected"])]),
            expected: Vec::new(),
        },
        Narrowing {
            label: "the transit-gateway-id filter",
            params: filters(&[("transit-gateway-id", &[gateway_b.as_str()])]),
            expected: fixture.ids(&[
                "vpc, gateway B, pendingAcceptance, no tags",
                "client-vpn, gateway B, pending-acceptance",
                "connect, gateway B, failed",
            ]),
        },
        Narrowing {
            label: "the transit-gateway-owner-id filter",
            params: filters(&[("transit-gateway-owner-id", &[owner.as_str()])]),
            expected: fixture.ids(&[
                "vpc, gateway A, available, one tag",
                "vpc, gateway B, pendingAcceptance, no tags",
                "peering, gateway A, available, two tags",
                "client-vpn, gateway B, pending-acceptance",
                "connect, gateway A, pending",
                "connect, gateway B, failed",
            ]),
        },
        Narrowing {
            label: "the transit-gateway-attachment-id filter",
            params: filters(&[("transit-gateway-attachment-id", &[deleting.as_str()])]),
            expected: vec![deleting.clone()],
        },
        Narrowing {
            label: "the association.state filter, which no unassociated attachment matches",
            params: filters(&[(
                "association.state",
                &[TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING],
            )]),
            expected: Vec::new(),
        },
        Narrowing {
            label: "the association.transit-gateway-route-table-id filter",
            params: filters(&[(
                "association.transit-gateway-route-table-id",
                &["tgw-rtb-0123456789abcdef0"],
            )]),
            expected: Vec::new(),
        },
        Narrowing {
            label: "two filters, joined with an AND",
            params: filters(&[
                ("transit-gateway-id", &[gateway_a.as_str()]),
                (
                    "resource-type",
                    &[TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT],
                ),
            ]),
            expected: vec![connect_a.clone()],
        },
        Narrowing {
            label: "the id selector naming two attachments",
            params: indexed(
                "TransitGatewayAttachmentIds",
                &[connect_a.as_str(), deleting.as_str()],
            ),
            expected: fixture.ids(&[
                "connect, gateway A, pending",
                "vpc, a gateway this store does not hold, deleting",
            ]),
        },
        Narrowing {
            label: "the id selector and a filter that excludes one of its ids",
            params: {
                let mut params = indexed(
                    "TransitGatewayAttachmentIds",
                    &[connect_a.as_str(), deleting.as_str()],
                );
                params.extend(filters(&[(
                    "resource-type",
                    &[TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT],
                )]));
                params
            },
            expected: vec![connect_a.clone()],
        },
    ];

    for narrowing in narrowings {
        let label = narrowing.label;
        let (status, body) = describe(&fixture.router, &narrowing.params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let got = reported_ids(label, &body);
        let mut want = narrowing.expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "case {label:?}: expected the attachments {want:?}, got {got:?}"
        );
    }
}

/// The association the store holds for an attachment is reported with it, in the state that
/// association is actually in — the transient state its writer left it in, and the terminal
/// one the clock owes it — and the `association.*` filters select on it. An attachment
/// nothing associated carries no association at all.
#[tokio::test]
async fn the_association_a_writer_stored_is_reported_and_filtered_on() {
    let fixture = Fixture::new().await;
    let associated = fixture.seed("vpc, gateway A, available, one tag").id.clone();
    let unassociated = fixture.seed("connect, gateway A, pending").id.clone();
    let policy_table = fixture.policy_table();

    let case = "an attachment associated with a policy table";
    let (status, body) = call(
        &fixture.router,
        REGION,
        "AssociateTransitGatewayPolicyTable",
        &[
            param("TransitGatewayPolicyTableId", &policy_table),
            param("TransitGatewayAttachmentId", &associated),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the association must be written, got {status} with body {body}"
    );

    let (status, body) = describe(&fixture.router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let report = report_of(case, &body, &associated);
    assert_eq!(
        (
            report.association_policy_table_id.as_str(),
            report.association_state.as_str(),
            report.association_route_table_id.as_str(),
        ),
        (
            policy_table.as_str(),
            TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING,
            "",
        ),
        "case {case:?}: expected the association just written, got {report:?}"
    );
    let unassociated_report = report_of(case, &body, &unassociated);
    assert_eq!(
        unassociated_report.association_policy_table_id, "",
        "case {case:?}: an attachment nothing associated must carry no association, \
         got {unassociated_report:?}"
    );

    let case = "the association.state filter, on the state the association is in";
    let (status, body) = describe(
        &fixture.router,
        &filters(&[(
            "association.state",
            &[TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING],
        )]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let got = reported_ids(case, &body);
    assert_eq!(
        got,
        vec![associated.clone()],
        "case {case:?}: expected only the associated attachment, got {got:?}"
    );

    // The move the clock owes the association, which this report must then carry: the
    // association was `associating` above and is `associated` once its due instant has passed.
    tokio::time::sleep(past_the_transition()).await;
    let case = "the association once the clock owes it `associated`";
    let (status, body) = describe(&fixture.router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let report = report_of(case, &body, &associated);
    assert_eq!(
        report.association_state, TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
        "case {case:?}: expected the association reported as {expected:?}, got {report:?}",
        expected = TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
    );
}

/// The report is paged in `TransitGatewayAttachmentId` order, a token is minted only when an
/// attachment follows the page, and the token resumes at the attachment after the last one
/// reported.
#[tokio::test]
async fn the_describe_pages_in_id_order() {
    let case = "seven attachments, five to a page";
    let fixture = Fixture::new().await;
    let all = fixture.all_ids();

    let (status, body) = describe(&fixture.router, &[param("MaxResults", "5")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let first_page = elements(&body, "transitGatewayAttachmentId");
    assert_eq!(
        first_page,
        all[..5].to_vec(),
        "case {case:?}: expected the first five ids in id order, got {first_page:?}"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken in {body}"));

    let (status, body) = describe(
        &fixture.router,
        &[param("MaxResults", "5"), param("NextToken", &token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the second page, got {status} with body {body}"
    );
    let second_page = elements(&body, "transitGatewayAttachmentId");
    assert_eq!(
        second_page,
        all[5..].to_vec(),
        "case {case:?}: expected the remaining ids, got {second_page:?}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: the last page must carry no token, got body {body}"
    );
}

/// A `MaxResults` outside the range the model publishes (`TransitGatewayMaxResults`,
/// `range 5..1000`) is refused one past each published bound, while each bound itself is
/// served.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let fixture = Fixture::new().await;

    for (case, page_size, expected_status, expected_code) in [
        ("one below the published minimum", 4, 400, "InvalidMaxResults"),
        ("the published minimum", 5, 200, ""),
        ("the published maximum", 1000, 200, ""),
        ("one above the published maximum", 1001, 400, "ValidationError"),
    ] {
        let (status, body) =
            describe(&fixture.router, &[param("MaxResults", &page_size.to_string())]).await;
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
        }
    }
}

/// A filter name this operation does not document is refused with the wording the transit
/// gateway describes give an unrecognised filter — AWS's parameter-validation sentence,
/// naming the filter the REQUEST sent, under EC2's `InvalidParameterValue`. Each name the
/// operation DOES document is then served, so what is refused is membership of the catalog
/// rather than filtering as such.
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let fixture = Fixture::new().await;

    for name in [
        "association.transit-gateway-policy-table-id",
        "resource-type ",
        "Resource-Type",
        "State",
        "tag:Name",
        "tag-key",
        "",
    ] {
        let case = format!("the filter {name:?}");
        let (status, body) = describe(&fixture.router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("Value ({name}) for parameter Filter is invalid. "),
            "case {case:?}: expected the refused filter named in the message, got {message:?}"
        );
    }

    // The names the operation documents, from
    // `DescribeTransitGatewayAttachmentsRequest$Filters`:
    //
    // smithy_query(query="doc",shape="DescribeTransitGatewayAttachmentsRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
    for name in [
        "association.state",
        "association.transit-gateway-route-table-id",
        "resource-id",
        "resource-owner-id",
        "resource-type",
        "state",
        "transit-gateway-attachment-id",
        "transit-gateway-id",
        "transit-gateway-owner-id",
    ] {
        let case = format!("the documented filter {name:?}");
        let (status, body) = describe(&fixture.router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a documented filter must be served, got {status} with body {body}"
        );
    }
}

/// What the describe cannot answer it refuses with EC2's own code: an id that is not a
/// transit gateway attachment id at all, a well-formed id the account holds no attachment
/// for, a pagination token this service did not mint, and a dry run.
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_answer() {
    struct Case<'a> {
        label: &'a str,
        params: Vec<Param>,
        status: u16,
        code: &'a str,
    }
    let fixture = Fixture::new().await;
    let cases = vec![
        Case {
            label: "an id that is not a transit gateway attachment id",
            params: indexed("TransitGatewayAttachmentIds", &["tgw-attach-nope"]),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.Malformed",
        },
        Case {
            label: "a well-formed id the account holds no attachment for",
            params: indexed(
                "TransitGatewayAttachmentIds",
                &["tgw-attach-0fedcba9876543210"],
            ),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.NotFound",
        },
        Case {
            label: "a pagination token this service did not mint",
            params: vec![param("NextToken", "not-a-token")],
            status: 400,
            code: "InvalidPaginationToken",
        },
        Case {
            label: "a dry run",
            params: vec![param("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
        },
    ];
    for case in cases {
        let label = case.label;
        let (status, body) = describe(&fixture.router, &case.params).await;
        assert_eq!(
            status, case.status,
            "case {label:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = case.status,
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {label:?}: expected the error code {expected:?}, got body {body}",
            expected = case.code,
        );
    }
}

/// An attachment of EVERY kind is resolvable by id here, because this operation reports every
/// kind: the id selector answers each seeded attachment, which is what tells this describe
/// apart from the per-kind ones that refuse an id naming an attachment of another kind.
#[tokio::test]
async fn an_id_of_any_kind_of_attachment_resolves() {
    let fixture = Fixture::new().await;

    for seed in &fixture.seeded {
        let label = seed.label;
        let (status, body) = describe(
            &fixture.router,
            &indexed("TransitGatewayAttachmentIds", &[seed.id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let got = reported_ids(label, &body);
        assert_eq!(
            got,
            vec![seed.id.clone()],
            "case {label:?}: expected only {id:?}, got {got:?}",
            id = seed.id
        );
    }
}

/// The describe is a REPORT: two identical calls around a refused one answer the same
/// attachments in the same states, and the store still holds every attachment it held, so
/// nothing a later caller sees was changed by reading.
#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let case = "two reads around a refusal";
    let fixture = Fixture::new().await;

    let (_, first) = describe(&fixture.router, &[]).await;
    let (refused, _) = describe(&fixture.router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        refused, 400,
        "case {case:?}: the middle call must be refused, got {refused}"
    );
    let (_, second) = describe(&fixture.router, &[]).await;
    assert_eq!(
        reported(case, &first),
        reported(case, &second),
        "case {case:?}: two reads must report the same attachments"
    );

    let store = fixture.engine.state::<Ec2State>();
    let store = store.lock();
    for seed in &fixture.seeded {
        let label = seed.label;
        let stored = store
            .transit_gateway_attachments
            .get(&seed.id)
            .unwrap_or_else(|| panic!("case {label:?}: {id:?} must still be stored", id = seed.id));
        assert_eq!(
            stored.view.state(),
            Some(seed.expected.state.as_str()),
            "case {label:?}: expected the stored state {expected:?}, got {got:?}",
            expected = seed.expected.state,
            got = stored.view.state(),
        );
    }
}
