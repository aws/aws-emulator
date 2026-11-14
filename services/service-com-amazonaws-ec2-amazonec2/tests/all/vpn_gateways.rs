// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the virtual private gateway slice — `CreateVpnGateway`, the
//! `DescribeVpnGateways` it reads back through, and the `DeleteVpnGateway` that takes one
//! away — driven over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A CREATE STORES THE GATEWAY THE REQUEST DESCRIBES: the requested type and Availability
//!    Zone, the Amazon-side ASN the request asked for or the default its documentation
//!    publishes (64512), and the tags of a `vpn-gateway` tag specification; it is answered
//!    with a server-minted id in the non-terminal `pending` state the clock then owes
//!    `available`, attached to no VPC, and every member of it reads back through
//!    `DescribeVpnGateways`.
//! 2. A VALUE OUTSIDE WHAT THE OPERATION PUBLISHES IS REFUSED rather than defaulted or
//!    stored — an ASN outside both published private ranges, an Availability Zone the Region
//!    does not have, a tag specification naming another resource type.
//! 3. A DELETE NAMES A GATEWAY BY ID: an id the account holds no gateway for is refused with
//!    `InvalidVpnGatewayID.NotFound` and an id that is not shaped like a virtual private
//!    gateway id with `InvalidVpnGatewayID.Malformed`, whatever the store holds.
//! 4. A DELETE OF A STORED GATEWAY MOVES IT THROUGH `deleting` TO `deleted`: `VpnState`
//!    carries both words, so the gateway keeps being reported while it goes away, and the
//!    move falls due on the clock rather than at the next call.
//! 5. `DryRun` CHANGES NOTHING: a create and a delete that would have succeeded answer
//!    `DryRunOperation` and leave the store exactly as it was.
//! 6. EVERY REFUSAL NAMES THE REQUEST IT ANSWERS: EC2's ec2Query error envelope carries a
//!    `<RequestID>` beside its `<Errors>`, in the 8-4-4-4-12 lowercase-hex form AWS reports,
//!    and two refusals never name one request.
//!
//! Every table row is a distinct input along what the rule generalises over — the
//! Availability Zone, the ASN at each published bound and one past it, the tag count, the id
//! form, the state the gateway is deleted in — so the one case a recording happens to
//! contain is one row among many rather than the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters, unescaped};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for, whose Availability Zones a gateway may name.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that a move the clock owes is certainly
/// due.
fn past_the_transition() -> Duration {
    ec2::state::TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `CreateVpnGateway` call carrying `params` beside the required `Type`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    let mut fields = vec![param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1)];
    fields.extend(params.iter().cloned());
    call(router, "CreateVpnGateway", &fields).await
}

/// `CreateVpnGateway` requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpnGateway, got {status} with body {body}"
    );
    body
}

/// One `DescribeVpnGateways` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, "DescribeVpnGateways", params).await
}

/// `DescribeVpnGateways` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpnGateways, got {status} with \
         body {body}"
    );
    body
}

/// One `DeleteVpnGateway` call naming `vpn_gateway_id`, with any extra `params`.
async fn delete(
    router: &MultiServiceRouter,
    vpn_gateway_id: &str,
    params: &[Param],
) -> (u16, String) {
    let mut fields = vec![param("VpnGatewayId", vpn_gateway_id)];
    fields.extend(params.iter().cloned());
    call(router, "DeleteVpnGateway", &fields).await
}

/// `DeleteVpnGateway` requiring success, and the body it answered.
async fn delete_ok(case: &str, router: &MultiServiceRouter, vpn_gateway_id: &str) -> String {
    let (status, body) = delete(router, vpn_gateway_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteVpnGateway, got {status} with body {body}"
    );
    body
}

/// The name of the Availability Zone at `index` in this Region, from the one place this
/// crate derives a Region's zones ([`ec2::state::availability_zones_in_region`]) rather
/// than from a hand-written list.
fn zone_name(index: usize) -> String {
    let zones = ec2::state::availability_zones_in_region(REGION);
    zones
        .get(index)
        .map(|zone| zone.zone_name.clone())
        .unwrap_or_else(|| panic!("Region {REGION} must have an Availability Zone at {index}"))
}

/// The form fields for `tags` as a `vpn-gateway` tag specification.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        ec2::state::RESOURCE_TYPE_VPN_GATEWAY,
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        let index = position + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{index}.Value"),
            value,
        ));
    }
    params
}

/// The `<RequestID>` the ec2Query error envelope names the request it answers with, having
/// first pinned the form every AWS request id carries.
///
/// A correlation id: what it must satisfy is the FORM and distinctness, never a recorded
/// value, so the rows below collect the ids they saw and assert they differ.
fn request_id(case: &str, body: &str) -> String {
    let id = element(body, "RequestID")
        .unwrap_or_else(|| panic!("case {case:?}: no <RequestID> in the refusal {body}"));
    let widths: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        widths,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected the 8-4-4-4-12 request-id form, got {id:?}"
    );
    assert!(
        id.bytes()
            .all(|byte| byte == b'-'
                || (byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())),
        "case {case:?}: expected lowercase hexadecimal groups, got {id:?}"
    );
    id
}

/// Assert that every id in `seen` named a different request.
fn assert_distinct(what: &str, seen: &[String]) {
    let mut sorted = seen.to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seen.len(),
        "{what}: expected {} distinct request ids, got {seen:?}",
        seen.len()
    );
}

/// One virtual private gateway as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Gateway {
    id: String,
    state: String,
    gateway_type: Option<String>,
    availability_zone: Option<String>,
    amazon_side_asn: Option<String>,
    tags: Vec<(String, String)>,
    attachments: usize,
}

/// Every virtual private gateway a response carries, in document order: the elements that
/// have a `vpnGatewayId` child.
fn gateways(case: &str, body: &str) -> Vec<Gateway> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "vpnGatewayId").is_some())
        .map(|node| Gateway {
            id: child_text(node, "vpnGatewayId").unwrap_or_default(),
            // A direct child, so the gateway's own state is never read off an attachment,
            // which carries a `<state>` too.
            state: child_text(node, "state").unwrap_or_default(),
            gateway_type: child_text(node, "type"),
            availability_zone: child_text(node, "availabilityZone"),
            amazon_side_asn: child_text(node, "amazonSideAsn"),
            // The set's entries are read through the one shared helper, so the item-
            // selection rule lives in `super::wire` rather than here.
            tags: set_items(node, "tagSet")
                .into_iter()
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect(),
            attachments: set_items(node, "attachments").len(),
        })
        .collect()
}

/// The one gateway a response carries, or a panic naming the case.
fn one(case: &str, body: &str) -> Gateway {
    let found = gateways(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one virtual private gateway, got {found:?} in {body}"
    );
    found.into_iter().next().expect("one gateway")
}

/// The ids every gateway a response reports carries, in document order.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    gateways(case, body)
        .into_iter()
        .map(|gateway| gateway.id)
        .collect()
}

/// What a gateway must report, beside the id and the state the call decides.
#[derive(Debug, Clone)]
struct Expected {
    availability_zone: Option<String>,
    amazon_side_asn: String,
    tags: Vec<(String, String)>,
}

impl Expected {
    /// The gateway a request that names nothing but the action and the type gets: no
    /// Availability Zone, no tags, and the ASN default `CreateVpnGatewayRequest$AmazonSideAsn`
    /// publishes, which this crate holds in one place.
    fn defaults() -> Self {
        Self {
            availability_zone: None,
            amazon_side_asn: ec2::state::DEFAULT_AMAZON_SIDE_ASN.to_string(),
            tags: Vec::new(),
        }
    }

    fn with_zone(mut self, zone: &str) -> Self {
        self.availability_zone = Some(zone.to_string());
        self
    }

    fn with_asn(mut self, asn: i64) -> Self {
        self.amazon_side_asn = asn.to_string();
        self
    }

    fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        self.tags = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        self
    }
}

/// Assert every member of `expected` on `gateway`, which must be in `state`.
fn assert_gateway(case: &str, gateway: &Gateway, expected: &Expected, state: &str) {
    assert!(
        is_long_id(ec2::state::VPN_GATEWAY_ID_PREFIX, &gateway.id),
        "case {case:?}: expected a vgw- long id, got {:?}",
        gateway.id
    );
    assert_eq!(
        gateway.state, state,
        "case {case:?}: expected state {state:?}, got {:?}",
        gateway.state
    );
    assert_eq!(
        gateway.gateway_type.as_deref(),
        Some(ec2::state::GATEWAY_TYPE_IPSEC_1),
        "case {case:?}: expected the requested type, got {:?}",
        gateway.gateway_type
    );
    assert_eq!(
        gateway.availability_zone, expected.availability_zone,
        "case {case:?}: expected availabilityZone {:?}, got {:?}",
        expected.availability_zone, gateway.availability_zone
    );
    assert_eq!(
        gateway.amazon_side_asn.as_deref(),
        Some(expected.amazon_side_asn.as_str()),
        "case {case:?}: expected amazonSideAsn {:?}, got {:?}",
        expected.amazon_side_asn,
        gateway.amazon_side_asn
    );
    assert_eq!(
        gateway.tags, expected.tags,
        "case {case:?}: expected tags {:?}, got {:?}",
        expected.tags, gateway.tags
    );
    assert_eq!(
        gateway.attachments, 0,
        "case {case:?}: a gateway nothing attached must report no VPC attachment, got {:?}",
        gateway.attachments
    );
}

// ---------------------------------------------------------------------------
// RULE 1: what a create stores, and reading it back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_vpn_gateway_stores_the_request_and_round_trips_through_describe() {
    let router = emulator();
    // (case, the request's form fields, what the gateway must report)
    let cases: Vec<(&str, Vec<Param>, Expected)> = vec![
        ("nothing but the action and the type", Vec::new(), Expected::defaults()),
        (
            "the Region's first Availability Zone",
            vec![param("AvailabilityZone", &zone_name(0))],
            Expected::defaults().with_zone(&zone_name(0)),
        ),
        (
            "another Availability Zone of the same Region",
            vec![param("AvailabilityZone", &zone_name(2))],
            Expected::defaults().with_zone(&zone_name(2)),
        ),
        (
            "the 16-bit ASN range's floor",
            vec![param("AmazonSideAsn", "64512")],
            Expected::defaults().with_asn(64_512),
        ),
        (
            "the 16-bit ASN range's ceiling",
            vec![param("AmazonSideAsn", "65534")],
            Expected::defaults().with_asn(65_534),
        ),
        (
            "the 32-bit ASN range's floor",
            vec![param("AmazonSideAsn", "4200000000")],
            Expected::defaults().with_asn(4_200_000_000),
        ),
        (
            "the 32-bit ASN range's ceiling",
            vec![param("AmazonSideAsn", "4294967294")],
            Expected::defaults().with_asn(4_294_967_294),
        ),
        (
            "one tag",
            tag_params(&[("Name", "alpha")]),
            Expected::defaults().with_tags(&[("Name", "alpha")]),
        ),
        (
            "three tags, a zone and an ASN at once",
            {
                let mut params = vec![
                    param("AvailabilityZone", &zone_name(1)),
                    param("AmazonSideAsn", "65000"),
                ];
                params.extend(tag_params(&[
                    ("Name", "beta"),
                    ("Tier", "edge"),
                    ("Owner", "TeamA"),
                ]));
                params
            },
            Expected::defaults()
                .with_zone(&zone_name(1))
                .with_asn(65_000)
                .with_tags(&[("Name", "beta"), ("Tier", "edge"), ("Owner", "TeamA")]),
        ),
    ];

    for (case, params, expected) in cases {
        let created = create_ok(case, &router, &params).await;
        let gateway = one(case, &created);
        // "After you create the virtual private gateway" it comes up, so a create answers
        // the non-terminal `pending` state the clock then owes `available`.
        assert_gateway(
            case,
            &gateway,
            &expected,
            ec2::state::VPN_GATEWAY_STATE_PENDING,
        );

        // The round trip: the describe narrowed to this id reports exactly it, with every
        // member the create answered.
        let read = describe_ok(case, &router, &[param("VpnGatewayId.1", &gateway.id)]).await;
        assert_eq!(
            reported_ids(case, &read),
            vec![gateway.id.clone()],
            "case {case:?}: the describe narrowed to one id must report exactly it: {read}"
        );
        let read_back = one(case, &read);
        assert!(
            read_back.state == ec2::state::VPN_GATEWAY_STATE_PENDING
                || read_back.state == ec2::state::VPN_GATEWAY_STATE_AVAILABLE,
            "case {case:?}: expected pending or available from the describe, got {:?}",
            read_back.state
        );
        assert_eq!(
            Gateway {
                state: gateway.state.clone(),
                ..read_back.clone()
            },
            gateway,
            "case {case:?}: every member must round-trip through the describe, got {read}"
        );
    }
}

#[tokio::test]
async fn a_created_gateway_reaches_available_when_the_clock_owes_it() {
    let case = "a gateway created with nothing but the type";
    let router = emulator();
    let created = create_ok(case, &router, &[]).await;
    let id = one(case, &created).id;

    tokio::time::sleep(past_the_transition()).await;

    let read = describe_ok(case, &router, &[param("VpnGatewayId.1", &id)]).await;
    let gateway = one(case, &read);
    assert_eq!(
        gateway.state,
        ec2::state::VPN_GATEWAY_STATE_AVAILABLE,
        "case {case:?}: expected the clock-owed available state, got {:?}",
        gateway.state
    );
}

// ---------------------------------------------------------------------------
// RULE 2: what a create refuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_vpn_gateway_refuses_a_value_outside_what_it_publishes() {
    let router = emulator();
    // (case, the request's form fields, the HTTP status, the error code)
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "one below the 16-bit ASN range",
            vec![param("AmazonSideAsn", "64511")],
            400,
            "InvalidParameterValue",
        ),
        (
            "one above the 16-bit ASN range",
            vec![param("AmazonSideAsn", "65535")],
            400,
            "InvalidParameterValue",
        ),
        (
            "one below the 32-bit ASN range",
            vec![param("AmazonSideAsn", "4199999999")],
            400,
            "InvalidParameterValue",
        ),
        (
            "one above the 32-bit ASN range",
            vec![param("AmazonSideAsn", "4294967295")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an Availability Zone of another Region",
            vec![param("AvailabilityZone", "eu-west-1a")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an Availability Zone that is not a zone name at all",
            vec![param("AvailabilityZone", "nowhere")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming another resource type",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "misdirected"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (case, params, expected_status, expected_code) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from CreateVpnGateway, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code} refusal, got {body}"
        );
        request_ids.push(request_id(case, &body));
    }
    assert_distinct("the refused creates", &request_ids);

    // A refused create stored nothing at all.
    let read = describe_ok("after every refusal", &router, &[]).await;
    assert!(
        reported_ids("after every refusal", &read).is_empty(),
        "a refused create must store no gateway, got {read}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the ids a delete refuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_vpn_gateway_refuses_an_id_that_names_no_gateway_of_the_account() {
    let router = emulator();
    // One stored gateway, so a refusal below is about the id the request named rather than
    // about an empty store.
    let stored = one("a stored gateway", &create_ok("a stored gateway", &router, &[]).await).id;

    // (case, the id the request names, the error code it is refused with)
    let not_found = [
        // The id the recorded negative path names: the long form, every digit zero.
        ("the long form, all zeros", "vgw-00000000000000000"),
        ("the long form, all f", "vgw-0ffffffffffffffff"),
        // EC2 ids come in a short and a long form; both are well formed, so both reach the
        // lookup and are refused as unknown rather than as malformed.
        ("the short form", "vgw-12345678"),
        ("a long form with mixed digits", "vgw-0a1b2c3d4e5f60718"),
    ];
    let mut request_ids: Vec<String> = Vec::new();
    for (case, vpn_gateway_id) in not_found {
        let (status, body) = delete(&router, vpn_gateway_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteVpnGateway, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_VPN_GATEWAY_ID_NOT_FOUND,
            "case {case:?}: expected the not-found refusal, got {body}"
        );
        // The message is built from the request, so it names the id the caller gave.
        let message = element(&body, "Message").map(|text| unescaped(&text));
        let wanted = format!("The vpnGateway ID '{vpn_gateway_id}' does not exist");
        assert_eq!(
            message.as_deref(),
            Some(wanted.as_str()),
            "case {case:?}: expected the message {wanted:?}, got {message:?}"
        );
        request_ids.push(request_id(case, &body));
    }

    // (case, an id that is not shaped like a virtual private gateway id)
    let malformed = [
        ("the prefix with nothing after it", "vgw-"),
        ("the prefix and no hyphen", "vgw0ffffffffffffffff"),
        ("digits that are not hexadecimal", "vgw-zzzzzzzz"),
        ("one digit short of the long form", "vgw-0fffffffffffffff"),
        ("another resource kind's id", "vpc-00000000000000000"),
        ("no prefix at all", "00000000000000000"),
    ];
    for (case, vpn_gateway_id) in malformed {
        let (status, body) = delete(&router, vpn_gateway_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteVpnGateway, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_VPN_GATEWAY_ID_MALFORMED,
            "case {case:?}: expected the malformed refusal, got {body}"
        );
        request_ids.push(request_id(case, &body));
    }
    assert_distinct("the refused deletes", &request_ids);

    // Every refusal above left the one stored gateway exactly as it was.
    let read = describe_ok("after every refusal", &router, &[]).await;
    assert_eq!(
        reported_ids("after every refusal", &read),
        vec![stored.clone()],
        "a refused delete must not touch the store, got {read}"
    );
    let gateway = one("after every refusal", &read);
    assert_ne!(
        gateway.state,
        ec2::state::VPN_GATEWAY_STATE_DELETING,
        "a refused delete must not start the deletion of another gateway: {read}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: what a delete does to a stored gateway
// ---------------------------------------------------------------------------

/// The rows: the state the gateway is deleted in — one still coming up, one that has reached
/// `available` — and a delete repeated on a gateway that is already going away, which is a
/// gateway no delete can act on.
#[tokio::test]
async fn delete_vpn_gateway_moves_the_gateway_through_deleting_to_deleted() {
    // (case, wait for the create's clock-owed move first, delete twice)
    let cases = [
        ("a gateway still coming up", false, false),
        ("a gateway that has reached available", true, false),
        ("a delete repeated on a gateway already going away", true, true),
    ];

    for (case, settle_first, delete_twice) in cases {
        let router = emulator();
        let created = create_ok(case, &router, &tag_params(&[("Name", case)])).await;
        let id = one(case, &created).id;
        if settle_first {
            tokio::time::sleep(past_the_transition()).await;
        }

        let answered = delete_ok(case, &router, &id).await;
        // When the FIRST delete's clock-owed move falls due. A repeat must not push this
        // back, so the `deleted` state below is asserted at this instant rather than at one
        // measured from the last call.
        let deleted_due_at = tokio::time::Instant::now() + past_the_transition();
        // A mutation that reports no result of its own carries the envelope's success flag.
        assert_eq!(
            element(&answered, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected the envelope's success flag, got {answered}"
        );

        // The gateway is still reported, in the non-terminal state a delete leaves it in.
        let read = describe_ok(case, &router, &[param("VpnGatewayId.1", &id)]).await;
        let gateway = one(case, &read);
        assert_eq!(
            gateway.state,
            ec2::state::VPN_GATEWAY_STATE_DELETING,
            "case {case:?}: expected the deleting state straight after the delete, got {:?}",
            gateway.state
        );
        assert_eq!(
            gateway.tags,
            vec![("Name".to_string(), case.to_string())],
            "case {case:?}: a gateway on its way out keeps its tags, got {:?}",
            gateway.tags
        );

        if delete_twice {
            // A gateway whose deletion is already under way is in no state to be deleted: the
            // repeat is refused rather than answering as though it had deleted the gateway a
            // second time.
            let (status, repeated) = delete(&router, &id, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from a repeated delete, got {status} with \
                 body {repeated}"
            );
            assert_eq!(
                error_code(&repeated),
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected the wrong-state refusal on the repeat, got {repeated}"
            );
            // The repeat changed nothing: the gateway is still the one the first delete
            // left going away.
            let after = describe_ok(case, &router, &[param("VpnGatewayId.1", &id)]).await;
            let gateway = one(case, &after);
            assert_eq!(
                gateway.state,
                ec2::state::VPN_GATEWAY_STATE_DELETING,
                "case {case:?}: a repeat must leave the deleting state alone, got {:?}",
                gateway.state
            );
        }

        // Only as far as the FIRST delete's due instant: a repeat that had restarted the
        // clock would still report `deleting` here.
        tokio::time::sleep_until(deleted_due_at).await;

        let settled = describe_ok(case, &router, &[param("VpnGatewayId.1", &id)]).await;
        let gateway = one(case, &settled);
        assert_eq!(
            gateway.state,
            ec2::state::VPN_GATEWAY_STATE_DELETED,
            "case {case:?}: expected the clock-owed deleted state, got {:?}",
            gateway.state
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1 (continued): what the describe narrows by
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_vpn_gateways_narrows_by_id_and_by_every_documented_filter() {
    let case = "three gateways";
    let router = emulator();
    let first = one(
        case,
        &create_ok(
            case,
            &router,
            &{
                let mut params = vec![
                    param("AvailabilityZone", &zone_name(0)),
                    param("AmazonSideAsn", "64512"),
                ];
                params.extend(tag_params(&[("Name", "first"), ("Tier", "edge")]));
                params
            },
        )
        .await,
    )
    .id;
    let second = one(
        case,
        &create_ok(
            case,
            &router,
            &{
                let mut params = vec![param("AmazonSideAsn", "4200000000")];
                params.extend(tag_params(&[("Name", "second")]));
                params
            },
        )
        .await,
    )
    .id;
    let third = one(
        case,
        &create_ok(case, &router, &[param("AvailabilityZone", &zone_name(1))]).await,
    )
    .id;

    // The store's one stable order is by id, and the ids are minted in ascending order.
    let mut all = vec![first.clone(), second.clone(), third.clone()];
    all.sort();
    let read = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(case, &read),
        all,
        "case {case:?}: a describe naming nothing reports every gateway, in id order: {read}"
    );

    // The third gateway is deleted, so the `state` filter has more than one value to tell
    // apart.
    let _ = delete_ok(case, &router, &third).await;

    // (case, the filters, the ids that must be reported)
    let cases: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        (
            "vpn-gateway-id",
            filters(&[("vpn-gateway-id", &[second.as_str()])]),
            vec![second.clone()],
        ),
        (
            "type, which every gateway shares",
            filters(&[("type", &[ec2::state::GATEWAY_TYPE_IPSEC_1])]),
            all.clone(),
        ),
        (
            "availability-zone",
            filters(&[("availability-zone", &[zone_name(0).as_str()])]),
            vec![first.clone()],
        ),
        (
            "availability-zone, which the second gateway carries none of",
            filters(&[("availability-zone", &[zone_name(2).as_str()])]),
            Vec::new(),
        ),
        (
            "amazon-side-asn, in decimal as a client sends it",
            filters(&[("amazon-side-asn", &["4200000000"])]),
            vec![second.clone()],
        ),
        (
            "state, naming the one gateway a delete started on",
            filters(&[("state", &[ec2::state::VPN_GATEWAY_STATE_DELETING])]),
            vec![third.clone()],
        ),
        (
            "tag:Name",
            filters(&[("tag:Name", &["first"])]),
            vec![first.clone()],
        ),
        (
            "tag:Name with two values, joined with an OR",
            filters(&[("tag:Name", &["first", "second"])]),
            {
                let mut wanted = vec![first.clone(), second.clone()];
                wanted.sort();
                wanted
            },
        ),
        (
            "tag-key",
            filters(&[("tag-key", &["Tier"])]),
            vec![first.clone()],
        ),
        (
            "two filters, joined with an AND",
            filters(&[("tag-key", &["Name"]), ("amazon-side-asn", &["64512"])]),
            vec![first.clone()],
        ),
        (
            "attachment.vpc-id, which no gateway of this emulator carries",
            filters(&[("attachment.vpc-id", &["vpc-00000000000000000"])]),
            Vec::new(),
        ),
        (
            "attachment.state, for the same reason",
            filters(&[("attachment.state", &["attached"])]),
            Vec::new(),
        ),
    ];

    for (filter_case, params, wanted) in cases {
        let body = describe_ok(filter_case, &router, &params).await;
        assert_eq!(
            reported_ids(filter_case, &body),
            wanted,
            "case {filter_case:?}: expected {wanted:?}, got {body}"
        );
    }

    // A filter the operation does not document is refused rather than read as "no results".
    let (status, body) = describe(&router, &filters(&[("vpn-gateway-name", &["first"])])).await;
    assert_eq!(
        status, 400,
        "an undocumented filter must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "an undocumented filter must be refused with InvalidParameterValue, got {body}"
    );

    // An id the account holds no gateway for is an error from the describe too, not an
    // absent row.
    let (status, body) = describe(&router, &[param("VpnGatewayId.1", "vgw-00000000000000000")]).await;
    assert_eq!(
        status, 400,
        "an unknown id must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_VPN_GATEWAY_ID_NOT_FOUND,
        "an unknown id must be refused with the not-found code, got {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: DryRun changes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_changes_nothing() {
    let case = "a dry run";
    let router = emulator();

    // A create that would have succeeded.
    let (status, body) = create(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 from a dry-run create, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected the DryRunOperation refusal, got {body}"
    );
    // A 412 is answered through the same envelope, so it names its request too.
    let dry_run_create_id = request_id(case, &body);
    let read = describe_ok(case, &router, &[]).await;
    assert!(
        reported_ids(case, &read).is_empty(),
        "case {case:?}: a dry-run create must store nothing, got {read}"
    );

    // A delete that would have succeeded.
    let id = one(case, &create_ok(case, &router, &[]).await).id;
    let (status, body) = delete(&router, &id, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 from a dry-run delete, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected the DryRunOperation refusal, got {body}"
    );
    assert_distinct(
        "the two dry runs",
        &[dry_run_create_id, request_id(case, &body)],
    );
    let read = describe_ok(case, &router, &[param("VpnGatewayId.1", &id)]).await;
    let gateway = one(case, &read);
    assert_ne!(
        gateway.state,
        ec2::state::VPN_GATEWAY_STATE_DELETING,
        "case {case:?}: a dry-run delete must not start the deletion, got {read}"
    );

    // A dry run on an id the account holds no gateway for is still refused as unknown: the
    // request would NOT have succeeded.
    let (status, body) = delete(
        &router,
        "vgw-00000000000000000",
        &[param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a dry run on an unknown id, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_VPN_GATEWAY_ID_NOT_FOUND,
        "case {case:?}: expected the not-found refusal, got {body}"
    );
}
