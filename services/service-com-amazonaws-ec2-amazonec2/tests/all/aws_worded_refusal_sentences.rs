// SPDX-License-Identifier: CC0-1.0

//! The WORDING of eight families of EC2 refusal, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST carrying
//! `Action=<operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: an error message is part of the contract, so each refusal carries
//! the sentence AWS words for its error CLASS — the fixed text exactly as AWS writes it,
//! with only the parts AWS interpolates taken from the request and from the store, and
//! nothing else (no member path the class does not name, no id the caller never sent, no
//! state word the class does not carry).
//!
//! One family per test, and every table row is a distinct input along what the family
//! generalises over — which operation answers, which value the request carried, how that
//! value is wrong, and (for the planes that word one resource two ways) which plane the
//! answering operation belongs to — so no single recorded request is the specification.
//!
//! The sentences are spelled out here as literals rather than read from `ec2::state`: a
//! suite that imported the service's own constant would pass by agreeing with itself.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters};
// The seeded attachments and the request builder for the transit gateway acceptance and
// rejection state machine live in the suite that drives it — one set of fixtures, so this
// suite seeds an attachment exactly the way that one does.
use super::transit_gateway_acceptance::{
    attachment_param, invoke, pending_client_vpn_attachment, pending_vpc_attachment,
};
use super::wire::{param, Param};

/// The Region every call here is signed for. Nothing in these refusals is Region-scoped;
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// EC2's code for a request member whose value the operation cannot take.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The refusal `action` answers for `params`, asserted to be HTTP 400 `code` carrying
/// exactly `message`.
async fn assert_refusal(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
    code: &str,
    message: &str,
) {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
    );
    let answered_code = error_code(&body);
    assert_eq!(
        answered_code, code,
        "case {case:?}: expected the code {code:?} from {action}, got {answered_code:?} in \
         {body}"
    );
    let answered = error_message(&body);
    assert_eq!(
        answered, message,
        "case {case:?}: expected the message {message:?} from {action}, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// Family 1: a BYOIP address range that is not a CIDR block at all
// ---------------------------------------------------------------------------

/// Every operation that takes a BYOIP address range refuses a `Cidr` it cannot read as one
/// with the BYOIP surface's own sentence — `The CIDR (<value>) is not valid.` — whichever
/// operation was called and whichever way the value is not a range.
///
/// The rows span the operations bound to that member and the ways a value is no address
/// range: free text, a dotted quad whose octets are out of range, an IPv4 prefix length
/// past 32, an IPv6 prefix length past 128, a bare address with no prefix at all, and an
/// address range of the wrong punctuation.
#[tokio::test]
async fn every_byoip_range_operation_words_an_unreadable_cidr_the_same_way() {
    let router = emulator();

    // (the operation, the members it needs beside `Cidr`)
    let operations: [(&str, &[(&str, &str)]); 6] = [
        ("AdvertiseByoipCidr", &[]),
        ("ProvisionByoipCidr", &[]),
        ("WithdrawByoipCidr", &[]),
        ("DeprovisionByoipCidr", &[]),
        (
            "MoveByoipCidrToIpam",
            &[
                ("IpamPoolId", "ipam-pool-0123456789abcdef0"),
                ("IpamPoolOwner", "123456789012"),
            ],
        ),
        // The ASN member is read before the range, so it carries a well-formed value from
        // the block RFC 5398 sets aside for documentation.
        ("AssociateIpamByoasn", &[("Asn", "64496")]),
    ];
    let values = [
        "not-a-cidr",
        "999.999.999.0/24",
        "10.0.0.0/33",
        "2001:db8::/129",
        "203.0.113.7",
        "203.0.113.0-24",
    ];

    for (action, extra) in operations {
        for value in values {
            let case = format!("{action} with Cidr={value:?}");
            let mut params = vec![param("Cidr", value)];
            params.extend(extra.iter().map(|(name, held)| param(name, held)));
            assert_refusal(
                &case,
                &router,
                action,
                &params,
                INVALID_PARAMETER_VALUE,
                &format!("The CIDR ({value}) is not valid."),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Family 2: a pagination token the service did not mint
// ---------------------------------------------------------------------------

/// Every paged read refuses a `NextToken` it did not mint with AWS's one sentence for
/// `InvalidPaginationToken` — which has NO variable part: the token the request sent is not
/// repeated back, however long it was.
///
/// The rows span the reads (each a different store behind the same paging layer) and the
/// ways a token is not one of the service's: free text, hexadecimal that decodes to no
/// cursor of this service, a resource id a caller might guess the token is, and a token far
/// longer than any this service mints — the row that shows no part of a caller-chosen value
/// reaches the message.
#[tokio::test]
async fn every_paged_read_words_a_foreign_pagination_token_the_same_way() {
    let router = emulator();

    let actions = [
        "DescribeVpcs",
        "DescribeSubnets",
        "DescribeTags",
        "DescribeNatGateways",
        "DescribeTransitGatewayAttachments",
        "DescribeCapacityReservationTopology",
    ];
    let long_token = "z".repeat(1024);
    let tokens = [
        "not-a-token",
        "6162636465",
        "vpc-0123456789abcdef0",
        long_token.as_str(),
    ];

    for action in actions {
        for token in tokens {
            let case = format!("{action} continuing a token of {} characters", token.len());
            assert_refusal(
                &case,
                &router,
                action,
                &[param("NextToken", token)],
                "InvalidPaginationToken",
                "The passed pagination token is invalid",
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Family 3: an unsupported filter name in the transit gateway describes
// ---------------------------------------------------------------------------

/// Every `DescribeTransitGateway*` read refuses a filter name it does not document with the
/// sentence AWS's parameter layer words a rejected `Filter` with — `Value (<name>) for
/// parameter Filter is invalid. `, the name from the REQUEST and that layer's trailing
/// separating space, with no reason after it.
///
/// The rows span every read of that family and the ways a name is not one of its own: a
/// name no describe documents, a documented name in the wrong case ("Filter names are
/// case-sensitive"), a name with trailing whitespace, and a filter entry that names nothing
/// at all.
#[tokio::test]
async fn every_transit_gateway_describe_words_an_unsupported_filter_as_a_parameter() {
    let router = emulator();

    let actions = [
        "DescribeTransitGateways",
        "DescribeTransitGatewayAttachments",
        "DescribeTransitGatewayVpcAttachments",
        "DescribeTransitGatewayPeeringAttachments",
        "DescribeTransitGatewayRouteTables",
        "DescribeTransitGatewayRouteTableAnnouncements",
        "DescribeTransitGatewayMulticastDomains",
        "DescribeTransitGatewayPolicyTables",
        "DescribeTransitGatewayMeteringPolicies",
        "DescribeTransitGatewayConnects",
        "DescribeTransitGatewayConnectPeers",
    ];
    let names = ["not-a-real-filter", "State", "state ", ""];

    for action in actions {
        for name in names {
            let case = format!("{action} filtering on {name:?}");
            assert_refusal(
                &case,
                &router,
                action,
                &filters(&[(name, &["anything"])]),
                INVALID_PARAMETER_VALUE,
                &format!("Value ({name}) for parameter Filter is invalid. "),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Family 4: the two planes that word an unknown placement group
// ---------------------------------------------------------------------------

/// A placement group a request names that the account holds none of is refused
/// `InvalidPlacementGroup.Unknown` by both planes, each with ITS OWN spelling of the
/// resource: the describe plane capitalises it, the operations that act on a group word it
/// in lower case. Only the group the REQUEST named is interpolated.
///
/// The rows span the ways a group is not the account's — never created, deleted a call ago,
/// a stored name in another case — and, for the describe, both selectors a caller can name
/// a group by.
#[tokio::test]
async fn each_placement_group_plane_words_an_unknown_group_its_own_way() {
    let router = emulator();

    // A group that exists, so a refusal below is a verdict about the name in the request
    // rather than about an empty account; and one created and then deleted, so "deleted a
    // call ago" is a row of its own.
    ok(
        "a held group",
        &router,
        "CreatePlacementGroup",
        &[param("GroupName", "held"), param("Strategy", "cluster")],
    )
    .await;
    let created = ok(
        "a group to delete",
        &router,
        "CreatePlacementGroup",
        &[param("GroupName", "gone"), param("Strategy", "cluster")],
    )
    .await;
    let deleted_id = element(&created, "groupId")
        .unwrap_or_else(|| panic!("CreatePlacementGroup answered no groupId: {created}"));
    ok(
        "the delete",
        &router,
        "DeletePlacementGroup",
        &[param("GroupName", "gone")],
    )
    .await;

    // (case, the group the request names)
    let unknown: [(&str, &str); 4] = [
        ("a name nothing was created under", "nothing-here"),
        ("the name of a group deleted a call ago", "gone"),
        ("a held name in another case", "HELD"),
        ("a well-formed id nothing holds", "pg-0000000000000dead"),
    ];

    for (case, named) in unknown {
        // The acting plane: a delete names the group by name only.
        assert_refusal(
            &format!("DeletePlacementGroup for {case}"),
            &router,
            "DeletePlacementGroup",
            &[param("GroupName", named)],
            "InvalidPlacementGroup.Unknown",
            &format!("The placement group '{named}' is unknown."),
        )
        .await;
        // The describe plane, by name.
        assert_refusal(
            &format!("DescribePlacementGroups by name for {case}"),
            &router,
            "DescribePlacementGroups",
            &[param("GroupName.1", named)],
            "InvalidPlacementGroup.Unknown",
            &format!("The Placement Group '{named}' is unknown."),
        )
        .await;
    }

    // The describe plane, by the id the deleted group was minted with: the same sentence,
    // around the id the request named.
    assert_refusal(
        "DescribePlacementGroups by the deleted group's id",
        &router,
        "DescribePlacementGroups",
        &[param("GroupId.1", &deleted_id)],
        "InvalidPlacementGroup.Unknown",
        &format!("The Placement Group '{deleted_id}' is unknown."),
    )
    .await;

    // The group that was never deleted is still there, so none of the refusals above
    // changed what a later caller sees.
    let held = ok(
        "the held group",
        &router,
        "DescribePlacementGroups",
        &[param("GroupName.1", "held")],
    )
    .await;
    assert_eq!(
        element(&held, "groupName").as_deref(),
        Some("held"),
        "a refused request must leave the held group describable, got {held}"
    );
}

// ---------------------------------------------------------------------------
// Family 5: one attribute of a subnet at a time
// ---------------------------------------------------------------------------

/// `ModifySubnetAttribute` refuses a request naming two attributes with the sentence AWS
/// words that class with: the fixed text, then the attributes the request provided in
/// brackets, spelled as the WIRE spells them — the member names the caller sent, in the
/// order the request's members are declared.
///
/// The rows span pairs drawn from different attribute members, so neither the pair nor the
/// value in it is the specification.
#[tokio::test]
async fn modify_subnet_attribute_names_the_attributes_the_request_provided() {
    let router = emulator();
    let vpc = ok("the vpc", &router, "CreateVpc", &[param("CidrBlock", "10.0.0.0/16")]).await;
    let vpc_id = element(&vpc, "vpcId").unwrap_or_else(|| panic!("no <vpcId> in {vpc}"));
    let subnet = ok(
        "the subnet",
        &router,
        "CreateSubnet",
        &[param("VpcId", &vpc_id), param("CidrBlock", "10.0.1.0/24")],
    )
    .await;
    let subnet_id =
        element(&subnet, "subnetId").unwrap_or_else(|| panic!("no <subnetId> in {subnet}"));

    // (the two members as the request writes them, the names the refusal must list)
    let pairs: [(&str, &str, &str); 4] = [
        (
            "AssignIpv6AddressOnCreation",
            "MapPublicIpOnLaunch",
            "assignIpv6AddressOnCreation, mapPublicIpOnLaunch",
        ),
        (
            "MapPublicIpOnLaunch",
            "EnableDns64",
            "mapPublicIpOnLaunch, enableDns64",
        ),
        (
            "EnableDns64",
            "EnableResourceNameDnsARecordOnLaunch",
            "enableDns64, enableResourceNameDnsARecordOnLaunch",
        ),
        (
            "AssignIpv6AddressOnCreation",
            "EnableResourceNameDnsAAAARecordOnLaunch",
            "assignIpv6AddressOnCreation, enableResourceNameDnsAAAARecordOnLaunch",
        ),
    ];

    for (first, second, listed) in pairs {
        let case = format!("{first} beside {second}");
        assert_refusal(
            &case,
            &router,
            "ModifySubnetAttribute",
            &[
                param("SubnetId", &subnet_id),
                param(&format!("{first}.Value"), "true"),
                param(&format!("{second}.Value"), "true"),
            ],
            "InvalidParameterCombination",
            &format!(
                "Only one subnet attribute can be modified at a time. Provided attributes: \
                 [{listed}]"
            ),
        )
        .await;
    }

    // One attribute at a time is served, so what the rows above refuse is the SECOND
    // attribute rather than modifying at all.
    ok(
        "one attribute",
        &router,
        "ModifySubnetAttribute",
        &[
            param("SubnetId", &subnet_id),
            param("MapPublicIpOnLaunch.Value", "true"),
        ],
    )
    .await;
}

// ---------------------------------------------------------------------------
// Family 6: a volume whose I/O is already on
// ---------------------------------------------------------------------------

/// `EnableVolumeIO` on a volume whose I/O is already enabled is refused `IncorrectState`
/// with AWS's sentence for that class: the volume from the request, and that its I/O is
/// already on — no state word and no explanation of what the operation would have done.
///
/// The rows span the volume sizes and types a caller can create, so the sentence is a
/// function of the volume's id alone.
#[tokio::test]
async fn enable_volume_io_words_an_already_enabled_volume_the_same_way() {
    let router = emulator();

    // (the volume type, the size in GiB)
    let volumes: [(&str, &str); 3] = [("gp3", "40"), ("gp2", "8"), ("standard", "2")];
    for (volume_type, size) in volumes {
        let case = format!("a {volume_type} volume of {size} GiB");
        let created = ok(
            &case,
            &router,
            "CreateVolume",
            &[
                param("AvailabilityZone", &format!("{REGION}a")),
                param("VolumeType", volume_type),
                param("Size", size),
            ],
        )
        .await;
        let volume_id = element(&created, "volumeId")
            .unwrap_or_else(|| panic!("case {case:?}: no <volumeId> in {created}"));

        // A freshly created volume has its I/O enabled, so the FIRST enable is already the
        // refused one.
        assert_refusal(
            &case,
            &router,
            "EnableVolumeIO",
            &[param("VolumeId", &volume_id)],
            ec2::state::INCORRECT_STATE,
            &format!("Volume {volume_id} already has IO enabled."),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Family 7: a decision on an attachment that is not waiting for one
// ---------------------------------------------------------------------------

/// A decision on a transit gateway attachment that is not waiting for one is refused
/// `IncorrectState` with the attachment family's own sentence: the id the REQUEST named
/// followed by the fixed text, naming neither the state the attachment is in nor the state
/// that would have admitted the decision.
///
/// The rows span both kinds of attachment this surface decides (whose state catalogs spell
/// their states differently) and both decisions, so the sentence is a function of the id
/// alone.
#[tokio::test]
async fn deciding_an_attachment_that_is_not_waiting_names_only_the_attachment() {
    // (the kind, a pending attachment of it, the accept action, the reject action)
    type Kind = (
        &'static str,
        fn() -> ec2::state::TransitGatewayAttachmentView,
        &'static str,
        &'static str,
    );
    let kinds: [Kind; 2] = [
        (
            "a VPC attachment",
            pending_vpc_attachment,
            "AcceptTransitGatewayVpcAttachment",
            "RejectTransitGatewayVpcAttachment",
        ),
        (
            "a client VPN attachment",
            pending_client_vpn_attachment,
            "AcceptTransitGatewayClientVpnAttachment",
            "RejectTransitGatewayClientVpnAttachment",
        ),
    ];

    for (kind, view, accept, reject) in kinds {
        let (router, engine) = super::ec2_query::emulator();
        let attachment_id = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment(view())
            .expect("the store holds the seeded attachment");

        // The one decision the waiting attachment admits, which is what takes it out of the
        // state the refusals below are about.
        let (status, body) = invoke(&router, accept, &attachment_param(&attachment_id)).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: expected HTTP 200 from {accept}, got {status} with body {body}"
        );

        for action in [accept, reject] {
            let case = format!("{kind}: {action} once it is no longer waiting");
            let (status, body) = invoke(&router, action, &attachment_param(&attachment_id)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code,
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected IncorrectState, got {code:?} in {body}"
            );
            let message = error_message(&body);
            let expected = format!("{attachment_id} is in invalid state");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Family 8: an exclusion that names neither target
// ---------------------------------------------------------------------------

/// `CreateVpcBlockPublicAccessExclusion` naming neither target is refused `MissingParameter`
/// with AWS's sentence for that class — the two members it could have carried, in AWS's
/// order — rather than with the emulator's own either/or prose.
///
/// The rows span every exclusion mode the model publishes and the dry-run probe, which the
/// missing parameter precedes: a request that names nothing to exclude is wrong whatever
/// else it carries.
#[tokio::test]
async fn creating_an_exclusion_with_neither_target_names_both_members() {
    let router = emulator();

    // Every member of the modelled `InternetGatewayExclusionMode` catalog
    // (`ec2::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES`), each with and without the
    // permission probe.
    for mode in ec2::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES {
        for dry_run in [false, true] {
            let case = format!("mode {mode:?} with DryRun={dry_run}");
            let mut params = vec![param("InternetGatewayExclusionMode", mode)];
            if dry_run {
                params.push(param("DryRun", "true"));
            }
            assert_refusal(
                &case,
                &router,
                "CreateVpcBlockPublicAccessExclusion",
                &params,
                "MissingParameter",
                "One of the following parameters must be provided: VpcId, SubnetId",
            )
            .await;
        }
    }
}
