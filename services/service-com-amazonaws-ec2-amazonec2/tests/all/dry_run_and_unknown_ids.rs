// SPDX-License-Identifier: CC0-1.0

//! The permission contract and the not-found boundary of the transit gateway attachment,
//! VPC endpoint and BYOIP surfaces, driven over the wire the service actually speaks:
//! `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! Three rules, each asserted over the WHOLE surface rather than over one recorded request:
//!
//! 1. An operation that models a `DryRun` member answers an authorized `DryRun=true` call
//!    with `DryRunOperation` and performs nothing — "checks whether you have the required
//!    permissions for the action, WITHOUT ACTUALLY MAKING THE REQUEST … If you have the
//!    required permissions, the error response is DryRunOperation" — so the answer does not
//!    depend on whether the resources the request names exist. WHERE the flag sits among an
//!    operation's own checks is a fact of that operation, recorded per operation, and one
//!    operation of this surface is recorded resolving the single resource its call is scoped
//!    to BEFORE it reads the flag; each row carries the answer its own operation gives, and
//!    [`super::dry_run_verdict_and_ordering`] is where that ordering itself is read.
//! 2. A well-formed id the account holds no resource for is refused with THAT resource's
//!    own `NotFound` code.
//! 3. A value that is not an id of that kind at all is refused with its `Malformed` code,
//!    before the lookup.
//!
//! Every row of the table below is one operation of the surface, so no single operation is
//! the specification; a row carries the ids and the codes of its own resources.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// The HTTP status a refused REQUEST carries.
const REFUSAL_STATUS: u16 = 400;

/// What an authorized dry run is answered with on an operation that answers the flag before
/// it resolves anything: the flag's own verdict.
const DRY_RUN_VERDICT: (u16, &str) = (DRY_RUN_STATUS, ec2::state::DRY_RUN_OPERATION);

/// A long-form EC2 id suffix — 17 lowercase hex digits — that nothing in this store mints,
/// so an id built from it is well formed and names no resource.
const UNKNOWN_SUFFIX: &str = "00000000000000000";

/// A BYOIP address range no account here has provisioned.
const UNPROVISIONED_CIDR: &str = "203.0.113.0/24";

/// One operation of the surface, with the request members that name only unknown resources
/// and the codes it refuses each kind of bad name with.
struct Operation {
    /// The ec2Query `Action`.
    action: &'static str,
    /// Members naming well-formed ids that name no resource of the account.
    unknown: Vec<(String, String)>,
    /// The code the `unknown` call is refused with.
    unknown_code: &'static str,
    /// The same request with one member carrying a value that is not an id of its kind.
    malformed: Vec<(String, String)>,
    /// The code the `malformed` call is refused with.
    malformed_code: &'static str,
    /// The status and code a DRY RUN of the `unknown` request is answered with: the flag's
    /// own verdict, unless this operation is recorded resolving what it is scoped to first.
    dry_run_answer: (u16, &'static str),
}

/// One `(name, value)` request member.
fn member(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A well-formed id of the kind `prefix` names that no resource of this store has.
fn unknown_id(prefix: &str) -> String {
    format!("{prefix}-{UNKNOWN_SUFFIX}")
}

/// The ten operations of this surface: the four acceptances and the rejection of a transit
/// gateway attachment, the acceptance of a multicast domain association, the delete of a
/// route table announcement, the acceptance of a VPC endpoint connection, the describe of a
/// VPC endpoint, and the two BYOIP operations.
fn operations() -> Vec<Operation> {
    let attachment = unknown_id(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX);
    let domain = unknown_id(ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX);
    let announcement = unknown_id(ec2::state::TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX);
    let service = unknown_id(ec2::state::VPC_ENDPOINT_SERVICE_ID_PREFIX);
    let endpoint = unknown_id(ec2::state::VPC_ENDPOINT_ID_PREFIX);
    let pool = unknown_id(ec2::state::IPAM_POOL_ID_PREFIX);
    let account = engine::emulator_account_id().to_string();

    // The attachment family: one member, one pair of codes, four operations.
    let attachment_family = [
        "AcceptTransitGatewayClientVpnAttachment",
        "AcceptTransitGatewayPeeringAttachment",
        "AcceptTransitGatewayVpcAttachment",
        "RejectTransitGatewayVpcAttachment",
    ];
    let mut operations: Vec<Operation> = attachment_family
        .into_iter()
        .map(|action| Operation {
            action,
            unknown: vec![member("TransitGatewayAttachmentId", &attachment)],
            unknown_code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
            // A value with the right prefix but a suffix that is not hex is not an id.
            malformed: vec![member("TransitGatewayAttachmentId", "tgw-attach-not-hex")],
            malformed_code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            dry_run_answer: DRY_RUN_VERDICT,
        })
        .collect();

    operations.push(Operation {
        action: "AcceptTransitGatewayMulticastDomainAssociations",
        unknown: vec![
            member("TransitGatewayMulticastDomainId", &domain),
            member("TransitGatewayAttachmentId", &attachment),
        ],
        // Both ids are unknown here, and the one the caller is told about is the ATTACHMENT:
        // the acceptance acts on the attachment whose pending associations it accepts, and
        // the multicast domain is the scope it accepts them into, so the subject is resolved
        // first — reporting the scope would send a client to fix a parameter that may well be
        // right and never mention the one that is wrong.
        unknown_code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        malformed: vec![
            member("TransitGatewayMulticastDomainId", "tgw-mcast-domain-xyz"),
            member("TransitGatewayAttachmentId", &attachment),
        ],
        malformed_code: ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
        dry_run_answer: DRY_RUN_VERDICT,
    });
    operations.push(Operation {
        action: "DeleteTransitGatewayRouteTableAnnouncement",
        unknown: vec![member(
            "TransitGatewayRouteTableAnnouncementId",
            &announcement,
        )],
        unknown_code: ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND,
        malformed: vec![member(
            "TransitGatewayRouteTableAnnouncementId",
            // An id of another resource of the same family is no announcement id.
            &attachment,
        )],
        malformed_code: ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_MALFORMED,
        dry_run_answer: DRY_RUN_VERDICT,
    });
    operations.push(Operation {
        action: "AcceptVpcEndpointConnections",
        unknown: {
            let mut members = vec![member("ServiceId", &service)];
            members.extend(indexed("VpcEndpointId", &[endpoint.as_str()]));
            members
        },
        unknown_code: ec2::state::INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND,
        malformed: {
            // An ENDPOINT id is not a SERVICE id, though both begin `vpce-`.
            let mut members = vec![member("ServiceId", &endpoint)];
            members.extend(indexed("VpcEndpointId", &[endpoint.as_str()]));
            members
        },
        malformed_code: ec2::state::INVALID_VPC_ENDPOINT_SERVICE_ID_MALFORMED,
        // The one operation of this surface recorded resolving the service its call is scoped
        // to BEFORE it reads the flag: a dry run naming a service the account does not hold
        // is told so rather than that the request would have succeeded.
        dry_run_answer: (
            REFUSAL_STATUS,
            ec2::state::INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND,
        ),
    });
    operations.push(Operation {
        action: "DescribeVpcEndpoints",
        unknown: indexed("VpcEndpointId", &[endpoint.as_str()]),
        unknown_code: ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
        // A SERVICE id is not an ENDPOINT id, the other way round.
        malformed: indexed("VpcEndpointId", &[service.as_str()]),
        malformed_code: ec2::state::INVALID_VPC_ENDPOINT_ID_MALFORMED,
        dry_run_answer: DRY_RUN_VERDICT,
    });
    operations.push(Operation {
        action: "AdvertiseByoipCidr",
        unknown: vec![member("Cidr", UNPROVISIONED_CIDR)],
        // A BYOIP range is named by its CIDR rather than by a resource id, and EC2's code
        // for a range the account has not brought to AWS is `InvalidCidr.NotFound`.
        unknown_code: ec2::state::INVALID_CIDR_NOT_FOUND,
        malformed: vec![member("Cidr", "not-a-cidr")],
        // A value that is not an address range at all is not a range that was not found:
        // there is nothing to look up, so the VALUE is what is refused.
        malformed_code: ec2::state::INVALID_PARAMETER_VALUE,
        dry_run_answer: DRY_RUN_VERDICT,
    });
    operations.push(Operation {
        action: "MoveByoipCidrToIpam",
        unknown: vec![
            member("Cidr", UNPROVISIONED_CIDR),
            member("IpamPoolId", &pool),
            member("IpamPoolOwner", &account),
        ],
        unknown_code: ec2::state::INVALID_IPAM_POOL_ID_NOT_FOUND,
        malformed: vec![
            member("Cidr", UNPROVISIONED_CIDR),
            member("IpamPoolId", "ipam-pool-not-hex"),
            member("IpamPoolOwner", &account),
        ],
        malformed_code: ec2::state::INVALID_IPAM_POOL_ID_MALFORMED,
        dry_run_answer: DRY_RUN_VERDICT,
    });
    operations
}

/// `params` with `DryRun=true` appended.
fn with_dry_run(params: &[(String, String)]) -> Vec<(String, String)> {
    let mut with_flag = params.to_vec();
    with_flag.push(member("DryRun", "true"));
    with_flag
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Rule 1, over every operation of the surface: an authorized dry run is answered with
/// `DryRunOperation` whether or not the resources named exist — except on the one operation
/// recorded resolving what its call is scoped to first, which answers that resource's own
/// refusal.
#[tokio::test]
async fn every_operation_answers_an_authorized_dry_run_with_dry_run_operation() {
    let (router, _engine) = super::ec2_query::emulator();

    for operation in operations() {
        let case = operation.action;
        let (expected_status, expected_code) = operation.dry_run_answer;
        let (status, body) = invoke(&router, case, &with_dry_run(&operation.unknown)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

/// Rule 2, over every operation of the surface: a well-formed id naming no resource is
/// refused with that resource's own code — never with a 501 and never as a success.
#[tokio::test]
async fn every_operation_refuses_an_unknown_resource_with_its_own_not_found_code() {
    let (router, _engine) = super::ec2_query::emulator();

    for operation in operations() {
        let case = operation.action;
        let (status, body) = invoke(&router, case, &operation.unknown).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            operation.unknown_code,
            "case {case:?}: expected {expected:?}, got {body}",
            expected = operation.unknown_code
        );
    }
}

/// Rule 3, over every operation of the surface: a value that is not an id of its kind is
/// refused as malformed, so a client can tell "not an id of this kind" from "no such
/// resource".
#[tokio::test]
async fn every_operation_refuses_a_value_that_is_not_an_id_of_its_kind() {
    let (router, _engine) = super::ec2_query::emulator();

    for operation in operations() {
        let case = operation.action;
        let (status, body) = invoke(&router, case, &operation.malformed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            operation.malformed_code,
            "case {case:?}: expected {expected:?}, got {body}",
            expected = operation.malformed_code
        );
    }
}

/// A malformed request is refused with ITS OWN reason even when `DryRun` is set: the flag
/// asks whether the caller is permitted, which is not a question about a value the service
/// cannot read as an id.
#[tokio::test]
async fn a_dry_run_of_a_malformed_request_is_refused_with_its_own_reason() {
    let (router, _engine) = super::ec2_query::emulator();

    for operation in operations() {
        let case = operation.action;
        let (status, body) = invoke(&router, case, &with_dry_run(&operation.malformed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            operation.malformed_code,
            "case {case:?}: expected {expected:?}, got {body}",
            expected = operation.malformed_code
        );
    }
}

/// One row of the list-cap table: the operation, the list member it takes, the values to
/// fill that member from, the members the request needs beside it, and the code it answers
/// when the list is within the cap.
type ListCapCase<'a> = (
    &'static str,
    &'static str,
    &'a [String],
    Vec<(String, String)>,
    &'static str,
);

/// A caller-chosen id list is CAPPED before any per-item work: one id past the ceiling the
/// rest of this crate uses is refused with `InvalidParameterValue` naming the maximum, while
/// a list AT the ceiling is let through to the ordinary resolution — so the cap bounds the
/// work one request can ask for without narrowing what a legitimate request may name.
#[tokio::test]
async fn a_caller_chosen_id_list_is_capped_before_any_per_item_work() {
    let (router, _engine) = super::ec2_query::emulator();
    let maximum = ec2::state::MAX_RESOURCE_IDS;

    // One well-formed id per slot, all distinct, so nothing but the LENGTH decides.
    let ids: Vec<String> = (0..=maximum)
        .map(|index| format!("{}-{index:017x}", ec2::state::VPC_ENDPOINT_ID_PREFIX))
        .collect();
    let subnets: Vec<String> = (0..=maximum)
        .map(|index| format!("subnet-{index:017x}"))
        .collect();
    let service = unknown_id(ec2::state::VPC_ENDPOINT_SERVICE_ID_PREFIX);
    let domain = unknown_id(ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX);
    let attachment = unknown_id(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX);

    // Rows: each operation of this surface that takes a caller-chosen list, with the list
    // member it takes, the base members it needs beside it, and the code it answers when
    // the list is WITHIN the cap (its own unknown-resource refusal).
    let cases: Vec<ListCapCase<'_>> = vec![
        (
            "DescribeVpcEndpoints",
            "VpcEndpointId",
            &ids,
            Vec::new(),
            ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
        ),
        (
            "AcceptVpcEndpointConnections",
            "VpcEndpointId",
            &ids,
            vec![member("ServiceId", &service)],
            ec2::state::INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND,
        ),
        (
            "AcceptTransitGatewayMulticastDomainAssociations",
            "SubnetIds",
            &subnets,
            vec![
                member("TransitGatewayMulticastDomainId", &domain),
                member("TransitGatewayAttachmentId", &attachment),
            ],
            // The attachment is the resource the acceptance acts on, so it is the id this
            // operation resolves first; the domain is the scope it acts in.
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        ),
    ];

    for (action, list_member, values, base, within_cap_code) in cases {
        for (length, expected_code) in [
            (maximum, within_cap_code),
            (maximum + 1, ec2::state::INVALID_PARAMETER_VALUE),
        ] {
            let case = format!("{action} with {length} {list_member} values");
            let borrowed: Vec<&str> = values
                .iter()
                .take(length)
                .map(String::as_str)
                .collect();
            let mut params = base.clone();
            params.extend(indexed(list_member, &borrowed));
            let (status, body) = invoke(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code:?}, got {body}"
            );
        }
    }
}

/// A filter VALUE is bounded too: a capped number of comparisons is not capped work, the
/// matcher's cost being the product of the pattern's length and the value's.
#[tokio::test]
async fn an_oversize_filter_value_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    let oversize = "a".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);
    let within = "a".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    for (case, value, expected_status) in [
        ("at the maximum", within.as_str(), 200),
        ("one byte past it", oversize.as_str(), 400),
    ] {
        let (status, body) = invoke(
            &router,
            "DescribeVpcEndpoints",
            &super::ec2_query::filters(&[("vpc-endpoint-id", &[value])]),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_PARAMETER_VALUE,
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

type OmittedMemberCase = (&'static str, Vec<(String, String)>, &'static str);

/// A request that omits a member the operation needs is refused, not answered with an empty
/// success: the result of an acceptance names one domain, one attachment and its subnets, so
/// there is no answer to give when the request names neither resource. A member the MODEL
/// marks `@required` is refused by the boundary validator before the handler runs, with the
/// boundary's own code; one the model leaves optional and the operation still needs is
/// refused by the handler with EC2's own `MissingParameter`.
#[tokio::test]
async fn a_member_the_operation_needs_and_the_request_omits_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    let attachment = unknown_id(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX);
    let domain = unknown_id(ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX);
    let service = unknown_id(ec2::state::VPC_ENDPOINT_SERVICE_ID_PREFIX);
    let cases: Vec<OmittedMemberCase> = vec![
        (
            "AcceptTransitGatewayMulticastDomainAssociations",
            vec![member("TransitGatewayAttachmentId", &attachment)],
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "AcceptTransitGatewayMulticastDomainAssociations",
            vec![member("TransitGatewayMulticastDomainId", &domain)],
            ec2::state::MISSING_PARAMETER,
        ),
        // `VpcEndpointIds` is `@required`, so the boundary validator is what refuses a
        // request that carries none — before this operation's handler is reached at all, and
        // with the boundary's own code rather than the one a handler answers for a parameter
        // the request left out.
        (
            "AcceptVpcEndpointConnections",
            vec![member("ServiceId", &service)],
            "MalformedInput",
        ),
    ];

    for (action, params, expected) in cases {
        let case = format!("{action} with {params:?}");
        let (status, body) = invoke(&router, action, &params).await;
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
}
