// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayPeeringAttachments`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeTransitGatewayPeeringAttachments`, answered with XML
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
//! Every table row below is a distinct input along what that generalises over: which side of
//! the peering this account owns (requester, accepter, both or neither — which is what
//! `local-owner-id` and `remote-owner-id` are read through), which transit gateways are
//! peered, which state the attachment is in, whether it carries tags, an options block or an
//! accepter attachment id, which selector the request uses, how many ids it names, and how
//! large a page it asks for (both published bounds of `TransitGatewayMaxResults` and one past
//! each). The attachments are seeded through the store's own writer
//! (`insert_transit_gateway_attachment`), because `CreateTransitGatewayPeeringAttachment` is
//! still a scaffold stub.

use ec2::ops::types::{
    PeeringAttachmentStatus, PeeringTgwInfo, TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES, Tag,
    TransitGatewayPeeringAttachment, TransitGatewayPeeringAttachmentOptions,
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
const ACTION: &str = "DescribeTransitGatewayPeeringAttachments";

/// An account in another Region that owns the peer transit gateway of most of the fixtures —
/// "the ID of the Amazon Web Services account in the remote Region that owns the transit
/// gateway", as `remote-owner-id` reads it.
const PEER_ACCOUNT: &str = "210987654321";

/// Two accounts neither of which is the one this emulator serves, for the peering whose
/// gateways are both somebody else's.
const THIRD_ACCOUNT: &str = "111122223333";
const FOURTH_ACCOUNT: &str = "444455556666";

/// A member of `TransitGatewayAttachmentState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayAttachmentState")`, re-exported as
/// `TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES`) rather than written out here.
fn attachment_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayAttachmentState member"))
}

/// One transit gateway of a peering, as `PeeringTgwInfo` describes it.
struct Side {
    /// The gateway's id.
    transit_gateway_id: &'static str,
    /// The account that owns it.
    owner_id: String,
    /// The Region it is in.
    region: &'static str,
}

/// One seeded peering attachment: the facts that distinguish it along what the rule
/// generalises over.
struct Fixture {
    /// The transit gateway that requested the peering.
    requester: Side,
    /// The transit gateway the peering was requested of.
    accepter: Side,
    /// The attachment's state, a member of `TransitGatewayAttachmentState`.
    state: &'static str,
    /// "Indicates whether dynamic routing is enabled or disabled"
    /// (`TransitGatewayPeeringAttachmentOptions$DynamicRouting`) — `None` for an attachment
    /// carrying no options block at all.
    dynamic_routing: Option<&'static str>,
    /// The accepter's own attachment id, which a peering only carries once the other side
    /// has one.
    accepter_attachment_id: Option<&'static str>,
    /// The status message beside the status code, for the one row that carries one.
    status_message: Option<&'static str>,
    /// The instant the attachment was created, as seconds since the epoch.
    created_at: i64,
    /// The tags it carries, as (key, value) pairs — empty for an untagged attachment.
    tags: &'static [(&'static str, &'static str)],
}

impl Fixture {
    /// The side of the peering THIS account owns, as `local-owner-id` and
    /// `transit-gateway-id` are read through: the requester unless only the accepter is
    /// ours.
    fn local(&self) -> &Side {
        let account = engine::emulator_account_id();
        if self.accepter.owner_id == account && self.requester.owner_id != account {
            &self.accepter
        } else {
            &self.requester
        }
    }
}

/// This emulator's own account id, which a fixture's owned side carries.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
}

/// The seven peering attachments every test here is written against: a peering this account
/// requested, one it was asked of (so the LOCAL side is the accepter), one between two
/// gateways it owns, one between two gateways it owns neither of, seven distinct states out
/// of the modelled catalog with `available` twice (so a state filter selects more than one
/// row), tagged and untagged attachments, and both an attachment with an options block and
/// accepter attachment id and attachments with neither.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-0000000000000000a",
                owner_id: local_account(),
                region: "us-east-1",
            },
            accepter: Side {
                transit_gateway_id: "tgw-0000000000000000b",
                owner_id: PEER_ACCOUNT.to_string(),
                region: "us-west-2",
            },
            state: attachment_state("available"),
            dynamic_routing: Some("enable"),
            accepter_attachment_id: Some("tgw-attach-0000000000000000b"),
            status_message: Some("Peering attachment is available"),
            created_at: 1_760_000_000,
            tags: &[("Name", "north"), ("Team", "network")],
        },
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-0000000000000000a",
                owner_id: local_account(),
                region: "us-east-1",
            },
            accepter: Side {
                transit_gateway_id: "tgw-0000000000000000c",
                owner_id: PEER_ACCOUNT.to_string(),
                region: "eu-west-1",
            },
            state: attachment_state("pendingAcceptance"),
            dynamic_routing: Some("disable"),
            accepter_attachment_id: None,
            status_message: None,
            created_at: 1_760_000_100,
            tags: &[("Name", "south")],
        },
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-0000000000000000d",
                owner_id: PEER_ACCOUNT.to_string(),
                region: "us-west-2",
            },
            accepter: Side {
                transit_gateway_id: "tgw-0000000000000000e",
                owner_id: local_account(),
                region: "us-east-1",
            },
            state: attachment_state("available"),
            dynamic_routing: None,
            accepter_attachment_id: None,
            status_message: None,
            created_at: 1_760_000_200,
            tags: &[],
        },
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-0000000000000000e",
                owner_id: local_account(),
                region: "us-east-1",
            },
            accepter: Side {
                transit_gateway_id: "tgw-0000000000000000f",
                owner_id: local_account(),
                region: "us-west-2",
            },
            state: attachment_state("deleting"),
            dynamic_routing: None,
            accepter_attachment_id: None,
            status_message: None,
            created_at: 1_760_000_300,
            tags: &[("Team", "network")],
        },
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-00000000000000011",
                owner_id: THIRD_ACCOUNT.to_string(),
                region: "ap-south-1",
            },
            accepter: Side {
                transit_gateway_id: "tgw-00000000000000012",
                owner_id: FOURTH_ACCOUNT.to_string(),
                region: "ap-northeast-1",
            },
            state: attachment_state("rejected"),
            dynamic_routing: None,
            accepter_attachment_id: None,
            status_message: None,
            created_at: 1_760_000_400,
            tags: &[],
        },
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-0000000000000000a",
                owner_id: local_account(),
                region: "us-east-1",
            },
            accepter: Side {
                transit_gateway_id: "tgw-00000000000000013",
                owner_id: PEER_ACCOUNT.to_string(),
                region: "us-west-2",
            },
            state: attachment_state("failed"),
            dynamic_routing: None,
            accepter_attachment_id: None,
            status_message: None,
            created_at: 1_760_000_500,
            tags: &[("Name", "north")],
        },
        Fixture {
            requester: Side {
                transit_gateway_id: "tgw-0000000000000000a",
                owner_id: local_account(),
                region: "us-east-1",
            },
            accepter: Side {
                transit_gateway_id: "tgw-00000000000000014",
                owner_id: PEER_ACCOUNT.to_string(),
                region: "ca-central-1",
            },
            state: attachment_state("modifying"),
            dynamic_routing: None,
            accepter_attachment_id: None,
            status_message: None,
            created_at: 1_760_000_600,
            tags: &[("Name", "east")],
        },
    ]
}

/// The modelled `PeeringTgwInfo` one side of a fixture is stored as.
fn stored_side(side: &Side) -> PeeringTgwInfo {
    PeeringTgwInfo {
        transit_gateway_id: Some(side.transit_gateway_id.to_string()),
        owner_id: Some(side.owner_id.clone()),
        region: Some(side.region.to_string()),
        // Nothing here peers a core network, so the member stays absent rather than being
        // given a value no writer chose.
        core_network_id: None,
    }
}

/// The modelled peering attachment one fixture is stored as.
fn stored_attachment(fixture: &Fixture) -> TransitGatewayPeeringAttachment {
    TransitGatewayPeeringAttachment {
        // The store mints the id; what a writer puts here is replaced by it.
        transit_gateway_attachment_id: None,
        accepter_transit_gateway_attachment_id: fixture
            .accepter_attachment_id
            .map(|value| value.to_string()),
        requester_tgw_info: Some(stored_side(&fixture.requester)),
        accepter_tgw_info: Some(stored_side(&fixture.accepter)),
        options: fixture
            .dynamic_routing
            .map(|routing| TransitGatewayPeeringAttachmentOptions {
                dynamic_routing: Some(routing.to_string()),
            }),
        // "The status of the transit gateway peering attachment" is the status OF the state
        // it is in, which is how every writer of this store sets it.
        status: Some(PeeringAttachmentStatus {
            code: Some(fixture.state.to_string()),
            message: fixture.status_message.map(|value| value.to_string()),
        }),
        state: Some(fixture.state.to_string()),
        creation_time: Some(::aws_smithy_types::DateTime::from_secs(fixture.created_at)),
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

/// Seed [`fixtures`] into the store through its own writer and answer the ids it minted, in
/// fixture order. One attachment of ANOTHER kind is seeded beside them, so every assertion
/// about what this describe reports is also an assertion that it reports only the peering
/// attachments of the one id space every kind of attachment shares.
fn seed(engine: &engine::InMemoryEngine) -> (Vec<String>, String) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let ids = fixtures()
        .iter()
        .map(|fixture| {
            guard
                .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Peering(
                    stored_attachment(fixture),
                ))
                .expect("the store holds the seeded peering attachment")
        })
        .collect();
    let other_kind = guard
        .insert_transit_gateway_attachment(super::transit_gateway_acceptance::pending_vpc_attachment())
        .expect("the store holds the seeded VPC attachment");
    (ids, other_kind)
}

/// The emulator with the attachments above seeded: the router to call, the engine, the
/// peering attachment ids in fixture order, and the id of the attachment of another kind.
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

/// One `DescribeTransitGatewayPeeringAttachments` call carrying `params`.
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

/// The ids of every peering attachment a response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "transitGatewayAttachmentId")
}

/// `TransitGatewayAttachmentIds.N` form fields for `ids`.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("TransitGatewayAttachmentIds", ids)
}

/// Read something off the one peering attachment a response carries — the element that has a
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
        .unwrap_or_else(|| panic!("case {case:?}: no peering attachment in {body}"));
    read(attachment)
}

/// A member of the one peering attachment a response carries, or `None` when it carries no
/// such member.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    with_attachment(case, body, |attachment| child_text(attachment, name))
}

/// A member of a nested element (`requesterTgwInfo/ownerId`) of the one peering attachment a
/// response carries.
fn nested(case: &str, body: &str, outer: &str, inner: &str) -> Option<String> {
    with_attachment(case, body, |attachment| {
        child(attachment, outer).and_then(|node| child_text(node, inner))
    })
}

/// Whether the one peering attachment a response carries has a `name` child ELEMENT at all.
///
/// The absence check for a member nothing recorded: [`member`] reads an element's TEXT, which
/// a present-but-empty container (`<tagSet/>`) does not carry, so only element absence shows
/// that nothing was reported for it.
fn has_member(case: &str, body: &str, name: &str) -> bool {
    with_attachment(case, body, |attachment| {
        child(attachment, name).is_some()
    })
}

/// The (key, value) pairs of the one peering attachment a response carries.
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

/// Every fact a stored peering attachment carries reads back: the id the store minted, both
/// peered gateways with their owners and Regions, the state and the status of it, the options
/// block, the accepter's attachment id, the instant it was created and the tags on it. One
/// row per seeded attachment, so no single attachment is the specification — and the rows
/// carrying neither options nor an accepter attachment id nor tags assert that a member the
/// store holds no value for is reported absent rather than invented.
#[tokio::test]
async fn every_fact_a_stored_peering_attachment_carries_reads_back() {
    let (router, _engine, ids, _other_kind) = seeded();

    for (fixture, attachment_id) in fixtures().iter().zip(&ids) {
        let case = format!(
            "peering attachment {attachment_id} between {} and {}",
            fixture.requester.transit_gateway_id, fixture.accepter.transit_gateway_id
        );
        let body = describe_ok(&case, &router, &id_params(&[attachment_id.as_str()])).await;

        assert_eq!(
            reported_ids(&body),
            vec![attachment_id.clone()],
            "case {case:?}: expected exactly the attachment named, got {body}"
        );
        let actual = member(&case, &body, "state");
        assert_eq!(
            actual.as_deref(),
            Some(fixture.state),
            "case {case:?}: expected <state> {:?}, got {actual:?} in {body}",
            fixture.state
        );
        let actual = nested(&case, &body, "status", "code");
        assert_eq!(
            actual.as_deref(),
            Some(fixture.state),
            "case {case:?}: the status reports the state it is the status of, got {actual:?} \
             in {body}"
        );
        let actual = nested(&case, &body, "status", "message");
        assert_eq!(
            actual.as_deref(),
            fixture.status_message,
            "case {case:?}: expected the stored status message, got {actual:?} in {body}"
        );

        for (outer, side) in [
            ("requesterTgwInfo", &fixture.requester),
            ("accepterTgwInfo", &fixture.accepter),
        ] {
            for (inner, expected) in [
                ("transitGatewayId", side.transit_gateway_id.to_string()),
                ("ownerId", side.owner_id.clone()),
                ("region", side.region.to_string()),
            ] {
                let actual = nested(&case, &body, outer, inner);
                assert_eq!(
                    actual.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected <{outer}>/<{inner}> {expected:?}, got {actual:?} \
                     in {body}"
                );
            }
            let actual = nested(&case, &body, outer, "coreNetworkId");
            assert_eq!(
                actual, None,
                "case {case:?}: <{outer}> peers no core network, got {actual:?} in {body}"
            );
        }

        let actual = nested(&case, &body, "options", "dynamicRouting");
        assert_eq!(
            actual.as_deref(),
            fixture.dynamic_routing,
            "case {case:?}: expected the stored dynamic routing option, got {actual:?} in \
             {body}"
        );
        let actual = member(&case, &body, "accepterTransitGatewayAttachmentId");
        assert_eq!(
            actual.as_deref(),
            fixture.accepter_attachment_id,
            "case {case:?}: expected the stored accepter attachment id, got {actual:?} in \
             {body}"
        );

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

/// A request that names no selector reports every PEERING attachment the store holds, in
/// `TransitGatewayAttachmentId` order — and no attachment of another kind, though every kind
/// shares the one id space this store is keyed by.
#[tokio::test]
async fn an_unselective_describe_reports_every_stored_peering_attachment_in_id_order() {
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
        "case {case:?}: a VPC attachment is no peering attachment, got {actual:?} in {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: a complete answer carries no nextToken, got {body}"
    );
}

/// An empty store is an empty answer, not a refusal: a describe of a resource kind nothing
/// has created reports no peering attachments at all.
#[tokio::test]
async fn an_empty_store_reports_no_peering_attachments() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "empty store";

    let body = describe_ok(case, &router, &[]).await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no peering attachments, got {actual:?} in {body}"
    );
}

/// Each filter selects exactly the attachments carrying its value, and several filters are
/// joined with AND. The rows span every filter name the operation documents, a value nothing
/// carries, a wildcard value, two values in one filter, and two-filter combinations.
#[tokio::test]
async fn each_filter_selects_the_attachments_that_carry_its_value() {
    let (router, _engine, ids, _other_kind) = seeded();
    let rows = fixtures();
    let account = local_account();

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
            "local-owner-id of this account",
            filters(&[("local-owner-id", &[account.as_str()])]),
            vec![0, 1, 2, 3, 5, 6],
        ),
        (
            "local-owner-id of a peering this account owns neither side of",
            filters(&[("local-owner-id", &[THIRD_ACCOUNT])]),
            vec![4],
        ),
        (
            "remote-owner-id of the peer account",
            filters(&[("remote-owner-id", &[PEER_ACCOUNT])]),
            vec![0, 1, 2, 5, 6],
        ),
        (
            // A peering between two gateways of one account has that account on BOTH sides.
            "remote-owner-id of this account",
            filters(&[("remote-owner-id", &[account.as_str()])]),
            vec![3],
        ),
        (
            "remote-owner-id of the fourth account",
            filters(&[("remote-owner-id", &[FOURTH_ACCOUNT])]),
            vec![4],
        ),
        (
            "transit-gateway-id of four attachments",
            filters(&[(
                "transit-gateway-id",
                &[rows[0].local().transit_gateway_id],
            )]),
            vec![0, 1, 5, 6],
        ),
        (
            // The local side is the accepter in one of these two rows and the requester in
            // the other, so a reading that looked only at the requester would fail here.
            "transit-gateway-id reached from either side of a peering",
            filters(&[(
                "transit-gateway-id",
                &[rows[2].local().transit_gateway_id],
            )]),
            vec![2, 3],
        ),
        (
            // "Filter values support wildcards" is EC2's filter contract: `*` stands for
            // zero or more characters, so a prefix pattern selects every transit gateway.
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
            "tag:Name of two attachments",
            filters(&[("tag:Name", &["north"])]),
            vec![0, 5],
        ),
        (
            "tag:Name of one attachment",
            filters(&[("tag:Name", &["east"])]),
            vec![6],
        ),
        (
            "tag:Name no attachment carries",
            filters(&[("tag:Name", &["west"])]),
            Vec::new(),
        ),
        (
            "tag:Team",
            filters(&[("tag:Team", &["network"])]),
            vec![0, 3],
        ),
        ("tag-key Name", filters(&[("tag-key", &["Name"])]), vec![
            0, 1, 5, 6,
        ]),
        ("tag-key Team", filters(&[("tag-key", &["Team"])]), vec![
            0, 3,
        ]),
        (
            "tag-key no attachment carries",
            filters(&[("tag-key", &["Owner"])]),
            Vec::new(),
        ),
        (
            "local-owner-id and state joined with AND",
            filters(&[
                ("local-owner-id", &[account.as_str()]),
                ("state", &[attachment_state("available")]),
            ]),
            vec![0, 2],
        ),
        (
            "transit-gateway-id and state joined with AND",
            filters(&[
                ("transit-gateway-id", &[rows[0].local().transit_gateway_id]),
                ("state", &[attachment_state("available")]),
            ]),
            vec![0],
        ),
        (
            "a state and a remote owner that no one attachment carries together",
            filters(&[
                ("state", &[attachment_state("deleting")]),
                ("remote-owner-id", &[PEER_ACCOUNT]),
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

    // The id filter is the id selector's twin, so it is asserted over every seeded
    // attachment rather than over one row.
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

/// A filter that names the attachment of ANOTHER kind sharing this store selects nothing:
/// this operation reports peering attachments, so the VPC attachment beside them is not one
/// of its rows however it is asked for.
#[tokio::test]
async fn an_attachment_of_another_kind_is_not_selected_by_any_filter() {
    let (router, _engine, _ids, other_kind) = seeded();
    let case = format!("transit-gateway-attachment-id {other_kind} of a VPC attachment");

    let body = describe_ok(
        &case,
        &router,
        &filters(&[("transit-gateway-attachment-id", &[other_kind.as_str()])]),
    )
    .await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no peering attachments, got {actual:?} in {body}"
    );
}

/// One row of the id-list table: the case, the ids the request names, the filters it carries
/// beside them, and the fixture positions the answer must carry.
type IdListCase<'a> = (&'static str, Vec<&'a str>, Vec<Param>, Vec<usize>);

/// The id list selects exactly the attachments it names, whatever its length and however
/// often an id is repeated, and it narrows what a filter would have selected.
#[tokio::test]
async fn an_id_list_selects_exactly_the_attachments_it_names() {
    let (router, _engine, ids, _other_kind) = seeded();

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
            "two ids narrowed by a tag filter",
            vec![ids[0].as_str(), ids[5].as_str()],
            filters(&[("tag:Team", &["network"])]),
            vec![0],
        ),
        (
            "two ids narrowed by a remote owner",
            vec![ids[3].as_str(), ids[4].as_str()],
            filters(&[("remote-owner-id", &[FOURTH_ACCOUNT])]),
            vec![4],
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

/// A well-formed id naming no peering attachment is refused with the attachment's own
/// `NotFound` code — including the id of an attachment of another kind, which names no
/// peering attachment — and a value that is not an attachment id at all with its `Malformed`
/// code, so a client can tell the two apart and neither is reported as an absent row. Both
/// are asserted beside an id that DOES name a peering attachment, so the refusal does not
/// depend on the request naming nothing real.
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
    // whichever of them sorts first: the two rows name the same pair of unknown ids in the
    // two possible orders, so an implementation that walked a sorted set would fail one.
    let lower = format!("{prefix}-00000000000000aaa");
    let higher = format!("{prefix}-00000000000000bbb");
    for (case, first, second) in [
        ("the lower id listed first", &lower, &higher),
        ("the higher id listed first", &higher, &lower),
    ] {
        let (status, body) =
            describe(&router, &id_params(&[first.as_str(), second.as_str()])).await;
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

/// A filter name this operation documents none of is refused, naming the filter — never read
/// as "no results". The rows span a misspelling, a filter of ANOTHER describe, this
/// operation's own names in the wrong case ("Filter names are case-sensitive"), a `tag:`
/// prefix with no key, and an empty name.
#[tokio::test]
async fn a_filter_this_operation_does_not_document_is_refused() {
    let (router, _engine, _ids, _other_kind) = seeded();

    for name in [
        "owner-id",
        "resource-type",
        "vpc-id",
        "transit-gateway-peering-attachment-id",
        "State",
        "Local-Owner-Id",
        "tag:",
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
/// attachment's own `NotFound` code: proof the SIZE check passed and the request went on to
/// be resolved.
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
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("state", &one_unmatched[..]))
            .collect();
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
/// last page carries none. Asserted at the published floor of `TransitGatewayMaxResults`, at
/// a size between the bounds, and at a size that holds everything.
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

/// A `MaxResults` outside the range the model publishes (`TransitGatewayMaxResults`,
/// `range 5..1000`) is refused at the boundary, one past each published bound, while each
/// bound itself is served. A page size below the smallest the operation serves carries EC2's
/// own `InvalidMaxResults`; one above the largest keeps the boundary validator's
/// `ValidationError`. Either way the message names the member it rejected — asserted, so a
/// 400 for some other reason could not pass for it.
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
        let (status, body) =
            describe(&router, &[param("MaxResults", &page_size.to_string())]).await;
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

/// The wire error code the boundary validator refuses a request with when a modelled
/// constraint rejects it before the handler runs — as `MaxResults` ABOVE
/// `TransitGatewayMaxResults`' published maximum is.
const BOUNDARY_VALIDATION_ERROR: &str = "ValidationError";

/// EC2's own code for a page size below the smallest the operation serves, which this
/// service answers in place of the boundary validator's generic one (the service's
/// `INVALID_MAX_RESULTS`).
const INVALID_MAX_RESULTS: &str = "InvalidMaxResults";

/// A `NextToken` this service did not mint is refused rather than read as an id, whatever
/// else the request names, and the refusal carries EC2's own sentence for an unusable token.
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
/// attachments twice over, and the store holds exactly the attachments it held before — a
/// read neither mints nor moves a resource.
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
    for name in ["state", "code", "ownerId", "region", "creationTime", "key", "value"] {
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
