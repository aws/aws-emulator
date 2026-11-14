// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the request id EC2's `ec2Query` envelope reports the answered
//! request in — a part of a response that belongs to the PROTOCOL rather than to any one
//! operation, so it is driven over the wire through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rule under test: EVERY RESPONSE THIS SERVICE RENDERS NAMES THE REQUEST IT ANSWERS.
//! A refusal names it in exactly one `<RequestID>`, the last child of the `<Response>`
//! error root — the shape the EC2 API Reference's "Error response" example shows:
//!
//! ```text
//! <Response><Errors><Error><Code/><Message/></Error></Errors>
//!   <RequestID>ea966190-f9aa-478e-9ede-cb5432daacc0</RequestID></Response>
//! ```
//!
//! while a success names it in the lowercase `<requestId>` of its own envelope. The value
//! identifies ONE call, so each response carries a fresh one, in the 8-4-4-4-12 lowercase
//! hexadecimal form an AWS request id takes, rather than anything read off the request —
//! and the rest of the envelope, the code and the message a client branches on, is left
//! exactly as it was. A recorded id is an example of that form, never a value to match.
//!
//! The rule itself is served by `Ec2QueryEnvelope` in this crate's `src/service.rs`, the
//! one place this service chooses its response envelope; what these rows add is the cover
//! ACROSS operations and refusal kinds, which is what makes the element the envelope's
//! rather than one operation's.
//!
//! The rows span what that rule generalises over: which operation answered, which kind of
//! refusal it is (an id that is not an id of its kind, an id that resolves to nothing, a
//! parameter the request left out, a parameter value the operation does not define, a
//! `DryRun` that would have succeeded, a mutation that is already done), which HTTP status
//! that refusal carries (400 and 412), and whether the operation's modelled output is
//! `smithy.api#Unit` or a result shape of its own — and, in [`framework_rows`], WHICH
//! LAYER refused: a request that never reaches an operation (it names an action this
//! service has none of, or none at all, or declares a transfer framing its body does not
//! carry) is answered by the same envelope, because the envelope is the service's and not
//! one operation's. The one success row pins the lowercase
//! element a 200 carries instead. The scaffold's 501 is rendered by the same
//! `serialize_error`, so it carries the element by construction; no row names a scaffolded
//! operation, because the operation this suite would have to name is one some other issue
//! implements next — the `CreateVpnConnectionRoute` rows below are an operation that WAS a
//! scaffold and is one no longer, which is what pins that an operation added to this
//! service answers this envelope rather than a body of its own.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, filters, unescaped};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The root element of the ec2Query ERROR envelope.
pub(crate) const ERROR_ENVELOPE_ROOT: &str = "Response";

/// The element a refusal reports the answered request in (`RequestID`), and the one a
/// success reports it in (`requestId`). XML is case-sensitive, so these are two distinct
/// elements and a response carries exactly one of them.
pub(crate) const ERROR_REQUEST_ID: &str = "RequestID";
pub(crate) const SUCCESS_REQUEST_ID: &str = "requestId";

/// A well-formed VPC id in the long (17 hexadecimal digit) form that the account holds no
/// VPC for.
const UNKNOWN_VPC_ID: &str = "vpc-0ffffffffffffffff";

/// A well-formed VPN connection id the account holds no connection for — enough for the
/// `CreateVpnConnectionRoute` rows, each of which is refused before anything is written.
const UNKNOWN_VPN_CONNECTION_ID: &str = "vpn-00000000000000001";

/// A well-formed transit gateway route table id that names no route table — enough for the
/// rows refused before any route table is resolved.
const UNRESOLVED_ROUTE_TABLE_ID: &str = "tgw-rtb-0000000000000000f";

/// A well-formed prefix list id for the same rows.
const PREFIX_LIST_ID: &str = "pl-00000000000000ef1";

/// The media type every EC2 ec2Query response declares.
///
/// Recorded behaviour: the model declares the protocol but not the media type's `charset`
/// parameter, where the protocol's own default is the bare `text/xml`.
///
/// `pub(crate)` for the same reason as [`refusal_request_id`]: a suite that brings ONE
/// operation under this rule states the media type by naming this one rather than by
/// declaring a second copy of it.
pub(crate) const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// The `content-encoding` an AWS SDK sets to announce `aws-chunked` transfer framing — the
/// header that makes the emulator decode the body as framed, and so the one that lets a
/// body be refused as undecodable framing before any action is resolved.
const AWS_CHUNKED: &[(&str, &str)] = &[("content-encoding", "aws-chunked")];

/// One `action` call signed for the Region above, as its status and body.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One row: what it calls, with which fields, and the status and `<Code>` the service
/// refuses it with.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// Every refusal row. Each is a DIFFERENT way a request fails, across five operations.
fn refusal_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a transit gateway route table id that is not an id of that kind",
            action: "CreateTransitGatewayPrefixListReference",
            params: vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-0123456789abcde"),
                param("PrefixListId", PREFIX_LIST_ID),
                param("Blackhole", "true"),
            ],
            status: 400,
            code: ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        },
        Row {
            case: "a prefix list reference that says nothing about where traffic goes",
            action: "CreateTransitGatewayPrefixListReference",
            params: vec![
                param("TransitGatewayRouteTableId", UNRESOLVED_ROUTE_TABLE_ID),
                param("PrefixListId", PREFIX_LIST_ID),
            ],
            status: 400,
            code: ec2::state::MISSING_PARAMETER,
        },
        Row {
            case: "a prefix list reference asked for as a DryRun",
            action: "CreateTransitGatewayPrefixListReference",
            params: vec![
                param("TransitGatewayRouteTableId", UNRESOLVED_ROUTE_TABLE_ID),
                param("PrefixListId", PREFIX_LIST_ID),
                param("Blackhole", "true"),
                param("DryRun", "true"),
            ],
            status: 412,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Row {
            case: "a describe of a VPC the account does not hold",
            action: "DescribeVpcs",
            params: vec![param("VpcId.1", UNKNOWN_VPC_ID)],
            status: 400,
            code: ec2::state::INVALID_VPC_ID_NOT_FOUND,
        },
        Row {
            // `DeleteVpc`'s modelled output is `smithy.api#Unit`, the shape the envelope
            // writes its `<return>` flag for — so this row also pins that a refusal of
            // such an operation answers the ERROR envelope and its `<RequestID>`.
            case: "a delete of a VPC the account does not hold",
            action: "DeleteVpc",
            params: vec![param("VpcId", UNKNOWN_VPC_ID)],
            status: 400,
            code: ec2::state::INVALID_VPC_ID_NOT_FOUND,
        },
        Row {
            case: "a describe narrowed by a filter name no operation defines",
            action: "DescribeVpcs",
            params: filters(&[("cidr-block", &["10.0.0.0/16"])]),
            status: 400,
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        Row {
            case: "a static route named for a VPN connection the account does not hold",
            action: "CreateVpnConnectionRoute",
            params: vec![
                param("VpnConnectionId", UNKNOWN_VPN_CONNECTION_ID),
                param("DestinationCidrBlock", "192.168.0.0/24"),
            ],
            status: 400,
            code: ec2::state::INVALID_VPN_CONNECTION_ID,
        },
        Row {
            case: "a static route whose destination is not a CIDR block",
            action: "CreateVpnConnectionRoute",
            params: vec![
                param("VpnConnectionId", UNKNOWN_VPN_CONNECTION_ID),
                param("DestinationCidrBlock", "not-a-cidr"),
            ],
            status: 400,
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        Row {
            // Both members are `@required` AND `@clientOptional`, so an empty value reaches
            // the handler rather than the framework's refusal tail: this row is the
            // SERVICE's own `MissingParameter`, rendered by the same `serialize_error`.
            case: "a static route that names no VPN connection at all",
            action: "CreateVpnConnectionRoute",
            params: vec![
                param("VpnConnectionId", ""),
                param("DestinationCidrBlock", "192.168.0.0/24"),
            ],
            status: 400,
            code: ec2::state::MISSING_PARAMETER,
        },
    ]
}

/// The code the query protocols answer a request that named no operation of this service
/// with, and the one they answer a body they could not read at all with. Both are the
/// FRAMEWORK's codes, not the service's — this service declares no error shapes, and these
/// two refusals happen before any operation of it is named — so they are spelled out here
/// rather than taken from `ec2::state`, whose codes are the ones its handlers answer.
const INVALID_ACTION: &str = "InvalidAction";
const INVALID_REQUEST: &str = "InvalidRequest";

/// One row the framework answers in front of this service's operations: the form body
/// exactly as the client sent it, any headers it sent beside the ones every call carries,
/// and the status and `<Code>` the request is refused with.
struct FrameworkRow {
    case: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: String,
    status: u16,
    code: &'static str,
}

/// Every row refused before an operation of this service is reached. Two layers refuse
/// here: the one that resolves the action a body names (four ways of naming none this
/// service has) and the one that decodes the body's transfer framing (three ways of
/// declaring framing the bytes do not carry).
fn framework_rows() -> Vec<FrameworkRow> {
    let version = super::ec2_query::API_VERSION;
    vec![
        FrameworkRow {
            case: "an action this service has no operation for",
            headers: &[],
            body: format!("Action=NotAnAction&Version={version}"),
            status: 400,
            code: INVALID_ACTION,
        },
        FrameworkRow {
            case: "an action that names another service's operation",
            headers: &[],
            body: format!("Action=ListBuckets&Version={version}"),
            status: 400,
            code: INVALID_ACTION,
        },
        FrameworkRow {
            case: "a body that names no action at all",
            headers: &[],
            body: format!("Version={version}"),
            status: 400,
            code: INVALID_ACTION,
        },
        FrameworkRow {
            case: "an action field left empty",
            headers: &[],
            body: format!("Action=&Version={version}"),
            status: 400,
            code: INVALID_ACTION,
        },
        FrameworkRow {
            case: "a body declaring aws-chunked framing whose chunk size is not a number",
            headers: AWS_CHUNKED,
            body: format!("zz\r\nAction=DescribeVpcs&Version={version}\r\n0\r\n\r\n"),
            status: 400,
            code: INVALID_REQUEST,
        },
        FrameworkRow {
            case: "a body declaring aws-chunked framing whose chunk is shorter than it says",
            headers: AWS_CHUNKED,
            body: format!("ff\r\nAction=DescribeVpcs&Version={version}\r\n0\r\n\r\n"),
            status: 400,
            code: INVALID_REQUEST,
        },
        FrameworkRow {
            case: "a body declaring aws-chunked framing with no chunk header at all",
            headers: AWS_CHUNKED,
            body: format!("Action=DescribeVpcs&Version={version}"),
            status: 400,
            code: INVALID_REQUEST,
        },
    ]
}

/// Whether `value` has the form an AWS request id takes: five groups of 8-4-4-4-12
/// lowercase hexadecimal digits, as the recorded ids and the ids the framework's own
/// success envelope renders both read (`generate_request_id` in
/// `vendor/smithy-schema-server/src/query.rs`).
///
/// The FORM is what is asserted and not a particular value: the id names one call, so AWS
/// generates a different one per run and the recorded id is an example, never a
/// requirement.
fn is_request_id_form(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    if groups
        .iter()
        .zip([8usize, 4, 4, 4, 12])
        .any(|(group, length)| group.len() != length)
    {
        return false;
    }
    groups.iter().all(|group| {
        group
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

/// The text of every direct child of `body`'s root element named `name`, in document order.
///
/// `pub(crate)` so a suite that brings ONE operation under this rule reads the envelope
/// through this reader rather than declaring a second one.
pub(crate) fn root_children(case: &str, body: &str, root_name: &str, name: &str) -> Vec<String> {
    let parsed = document(case, body);
    let root = parsed.root_element();
    assert_eq!(
        root.tag_name().name(),
        root_name,
        "case {case:?}: expected a <{root_name}> root, got {body}"
    );
    root.children()
        .filter(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// The name of the LAST element child of `body`'s root.
fn last_root_child(case: &str, body: &str) -> String {
    let parsed = document(case, body);
    parsed
        .root_element()
        .children()
        .rfind(roxmltree::Node::is_element)
        .map(|node| node.tag_name().name().to_string())
        .unwrap_or_else(|| panic!("case {case:?}: the envelope has no children at all: {body}"))
}

/// The one request id a REFUSAL reports, having checked the rest of the error envelope is
/// intact: the expected `<Code>`, a non-empty `<Message>`, and `<RequestID>` last.
///
/// `pub(crate)` so a suite that brings FURTHER operations under this rule
/// ([`super::ec2_query_error_envelope_operations`]) reads the envelope through this one
/// reader rather than declaring a second one.
pub(crate) fn refusal_request_id(case: &str, body: &str, expected_code: &str) -> String {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let error = child(root, "Errors")
        .and_then(|errors| child(errors, "Error"))
        .unwrap_or_else(|| panic!("case {case:?}: no <Errors><Error> in {body}"));
    assert_eq!(
        child_text(error, "Code").as_deref(),
        Some(expected_code),
        "case {case:?}: expected the <Code> {expected_code:?}, got {body}"
    );
    let message = child_text(error, "Message").unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case {case:?}: the refusal must still say why, got {body}"
    );

    let reported = root_children(case, body, ERROR_ENVELOPE_ROOT, ERROR_REQUEST_ID);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one <{ERROR_REQUEST_ID}> under <{ERROR_ENVELOPE_ROOT}>, \
         got {reported:?} in {body}"
    );
    assert_eq!(
        last_root_child(case, body),
        ERROR_REQUEST_ID,
        "case {case:?}: <{ERROR_REQUEST_ID}> is the last child of the error envelope, got \
         {body}"
    );
    let request_id = reported[0].clone();
    assert!(
        is_request_id_form(&request_id),
        "case {case:?}: expected a request id in the 8-4-4-4-12 hexadecimal form, got \
         {request_id:?} in {body}"
    );
    assert!(
        root_children(case, body, ERROR_ENVELOPE_ROOT, SUCCESS_REQUEST_ID).is_empty(),
        "case {case:?}: a refusal reports the request under <{ERROR_REQUEST_ID}> only, got \
         {body}"
    );
    request_id
}

/// Every refusal this service renders reports the request it answers in one `<RequestID>`,
/// whatever operation answered and whatever the refusal is.
#[tokio::test]
async fn every_refusal_reports_the_request_it_answers_in_one_request_id() {
    // Each row's id is its own: a request id identifies ONE call, so no two responses may
    // report the same one.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in refusal_rows() {
        let (router, _engine) = super::ec2_query::emulator();
        let (status, body) = invoke(&router, row.action, &row.params).await;
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
        let request_id = refusal_request_id(&case, &body, row.code);
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: the request id {request_id:?} was already reported for another \
             response"
        );
        // The message is the refusal's own text, not the request id: splicing the id in
        // must leave it readable.
        assert!(
            !unescaped(&body).contains(&format!("<Message>{request_id}")),
            "case {case:?}: the request id must not displace the message, got {body}"
        );
    }
}

/// Two identical calls are two calls, so each is reported under its own request id.
#[tokio::test]
async fn two_identical_refusals_report_two_different_request_ids() {
    let (router, _engine) = super::ec2_query::emulator();
    for row in refusal_rows() {
        let case = format!("{} refused twice for {}", row.action, row.case);
        let (first_status, first) = invoke(&router, row.action, &row.params).await;
        let (second_status, second) = invoke(&router, row.action, &row.params).await;
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

/// A SUCCESS reports the request in its own envelope's lowercase `<requestId>` and carries
/// no `<RequestID>`; the refusal that follows it — a mutation that is already done — reports
/// its own under `<RequestID>`.
///
/// The pair is one resource's story: a transit gateway created (and with it the default
/// association route table the create owes it), a prefix list reference added to that route
/// table, and then the same reference asked for again, which a route table carrying at most
/// one route per prefix list has nothing left to do.
#[tokio::test]
async fn a_success_reports_the_request_in_its_own_lowercase_element() {
    let (router, engine) = super::ec2_query::emulator();
    // The prefix list the reference below routes: a reference routes exactly the ranges its
    // list holds, so the create resolves the `PrefixListId` against the account's own lists.
    // Which list it is has no part in this rule, which is about the ENVELOPE.
    engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>().lock().managed_prefix_lists.insert(
        PREFIX_LIST_ID.to_string(),
        ec2::state::ManagedPrefixListRecord {
            view: ec2::ops::types::ManagedPrefixList {
                prefix_list_id: Some(PREFIX_LIST_ID.to_string()),
                address_family: Some("IPv4".to_string()),
                state: Some("create-complete".to_string()),
                prefix_list_name: Some("the envelope suite's prefix list".to_string()),
                max_entries: Some(5),
                version: Some(1),
                owner_id: Some(engine::emulator_account_id().to_string()),
                ..Default::default()
            },
            entries: Vec::new(),
            pending: None,
        },
    );

    let case = "CreateTransitGateway";
    let (status, body) = invoke(&router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let reported = root_children(case, &body, "CreateTransitGatewayResponse", SUCCESS_REQUEST_ID);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one <{SUCCESS_REQUEST_ID}> in the success envelope, \
         got {reported:?} in {body}"
    );
    assert!(
        is_request_id_form(&reported[0]),
        "case {case:?}: expected a request id in the 8-4-4-4-12 hexadecimal form, got {:?} in \
         {body}",
        reported[0]
    );
    assert!(
        !body.contains(&format!("<{ERROR_REQUEST_ID}>")),
        "case {case:?}: a success reports the request under <{SUCCESS_REQUEST_ID}> only, got \
         {body}"
    );
    let route_table_id = super::ec2_query::element(&body, "associationDefaultRouteTableId")
        .unwrap_or_else(|| panic!("case {case:?}: no default association route table in {body}"));

    let reference = vec![
        param("TransitGatewayRouteTableId", &route_table_id),
        param("PrefixListId", PREFIX_LIST_ID),
        param("Blackhole", "true"),
    ];
    let case = "CreateTransitGatewayPrefixListReference";
    let (status, body) = invoke(&router, case, &reference).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let first_id = root_children(
        case,
        &body,
        "CreateTransitGatewayPrefixListReferenceResponse",
        SUCCESS_REQUEST_ID,
    );
    assert_eq!(
        first_id.len(),
        1,
        "case {case:?}: expected exactly one <{SUCCESS_REQUEST_ID}>, got {first_id:?} in {body}"
    );

    let case = "CreateTransitGatewayPrefixListReference, repeated";
    let (status, body) = invoke(
        &router,
        "CreateTransitGatewayPrefixListReference",
        &reference,
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a route table carries at most one route per prefix list, so the \
         repeat must be refused, got {status} with body {body}"
    );
    let refused_id = refusal_request_id(case, &body, ec2::state::INCORRECT_STATE);
    assert_ne!(
        refused_id, first_id[0],
        "case {case:?}: the refusal reports its own request id, got the successful call's \
         {refused_id:?}"
    );
}

/// A refusal answered BEFORE any operation of this service runs reports the request it
/// answers in the same one `<RequestID>`, and announces the same media type.
///
/// The envelope belongs to the service, not to an operation, so a request refused by the
/// layer that resolves the action — or by the one that decodes the body's transfer framing —
/// closes the same `<Response>` as a refusal a handler gave: the `<Code>` a client branches
/// on, the `<Message>` saying why, and the id it quotes when it reports the failure.
#[tokio::test]
async fn every_refusal_answered_before_an_operation_runs_reports_the_request_too() {
    // Each row's id is its own, exactly as for a refusal an operation gave.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in framework_rows() {
        let (router, _engine) = super::ec2_query::emulator();
        let (status, content_type, body) =
            super::ec2_query::call_raw(&router, REGION, row.headers, &row.body).await;
        let case = row.case;
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
        let request_id = refusal_request_id(case, &body, row.code);
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: the request id {request_id:?} was already reported for another \
             response"
        );
        // The message is the refusal's own text, not the request id: splicing the id in
        // must leave it readable.
        assert!(
            !unescaped(&body).contains(&format!("<Message>{request_id}")),
            "case {case:?}: the request id must not displace the message, got {body}"
        );
    }
}

/// Two identical requests refused before any operation runs are two calls, so each is
/// reported under its own request id.
#[tokio::test]
async fn two_identical_pre_operation_refusals_report_two_different_request_ids() {
    let (router, _engine) = super::ec2_query::emulator();
    for row in framework_rows() {
        let case = format!("{}, sent twice", row.case);
        let (first_status, _, first) =
            super::ec2_query::call_raw(&router, REGION, row.headers, &row.body).await;
        let (second_status, _, second) =
            super::ec2_query::call_raw(&router, REGION, row.headers, &row.body).await;
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
