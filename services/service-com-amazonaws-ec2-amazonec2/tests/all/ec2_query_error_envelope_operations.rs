// SPDX-License-Identifier: CC0-1.0

//! The ec2Query error envelope, driven over SEVEN FURTHER OPERATIONS and the four refusal
//! kinds they answer — the across-operations half of the rule
//! [`super::ec2_query_error_envelope`] owns.
//!
//! The rule under test: EVERY REFUSAL THIS SERVICE RENDERS CLOSES ITS `<Response>` ERROR
//! ENVELOPE WITH ONE `<RequestID>` NAMING THE CALL IT ANSWERS, AND DECLARES THE SAME MEDIA
//! TYPE, whichever operation answered and whichever refusal it is. The element belongs to
//! the ENVELOPE — the EC2 API Reference's "Error response" is
//! `<Response><Errors><Error><Code/><Message/></Error></Errors><RequestID/></Response>` —
//! so no operation may be an exception to it, which is exactly what rows across unrelated
//! operations pin and what rows on one operation cannot.
//!
//! What is NOT asserted is a particular id: the value names ONE call, so a recorded id is
//! an example of its form and never a value to match. What is asserted is the form (the
//! 8-4-4-4-12 lowercase hexadecimal an AWS request id takes), that there is exactly one of
//! it, that it closes the envelope, and that no two responses report the same one — read
//! through that suite's own [`super::ec2_query_error_envelope::refusal_request_id`] rather
//! than through a second reader declared here.
//!
//! The rows span what the rule generalises over:
//!
//! * WHICH OPERATION answered: a get of an account setting, an address association, a
//!   describe of local gateway associations, an instance launch, a placement-score
//!   calculation, a describe of volume modifications, and a transit gateway propagation
//!   disable — operations that share no resource, no state and no handler;
//! * WHICH REFUSAL it is: a `DryRun` the caller asked for (the 412 answer, where the
//!   request would otherwise have succeeded), a well-formed id the account holds no
//!   resource for, a value that is not an id of its kind at all, a parameter value the
//!   operation does not define, and a request that names no parameter at all;
//! * WHICH STATUS it carries: 412 for the precondition the caller set, 400 for the rest;
//! * WHERE in the handler the refusal is decided: before anything is resolved
//!   (`DisableTransitGatewayRouteTablePropagation` answers `DryRun` first) and after every
//!   named resource has been (`AssociateAddress` and `DescribeVolumesModifications` refuse
//!   an unresolved id ahead of the flag) — so the element cannot depend on how far a call
//!   got.
//!
//! One further row set ([`echo_rows`]) sends a value carrying every XML metacharacter into
//! three of those refusals: an envelope is XML, so a request value a message quotes back is
//! character data and the envelope the client parses is still the envelope.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{call_with_content_type, error_code, filters, indexed, unescaped};
use super::ec2_query_error_envelope::refusal_request_id;
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The media type every EC2 ec2Query response declares, refusals included.
///
/// Recorded behaviour: the model declares the protocol but not the media type's `charset`
/// parameter, where the protocol's own default is the bare `text/xml`.
const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// The status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// A long-form EC2 id suffix — 17 lowercase hexadecimal digits — that nothing in this store
/// mints, so an id built from it is well formed and names no resource of the account.
const UNKNOWN_SUFFIX: &str = "00000000000000000";

/// A well-formed id of the kind `prefix` names that no resource of this store has.
fn unknown_id(prefix: &str) -> String {
    format!("{prefix}-{UNKNOWN_SUFFIX}")
}

/// A VOLUME id inside the form EC2 ISSUES that no volume of this store has.
///
/// A volume id is read against the issued form rather than the printed one
/// (`ec2::state::volume_id_is_well_formed`): the long form carries 68 digits' worth of a
/// 64-bit identifier, so an issued id reads `vol-0…` and is never the all-zero placeholder
/// [`UNKNOWN_SUFFIX`] is. So this names a volume the account does not hold, where that
/// placeholder would be a value EC2 cannot read at all.
fn unknown_volume_id() -> String {
    format!("{}-00000000000000999", ec2::state::VOLUME_ID_PREFIX)
}

/// The Elastic IP allocation id prefix. Spelled out rather than taken from `ec2::state`,
/// which publishes the id's FORM (`allocation_id_is_well_formed`) and not its prefix.
const ALLOCATION_ID_PREFIX: &str = "eipalloc";

/// The instance id prefix, for the same reason.
const INSTANCE_ID_PREFIX: &str = "i";

/// An AMI id of the form EC2 ISSUES (`ami-` plus 17 hexadecimal digits under a leading
/// `0`) — enough for the launch rows, each of which is refused before any image is
/// resolved, and inside the form `RunInstances` reads an `ImageId` for before it looks one
/// up (`ec2::state::issued_image_id_is_well_formed`).
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// A target capacity inside the member's published `@range` (1..2000000000, from
/// `smithy_query(query="operation",operation="GetSpotPlacementScores")`), so the rows below
/// are refused by the handler rather than by the boundary validator in front of it.
const TARGET_CAPACITY: &str = "5";

/// One row: what it calls, with which form fields, and the status and `<Code>` this service
/// refuses it with.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// Every refusal row: the seven operations, each with the refusals it can answer.
fn refusal_rows() -> Vec<Row> {
    let allocation = unknown_id(ALLOCATION_ID_PREFIX);
    let instance = unknown_id(INSTANCE_ID_PREFIX);
    let volume = unknown_volume_id();
    let route_table = unknown_id(ec2::state::TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX);
    let attachment = unknown_id(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX);

    vec![
        Row {
            // `DryRun` is this operation's ONLY member, so the flag is the whole request:
            // the refusal is decided before anything of the account is read.
            case: "an account setting asked about as a DryRun",
            action: "GetAllowedImagesSettings",
            params: vec![param("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Row {
            case: "an association of an address the account does not hold",
            action: "AssociateAddress",
            params: vec![
                param("AllocationId", &allocation),
                param("InstanceId", &instance),
            ],
            status: 400,
            code: ec2::state::INVALID_ALLOCATION_ID_NOT_FOUND,
        },
        Row {
            case: "an association naming something that is not an allocation id",
            action: "AssociateAddress",
            params: vec![
                param("AllocationId", "eipalloc-not-hex"),
                param("InstanceId", &instance),
            ],
            status: 400,
            code: ec2::state::INVALID_ALLOCATION_ID_MALFORMED,
        },
        Row {
            case: "an association that names no address at all",
            action: "AssociateAddress",
            params: vec![param("InstanceId", &instance)],
            status: 400,
            code: ec2::state::MISSING_PARAMETER,
        },
        Row {
            case: "a describe of local gateway group associations asked for as a DryRun",
            action: "DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations",
            params: vec![param("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Row {
            case: "a describe of local gateway group associations narrowed by a filter name \
                   the operation does not define",
            action: "DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations",
            params: filters(&[("not-a-filter", &["any"])]),
            status: 400,
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        Row {
            case: "an instance launch asked for as a DryRun",
            action: "RunInstances",
            params: vec![
                param("ImageId", IMAGE_ID),
                param("MinCount", "1"),
                param("MaxCount", "1"),
                param("DryRun", "true"),
            ],
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Row {
            case: "an instance launch whose maximum count is below its minimum",
            action: "RunInstances",
            params: vec![
                param("ImageId", IMAGE_ID),
                param("MinCount", "2"),
                param("MaxCount", "1"),
            ],
            status: 400,
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        Row {
            case: "a placement-score calculation asked for as a DryRun",
            action: "GetSpotPlacementScores",
            params: vec![
                param("TargetCapacity", TARGET_CAPACITY),
                param("DryRun", "true"),
            ],
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Row {
            case: "a placement-score calculation naming an instance type nothing offers",
            action: "GetSpotPlacementScores",
            params: {
                let mut params = vec![param("TargetCapacity", TARGET_CAPACITY)];
                params.extend(indexed("InstanceType", &["not-an-instance-type"]));
                params
            },
            status: 400,
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        Row {
            case: "a describe of the modifications of a volume the account does not hold",
            action: "DescribeVolumesModifications",
            params: indexed("VolumeId", &[&volume]),
            status: 400,
            code: ec2::state::INVALID_VOLUME_ID_NOT_FOUND,
        },
        Row {
            case: "a describe of modifications naming something that is not a volume id",
            action: "DescribeVolumesModifications",
            params: indexed("VolumeId", &["vol-not-hex"]),
            status: 400,
            code: ec2::state::INVALID_VOLUME_ID_MALFORMED,
        },
        Row {
            case: "a describe of modifications asked for as a DryRun",
            action: "DescribeVolumesModifications",
            params: vec![param("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Row {
            case: "a propagation disable on a route table the account does not hold",
            action: "DisableTransitGatewayRouteTablePropagation",
            params: vec![
                param("TransitGatewayRouteTableId", &route_table),
                param("TransitGatewayAttachmentId", &attachment),
            ],
            status: 400,
            code: ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        },
        Row {
            case: "a propagation disable naming something that is not a route table id",
            action: "DisableTransitGatewayRouteTablePropagation",
            params: vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-not-hex"),
                param("TransitGatewayAttachmentId", &attachment),
            ],
            status: 400,
            code: ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        },
        Row {
            case: "a propagation disable asked for as a DryRun",
            action: "DisableTransitGatewayRouteTablePropagation",
            params: vec![
                param("TransitGatewayRouteTableId", &route_table),
                param("DryRun", "true"),
            ],
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
    ]
}

/// A value a refusal QUOTES BACK, carrying every character that is markup in XML.
///
/// The envelope is XML, so a request value echoed into a `<Message>` reaches the wire as
/// character data and not as markup: the id a caller sent is reported back whatever bytes it
/// carried, and the envelope the client parses is still the envelope — one `<Errors><Error>`
/// and one closing `<RequestID>`.
const METACHARACTER_VALUE: &str = "not-an-id-<&\">]]>";

/// One row whose refusal quotes [`METACHARACTER_VALUE`] back: the member that carries it
/// and the code the operation refuses it with.
struct EchoRow {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
    code: &'static str,
}

/// Three operations, three different members: an id resolved as malformed, a filter name the
/// operation does not define, and an instance type nothing offers — three places a refusal
/// of this service quotes a request value back.
fn echo_rows() -> Vec<EchoRow> {
    vec![
        EchoRow {
            case: "an allocation id that is not an id at all",
            action: "AssociateAddress",
            params: vec![
                param("AllocationId", METACHARACTER_VALUE),
                param("InstanceId", &unknown_id(INSTANCE_ID_PREFIX)),
            ],
            code: ec2::state::INVALID_ALLOCATION_ID_MALFORMED,
        },
        EchoRow {
            case: "a filter name the operation does not define",
            action: "DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations",
            params: filters(&[(METACHARACTER_VALUE, &["any"])]),
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        EchoRow {
            case: "an instance type nothing offers",
            action: "GetSpotPlacementScores",
            params: {
                let mut params = vec![param("TargetCapacity", TARGET_CAPACITY)];
                params.extend(indexed("InstanceType", &[METACHARACTER_VALUE]));
                params
            },
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
    ]
}

/// A refused value the message quotes back reaches the wire as character data, so the
/// envelope a client parses is intact and the value it reads back is the one it sent.
#[tokio::test]
async fn a_refused_value_is_quoted_back_as_data_rather_than_as_markup() {
    let (router, _engine) = super::ec2_query::emulator();
    for row in echo_rows() {
        let (status, content_type, body) =
            call_with_content_type(&router, REGION, row.action, &row.params).await;
        let case = format!("{} refused for {}", row.action, row.case);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            &error_code(&body),
            row.code,
            "case {case:?}: expected the code {}, got {body}",
            row.code
        );
        assert_eq!(
            content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the media type {RESPONSE_CONTENT_TYPE:?}, got \
             {content_type:?} with body {body}"
        );
        // The parse is the whole point: a value written as markup would either break the
        // document or add elements of its own, and `refusal_request_id` reads the envelope
        // through the parser — one `<Errors><Error>` with this code, a non-empty
        // `<Message>`, and one `<RequestID>` closing it.
        refusal_request_id(&case, &body, row.code);
        assert!(
            !body.contains(METACHARACTER_VALUE),
            "case {case:?}: the quoted value must reach the wire escaped, not as markup, \
             got {body}"
        );
        assert!(
            unescaped(&body).contains(METACHARACTER_VALUE),
            "case {case:?}: the refusal must quote back the value that was sent, got {body}"
        );
    }
}

/// Every one of these refusals reports the request it answers in one `<RequestID>` and
/// declares the envelope's media type — and no two of them report the same id, because the
/// id names ONE call.
///
/// Every row runs against ONE emulator, so the ids are compared across operations and not
/// only within one: a per-operation counter would pass a within-operation check and fail
/// this one.
#[tokio::test]
async fn every_refusal_of_these_operations_reports_its_own_request_id() {
    let (router, _engine) = super::ec2_query::emulator();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in refusal_rows() {
        let (status, content_type, body) =
            call_with_content_type(&router, REGION, row.action, &row.params).await;
        let case = format!("{} refused for {}", row.action, row.case);
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            &error_code(&body),
            row.code,
            "case {case:?}: expected the code {}, got {body}",
            row.code
        );
        assert_eq!(
            content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the media type {RESPONSE_CONTENT_TYPE:?}, got \
             {content_type:?} with body {body}"
        );
        let request_id = refusal_request_id(&case, &body, row.code);
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: the request id {request_id:?} was already reported for another \
             response"
        );
    }
}

/// Repeating one of these calls is a second call, so it is answered under a request id of
/// its own — the id is minted per RESPONSE and is nothing read off the request, which is
/// what makes it usable to tell two identical failures apart.
#[tokio::test]
async fn an_identical_refusal_of_these_operations_reports_a_second_request_id() {
    let (router, _engine) = super::ec2_query::emulator();
    for row in refusal_rows() {
        let case = format!("{} refused twice for {}", row.action, row.case);
        let (first_status, _, first) =
            call_with_content_type(&router, REGION, row.action, &row.params).await;
        let (second_status, _, second) =
            call_with_content_type(&router, REGION, row.action, &row.params).await;
        assert_eq!(
            (first_status, second_status),
            (row.status, row.status),
            "case {case:?}: both calls must be refused with HTTP {}, got {first_status} and \
             {second_status} with bodies {first} / {second}",
            row.status
        );
        let first_id = refusal_request_id(&case, &first, row.code);
        let second_id = refusal_request_id(&case, &second, row.code);
        assert_ne!(
            first_id, second_id,
            "case {case:?}: each response reports its own request id, got {first_id:?} twice"
        );
    }
}
