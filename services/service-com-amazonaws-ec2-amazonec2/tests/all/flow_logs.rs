// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VPC flow log slice — `CreateFlowLogs`, `DescribeFlowLogs` and
//! `DeleteFlowLogs` — driven over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! Four rules are under test:
//!
//! * `CreateFlowLogs` STORES ONE FLOW LOG PER MONITORED RESOURCE and `DescribeFlowLogs`
//!   reports what it stored: the rows differ in the resource type, the destination, the
//!   traffic type, the aggregation interval and the tags.
//! * A flow log created with no `LogFormat` carries the DEFAULT record format, and one
//!   created with an explicit `LogFormat` carries exactly that string — so the two differ.
//!   The rows differ in the format asked for, including none at all.
//! * A flow log's DESTINATION ARN is only ever the one the request carried: the member is
//!   reported verbatim for a request that gave one and is ABSENT from a flow log whose
//!   CloudWatch Logs destination was named by log group name. The rows differ in the
//!   destination type and in which member named the destination.
//! * `DescribeFlowLogs` is a FILTER over the store, not a point lookup: an id or filter
//!   value nothing matches selects nothing and the call succeeds with an empty list. The
//!   rows differ in which ids and which documented filter narrow it.
//! * A flow-log call that names ids RESOLVES THE WHOLE BATCH before it mutates anything:
//!   one unknown id refuses the whole call, naming only the unknown ids, and the flow logs
//!   it named that do exist survive. The rows differ in how many ids are unknown and where
//!   in the batch they sit.
//! * A `CreateFlowLogs` resolves the resources it monitors BEFORE the delivery
//!   requirements of the destination it asks for: an unresolvable `ResourceIds` entry is
//!   refused with that resource kind's own not-found code whether or not the request
//!   carries the delivery role, and a request whose resources all resolve and whose
//!   CloudWatch Logs destination has no role is refused as a request whose members cannot
//!   be honoured together. The rows differ in the resource kind and in how the destination
//!   was named.
//! * An authorized `DryRun` is answered with `DryRunOperation` and performs nothing,
//!   whether or not the resources the request names exist — the rule
//!   `dry_run_and_unknown_ids` asserts over the rest of this service's surface.
//! * The id list a delete names is held to the bound its member documents, BEFORE any id is
//!   resolved: no id at all is refused, the maximum itself is served, one past it is refused.
//!   The rows differ in how many ids the batch names and whether they are stored or absent.
//! * The batch is a SET and a delete is NOT idempotent: one flow log gets ONE answer however
//!   many times the batch names it, and a flow log already deleted is unknown to the next
//!   delete. The steps differ in how often an id repeats and in whether it is stored, absent
//!   or already deleted.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_FLOW_LOG_FORMAT,
    DEFAULT_MAX_AGGREGATION_INTERVAL, FLOW_LOG_ID_PREFIX, FLOW_LOG_STATUS_ACTIVE,
    LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};

/// The Region every call here is signed for. The flow log store holds the account's flow
/// logs rather than one Region's, and the seeded default VPC is the one of the emulator's
/// default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// The IAM role a CloudWatch Logs flow log publishes under: "This parameter is required if
/// the destination type is cloud-watch-logs"
/// (`CreateFlowLogsRequest$DeliverLogsPermissionArn`).
const DELIVER_LOGS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/flow-logs-delivery";

/// A well-formed flow log id this account holds no flow log for — 17 lowercase hex digits
/// after the `fl-` prefix, the form `ComAmazonawsEc2Amazonec2State::next_id` mints.
const ABSENT_FLOW_LOG_ID: &str = "fl-0123456789abcdef0";

/// A second absent id, so a batch can name more than one unknown flow log.
const SECOND_ABSENT_FLOW_LOG_ID: &str = "fl-00000000000000000";

/// The record format AWS documents a flow log created with no `LogFormat` gets, written out
/// here as its own literal rather than read from the crate: the fields of the default flow
/// log record, in order (Amazon VPC User Guide, "Flow log records" — the default format's
/// field list). Asserting against the implementation's own constant could not catch a wrong
/// or truncated default, so the table below compares against THIS.
const AWS_DEFAULT_FLOW_LOG_FORMAT: &str = "${version} ${account-id} ${interface-id} \
                                           ${srcaddr} ${dstaddr} ${srcport} ${dstport} \
                                           ${protocol} ${packets} ${bytes} ${start} \
                                           ${end} ${action} ${log-status}";

/// The flow log status AWS documents: "The status of the flow log (`ACTIVE`)"
/// (`FlowLog$FlowLogStatus` —
/// `smithy_query(query="shape",shape="FlowLog")`). Written out here rather than read from
/// the crate for the reason [`AWS_DEFAULT_FLOW_LOG_FORMAT`] is.
const AWS_FLOW_LOG_STATUS_ACTIVE: &str = "ACTIVE";

/// A delivering flow log's outcome, one of the two values AWS documents: "The status of the
/// logs delivery (`SUCCESS` | `FAILED`)" (`FlowLog$DeliverLogsStatus`, and the values the
/// `deliver-log-status` filter selects on — `DescribeFlowLogsRequest$Filter`).
const AWS_DELIVER_LOGS_STATUS_SUCCESS: &str = "SUCCESS";

/// The other documented outcome of the same member — a flow log whose delivery failed.
const AWS_DELIVER_LOGS_STATUS_FAILED: &str = "FAILED";

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateFlowLogs` call carrying `params`, as its HTTP status and XML body.
async fn create_flow_logs(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateFlowLogs", params).await
}

/// One `DescribeFlowLogs` call carrying `params`, as its HTTP status and XML body.
async fn describe_flow_logs(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeFlowLogs", params).await
}

/// One `DeleteFlowLogs` call carrying `params`, as its HTTP status and XML body.
async fn delete_flow_logs(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteFlowLogs", params).await
}

/// The flow log ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe_flow_logs(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "flowLogId")
}

/// The XML of the one flow log `flow_log_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, flow_log_id: &str) -> String {
    let params = indexed("FlowLogId", &[flow_log_id]);
    let (status, body) = describe_flow_logs(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "flowLogId"),
        vec![flow_log_id.to_string()],
        "case {case:?}: expected exactly the flow log {flow_log_id}, got {body}"
    );
    body
}

/// A VPC created over the wire, by id — so a create can name a VPC the account really has
/// without reaching into the store.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// A subnet of `vpc_id` created over the wire, by id.
async fn create_subnet(router: &MultiServiceRouter, vpc_id: &str, cidr_block: &str) -> String {
    let params = [param("VpcId", vpc_id), param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateSubnet", &params).await;
    assert_eq!(
        status, 200,
        "CreateSubnet {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "subnetId").unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// A network interface in `subnet_id` created over the wire, by id.
async fn create_network_interface(router: &MultiServiceRouter, subnet_id: &str) -> String {
    let params = [param("SubnetId", subnet_id)];
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateNetworkInterface", &params).await;
    assert_eq!(
        status, 200,
        "CreateNetworkInterface in {subnet_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "networkInterfaceId")
        .unwrap_or_else(|| panic!("CreateNetworkInterface answered no networkInterfaceId: {body}"))
}

/// The form fields a CloudWatch Logs flow log on one resource is created with: everything
/// the destination type requires, and nothing else.
///
/// Shared with [`super::describe_page_truncation`], which seeds a flow log set to page.
pub(crate) fn cloud_watch_logs_params(
    resource_type: &str,
    resource_id: &str,
    log_group_name: &str,
) -> Vec<(String, String)> {
    vec![
        param("ResourceType", resource_type),
        param("ResourceId.1", resource_id),
        param("TrafficType", "ALL"),
        param("LogDestinationType", LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
        param("LogGroupName", log_group_name),
        param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
    ]
}

/// The one flow log id a successful create reports, asserting it reported exactly one.
fn created_id(case: &str, status: u16, body: &str) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let ids = elements(body, "item");
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: expected exactly one flow log id, got {ids:?} in {body}"
    );
    let flow_log_id = ids[0].clone();
    assert!(
        flow_log_id.starts_with(&format!("{FLOW_LOG_ID_PREFIX}-")),
        "case {case:?}: expected a {FLOW_LOG_ID_PREFIX}- prefixed id, got {flow_log_id:?}"
    );
    flow_log_id
}

/// Create one CloudWatch Logs flow log on `resource_id` and answer its id.
async fn create_one(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    resource_id: &str,
    log_group_name: &str,
) -> String {
    let params = cloud_watch_logs_params(resource_type, resource_id, log_group_name);
    let (status, body) = create_flow_logs(router, &params).await;
    created_id(case, status, &body)
}

/// Whether the XML `body` carries `<{member}>{value}</{member}>`.
fn reports(body: &str, member: &str, value: &str) -> bool {
    body.contains(&format!("<{member}>{value}</{member}>"))
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has turned on no flow log, so the describe reports an EMPTY set — not a
/// fabricated one — and describing is a READ: two calls report the same thing and the store
/// is untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_flow_log_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe_flow_logs(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let first_ids = elements(&first, "flowLogId");
    assert!(
        first_ids.is_empty(),
        "fresh account: expected no flow log, got {first_ids:?}"
    );
    assert!(
        first.contains("flowLogSet"),
        "fresh account: expected a flowLogSet element, got {first}"
    );

    let second_ids = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second_ids, first_ids,
        "a read must not change what a later caller sees: {first_ids:?} then {second_ids:?}"
    );
    assert!(
        engine.state::<Ec2State>().lock().flow_logs.is_empty(),
        "describing flow logs must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// CreateFlowLogs, and the round trip through DescribeFlowLogs
// ---------------------------------------------------------------------------

/// One row of the create table: which monitored resource, and which of the optional members
/// the request carries.
struct CreateCase {
    name: &'static str,
    /// The `FlowLogsResourceType` the row monitors.
    resource_type: &'static str,
    /// Which of the resources this test creates the row monitors: 0 the VPC, 1 the subnet,
    /// 2 the network interface.
    resource_index: usize,
    traffic_type: &'static str,
    log_group_name: &'static str,
    /// The `MaxAggregationInterval` the row asks for, or none — which must read back as the
    /// documented default.
    max_aggregation_interval: Option<i32>,
    tags: Vec<(&'static str, &'static str)>,
}

/// Each create stores one flow log per monitored resource and `DescribeFlowLogs` reads
/// every member back: the resource, the traffic type, the destination type, the log group,
/// the aggregation interval, the tags and the `ACTIVE` status. The rows differ in the
/// resource TYPE (a VPC, a subnet and a network interface), in the traffic type, in whether
/// an aggregation interval was asked for, and in how many tags they carry.
#[tokio::test]
async fn a_created_flow_log_round_trips_through_describe_flow_logs() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.213.0.0/16").await;
    let subnet_id = create_subnet(&router, &vpc_id, "10.213.1.0/24").await;
    let network_interface_id = create_network_interface(&router, &subnet_id).await;
    let resource_ids = [vpc_id, subnet_id, network_interface_id];

    let cases = vec![
        CreateCase {
            name: "a VPC, all traffic, no interval, no tags",
            resource_type: "VPC",
            resource_index: 0,
            traffic_type: "ALL",
            log_group_name: "/aws/vpc/flow-logs",
            max_aggregation_interval: None,
            tags: Vec::new(),
        },
        CreateCase {
            name: "a subnet, rejected traffic, one-minute interval, one tag",
            resource_type: "Subnet",
            resource_index: 1,
            traffic_type: "REJECT",
            log_group_name: "subnet-flow-logs",
            max_aggregation_interval: Some(60),
            tags: vec![("Name", "subnet-logs")],
        },
        CreateCase {
            name: "a network interface, accepted traffic, ten-minute interval, two tags",
            resource_type: "NetworkInterface",
            resource_index: 2,
            traffic_type: "ACCEPT",
            log_group_name: "eni-flow-logs",
            max_aggregation_interval: Some(600),
            tags: vec![("Name", "eni-logs"), ("Owner", "TeamA")],
        },
    ];

    for case in &cases {
        let resource_id = &resource_ids[case.resource_index];
        let mut params = cloud_watch_logs_params(
            case.resource_type,
            resource_id,
            case.log_group_name,
        );
        // The row's own traffic type replaces the one the shared fields carry.
        params.retain(|(name, _)| name != "TrafficType");
        params.push(param("TrafficType", case.traffic_type));
        if let Some(interval) = case.max_aggregation_interval {
            params.push(param("MaxAggregationInterval", &interval.to_string()));
        }
        if !case.tags.is_empty() {
            params.push(param("TagSpecification.1.ResourceType", "vpc-flow-log"));
            for (index, (key, value)) in case.tags.iter().enumerate() {
                params.push(param(
                    &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                    key,
                ));
                params.push(param(
                    &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                    value,
                ));
            }
        }

        let (status, body) = create_flow_logs(&router, &params).await;
        let flow_log_id = created_id(case.name, status, &body);

        let reported = describe_one(&router, case.name, &flow_log_id).await;
        let expected_interval = case
            .max_aggregation_interval
            .unwrap_or(DEFAULT_MAX_AGGREGATION_INTERVAL)
            .to_string();
        let mut expected: Vec<(&str, String)> = vec![
            ("resourceId", resource_id.clone()),
            ("trafficType", case.traffic_type.to_string()),
            (
                "logDestinationType",
                LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS.to_string(),
            ),
            ("logGroupName", case.log_group_name.to_string()),
            ("flowLogStatus", FLOW_LOG_STATUS_ACTIVE.to_string()),
            (
                "deliverLogsPermissionArn",
                DELIVER_LOGS_ROLE_ARN.to_string(),
            ),
            ("maxAggregationInterval", expected_interval),
        ];
        for (key, value) in &case.tags {
            expected.push(("key", (*key).to_string()));
            expected.push(("value", (*value).to_string()));
        }
        for (member, value) in &expected {
            assert!(
                reports(&reported, member, value),
                "case {case:?}: expected <{member}>{value}</{member}>, got {reported}",
                case = case.name,
            );
        }
        // Every row named its destination by log group NAME, so there is no destination ARN
        // to report; `destination_arn_reports_only_what_the_request_gave` is the table that
        // covers the member across the destination types.
        assert!(
            !reported.contains("<logDestination>"),
            "case {case:?}: expected no logDestination member, got {reported}",
            case = case.name,
        );
        // "The date and time the flow log was created" (`FlowLog$CreationTime`) is a real
        // timestamp, not an empty member.
        let creation_time = element(&reported, "creationTime").unwrap_or_default();
        assert!(
            super::ec2_query::as_timestamp(&creation_time).is_some(),
            "case {case:?}: expected an ISO 8601 creationTime, got {creation_time:?}",
            case = case.name,
        );
    }

    // Every row's flow log is in the store, each on its own resource.
    let reported = described_ids(&router, "every created flow log", &[]).await;
    assert_eq!(
        reported.len(),
        cases.len(),
        "every created flow log: expected {} flow logs, got {reported:?}",
        cases.len()
    );
}

/// One call may monitor SEVERAL resources, and it mints one flow log for each of them.
#[tokio::test]
async fn one_call_mints_one_flow_log_per_named_resource() {
    let (router, _engine) = super::ec2_query::emulator();
    let first_vpc = create_vpc(&router, "10.214.0.0/16").await;
    let second_vpc = create_vpc(&router, "10.215.0.0/16").await;

    let mut params = cloud_watch_logs_params("VPC", &first_vpc, "/aws/vpc/flow-logs");
    params.push(param("ResourceId.2", &second_vpc));
    let (status, body) = create_flow_logs(&router, &params).await;
    assert_eq!(
        status, 200,
        "two VPCs: expected HTTP 200, got {status} with body {body}"
    );
    let minted = elements(&body, "item");
    assert_eq!(
        minted.len(),
        2,
        "two VPCs: expected one flow log per resource, got {minted:?} in {body}"
    );

    for (vpc_id, flow_log_id) in [(&first_vpc, &minted[0]), (&second_vpc, &minted[1])] {
        let reported = describe_one(&router, "one per resource", flow_log_id).await;
        assert!(
            reports(&reported, "resourceId", vpc_id),
            "one per resource: expected {flow_log_id} to monitor {vpc_id}, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// LogFormat: the documented default, or exactly what was asked for
// ---------------------------------------------------------------------------

/// A flow log created WITHOUT a `LogFormat` reports the documented default format, and one
/// created WITH an explicit `LogFormat` reports exactly that string — so a client reading
/// the member always learns the real record layout. The rows differ in the format asked
/// for: none at all, one field, a few fields, and the default spelled out explicitly.
#[tokio::test]
async fn log_format_defaults_when_the_request_omits_it_and_is_kept_when_it_does_not() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.216.0.0/16").await;

    // The crate's constant must BE the format AWS documents, so every other assertion here
    // can name either.
    assert_eq!(
        DEFAULT_FLOW_LOG_FORMAT, AWS_DEFAULT_FLOW_LOG_FORMAT,
        "the default record format must be the one AWS documents, got {DEFAULT_FLOW_LOG_FORMAT:?}"
    );

    // Each row: the `LogFormat` the request carries (or none), and what the flow log must
    // then report.
    let cases: Vec<(&str, Option<&str>, String)> = vec![
        (
            "no LogFormat at all",
            None,
            AWS_DEFAULT_FLOW_LOG_FORMAT.to_string(),
        ),
        (
            "one field",
            Some("${srcaddr}"),
            "${srcaddr}".to_string(),
        ),
        (
            "four fields",
            Some("${version} ${srcaddr} ${dstaddr} ${action}"),
            "${version} ${srcaddr} ${dstaddr} ${action}".to_string(),
        ),
        (
            "the default format, asked for explicitly",
            Some(AWS_DEFAULT_FLOW_LOG_FORMAT),
            AWS_DEFAULT_FLOW_LOG_FORMAT.to_string(),
        ),
    ];

    let mut reported_formats: Vec<String> = Vec::new();
    for (row, (case, asked_for, expected)) in cases.iter().enumerate() {
        // Each row delivers to a log group of its own: one resource carries ONE flow log of
        // the same configuration and destination, and two rows here ask for the same record
        // format (the default, once by omission and once spelled out).
        let log_group_name = format!("/aws/vpc/flow-logs-{row}");
        let mut params = cloud_watch_logs_params("VPC", &vpc_id, &log_group_name);
        if let Some(log_format) = asked_for {
            params.push(param("LogFormat", log_format));
        }
        let (status, body) = create_flow_logs(&router, &params).await;
        let flow_log_id = created_id(case, status, &body);

        let reported = describe_one(&router, case, &flow_log_id).await;
        let log_format = element(&reported, "logFormat").unwrap_or_default();
        assert_eq!(
            &log_format, expected,
            "case {case:?}: expected the log format {expected:?}, got {log_format:?}"
        );
        reported_formats.push(log_format);
    }

    // The point of the rule: a flow log with no `LogFormat` and one with an explicit one
    // report DIFFERENT formats.
    assert_ne!(
        reported_formats[0], reported_formats[2],
        "a defaulted and an explicit LogFormat must differ, both reported \
         {:?}",
        reported_formats[0]
    );

    // A `LogFormat` that names no COMPLETE field is refused rather than stored: "If you
    // specify this parameter, you must include at least one field", and a field is
    // `${field-id}` — an opening marker with nothing closing it names none.
    for no_field in ["version srcaddr", "${", "${srcaddr"] {
        let mut params = cloud_watch_logs_params("VPC", &vpc_id, "/aws/vpc/flow-logs");
        params.push(param("LogFormat", no_field));
        let (status, body) = create_flow_logs(&router, &params).await;
        assert_eq!(
            status, 400,
            "the LogFormat {no_field:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "the LogFormat {no_field:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeFlowLogs is a filter, not a point lookup
// ---------------------------------------------------------------------------

/// One row of a table whose input is a whole request: the form fields it carries, and what
/// the call must then answer. Named rather than a tuple so the tables below stay readable.
struct RequestCase {
    name: &'static str,
    params: Vec<(String, String)>,
}

/// `DescribeFlowLogs` rows: a request, and the flow log ids it must report.
struct NarrowingCase {
    request: RequestCase,
    expected: Vec<String>,
}

/// `CreateFlowLogs` rows: a request, and the error code it must be refused with.
struct RefusalCase {
    request: RequestCase,
    expected_code: &'static str,
}

/// Dry-run rows: which operation is asked, what the request names, and the fields it
/// carries.
struct DryRunCase {
    action: &'static str,
    names: &'static str,
    params: Vec<(String, String)>,
}

/// `FlowLogIds` and `Filter` NARROW the store: a well-formed id or a filter value nothing
/// matches selects nothing and the call succeeds with an empty list, and an id that names a
/// stored flow log selects exactly it. The rows differ in which ids are named, how many of
/// them are absent, and which documented filter does the narrowing.
#[tokio::test]
async fn describe_flow_logs_filters_the_store_rather_than_looking_one_up() {
    let (router, _engine) = super::ec2_query::emulator();
    let first_vpc = create_vpc(&router, "10.217.0.0/16").await;
    let second_vpc = create_vpc(&router, "10.218.0.0/16").await;

    let on_first = create_one(&router, "seed first", "VPC", &first_vpc, "first-group").await;
    let mut params = cloud_watch_logs_params("VPC", &second_vpc, "second-group");
    params.retain(|(name, _)| name != "TrafficType");
    params.push(param("TrafficType", "REJECT"));
    params.push(param("TagSpecification.1.ResourceType", "vpc-flow-log"));
    params.push(param("TagSpecification.1.Tag.1.Key", "Owner"));
    params.push(param("TagSpecification.1.Tag.1.Value", "TeamB"));
    let (status, body) = create_flow_logs(&router, &params).await;
    let on_second = created_id("seed second", status, &body);

    // Each row: what the request names, and the flow log ids it must then report.
    let narrowing = |name: &'static str,
                     params: Vec<(String, String)>,
                     expected: Vec<String>| NarrowingCase {
        request: RequestCase { name, params },
        expected,
    };
    let cases: Vec<NarrowingCase> = vec![
        narrowing(
            "no narrowing at all",
            Vec::new(),
            vec![on_first.clone(), on_second.clone()],
        ),
        narrowing(
            "one absent id",
            indexed("FlowLogId", &[ABSENT_FLOW_LOG_ID]),
            Vec::new(),
        ),
        narrowing(
            "two absent ids",
            indexed("FlowLogId", &[ABSENT_FLOW_LOG_ID, SECOND_ABSENT_FLOW_LOG_ID]),
            Vec::new(),
        ),
        narrowing(
            "one stored id",
            indexed("FlowLogId", &[on_first.as_str()]),
            vec![on_first.clone()],
        ),
        narrowing(
            "a stored id and an absent one",
            indexed("FlowLogId", &[on_second.as_str(), ABSENT_FLOW_LOG_ID]),
            vec![on_second.clone()],
        ),
        narrowing(
            "the resource-id filter on the first VPC",
            filters(&[("resource-id", &[first_vpc.as_str()])]),
            vec![on_first.clone()],
        ),
        narrowing(
            "the resource-id filter on a VPC nothing monitors",
            filters(&[("resource-id", &["vpc-0123456789abcdef0"])]),
            Vec::new(),
        ),
        narrowing(
            "the resource-id filter on both VPCs",
            filters(&[("resource-id", &[first_vpc.as_str(), second_vpc.as_str()])]),
            vec![on_first.clone(), on_second.clone()],
        ),
        narrowing(
            "the flow-log-id filter",
            filters(&[("flow-log-id", &[on_second.as_str()])]),
            vec![on_second.clone()],
        ),
        narrowing(
            "the log-group-name filter",
            filters(&[("log-group-name", &["first-group"])]),
            vec![on_first.clone()],
        ),
        narrowing(
            "the traffic-type filter",
            filters(&[("traffic-type", &["REJECT"])]),
            vec![on_second.clone()],
        ),
        narrowing(
            "the log-destination-type filter",
            filters(&[(
                "log-destination-type",
                &[LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS],
            )]),
            vec![on_first.clone(), on_second.clone()],
        ),
        narrowing(
            "the log-destination-type filter on a type nothing uses",
            filters(&[("log-destination-type", &["s3"])]),
            Vec::new(),
        ),
        narrowing(
            "the tag filter",
            filters(&[("tag:Owner", &["TeamB"])]),
            vec![on_second.clone()],
        ),
        narrowing(
            "the tag-key filter",
            filters(&[("tag-key", &["Owner"])]),
            vec![on_second.clone()],
        ),
        narrowing(
            "two filters, joined with AND",
            [
                filters(&[("traffic-type", &["ALL"])]),
                filters(&[("log-group-name", &["second-group"])])
                    .into_iter()
                    .map(|(name, value)| (name.replace("Filter.1.", "Filter.2."), value))
                    .collect(),
            ]
            .concat(),
            Vec::new(),
        ),
        narrowing(
            "the deliver-log-status filter on the delivering flow logs",
            filters(&[("deliver-log-status", &[AWS_DELIVER_LOGS_STATUS_SUCCESS])]),
            vec![on_first.clone(), on_second.clone()],
        ),
        narrowing(
            "the deliver-log-status filter on a delivery outcome nothing is in",
            filters(&[("deliver-log-status", &[AWS_DELIVER_LOGS_STATUS_FAILED])]),
            Vec::new(),
        ),
    ];

    for case in &cases {
        let reported = described_ids(&router, case.request.name, &case.request.params).await;
        assert_eq!(
            &reported, &case.expected,
            "case {case:?}: expected the flow logs {expected:?}, got {reported:?}",
            case = case.request.name,
            expected = case.expected,
        );
    }

    // A filter name this operation does not document is refused rather than read as "no
    // results".
    let (status, body) =
        describe_flow_logs(&router, &filters(&[("resource-type", &["VPC"])])).await;
    assert_eq!(
        status, 400,
        "an undocumented filter: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFilter",
        "an undocumented filter: expected InvalidFilter, got {body}"
    );
}

// ---------------------------------------------------------------------------
// DeleteFlowLogs resolves the whole batch before it deletes anything
// ---------------------------------------------------------------------------

/// A lone valid id deletes cleanly, with an empty `Unsuccessful` and the flow log gone from
/// the store.
#[tokio::test]
async fn deleting_one_flow_log_reports_nothing_unsuccessful_and_removes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.219.0.0/16").await;
    let flow_log_id = create_one(&router, "lone delete", "VPC", &vpc_id, "lone-group").await;

    // `delete_served` is where the served shape is asserted once for every suite row: HTTP
    // 200 with an empty `Unsuccessful`.
    delete_served(&router, "lone delete", &[&flow_log_id]).await;

    let reported = described_ids(&router, "after the delete", &[]).await;
    assert!(
        reported.is_empty(),
        "after the delete: expected no flow log, got {reported:?}"
    );
    assert!(
        engine.state::<Ec2State>().lock().flow_logs.is_empty(),
        "a delete must remove the record from the store"
    );
}

/// A batch carrying ANY id this account holds no flow log for is refused whole: the code is
/// `InvalidFlowLogId.NotFound`, the message enumerates the unknown ids and their count and
/// names NONE of the valid ones, and nothing is deleted. The rows differ in how many ids
/// are unknown and where in the batch they sit.
#[tokio::test]
async fn an_unknown_id_anywhere_in_the_batch_refuses_the_whole_delete() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.220.0.0/16").await;
    // Each seed delivers to its own log group: one resource carries ONE flow log of the
    // same configuration and destination.
    let first = create_one(&router, "seed first", "VPC", &vpc_id, "batch-group").await;
    let second = create_one(&router, "seed second", "VPC", &vpc_id, "second-batch-group").await;
    let stored = vec![first.clone(), second.clone()];

    // Each row: the ids the batch names, and the unknown ones the refusal must enumerate.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one unknown id alone",
            vec![ABSENT_FLOW_LOG_ID],
            vec![ABSENT_FLOW_LOG_ID],
        ),
        (
            "two unknown ids, none valid",
            vec![SECOND_ABSENT_FLOW_LOG_ID, ABSENT_FLOW_LOG_ID],
            vec![SECOND_ABSENT_FLOW_LOG_ID, ABSENT_FLOW_LOG_ID],
        ),
        (
            "a valid id first, then an unknown one",
            vec![first.as_str(), ABSENT_FLOW_LOG_ID],
            vec![ABSENT_FLOW_LOG_ID],
        ),
        (
            "an unknown id first, then a valid one",
            vec![ABSENT_FLOW_LOG_ID, second.as_str()],
            vec![ABSENT_FLOW_LOG_ID],
        ),
        (
            "two valid ids around one unknown",
            vec![first.as_str(), SECOND_ABSENT_FLOW_LOG_ID, second.as_str()],
            vec![SECOND_ABSENT_FLOW_LOG_ID],
        ),
    ];

    for (case, named, unknown) in &cases {
        let message = delete_refused(&router, case, named, "InvalidFlowLogId.NotFound").await;
        let expected = not_found_message(unknown);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        for valid in &stored {
            assert!(
                !message.contains(valid.as_str()),
                "case {case:?}: the refusal must name no valid id, but named {valid} in \
                 {message:?}"
            );
        }
        // Nothing was deleted: both flow logs the batch could have removed are still there.
        let reported = described_ids(&router, case, &[]).await;
        assert_eq!(
            &reported, &stored,
            "case {case:?}: a refused batch must delete nothing, but the store now holds \
             {reported:?}"
        );
    }
}

/// An authorized `DryRun` is answered with `DryRunOperation` and performs nothing, and the
/// answer does NOT depend on whether the resources the request names exist — the flag asks
/// whether the caller is permitted. The rows differ in the operation and in whether what
/// the request names is there at all.
#[tokio::test]
async fn a_dry_run_reports_the_dry_run_whether_or_not_what_it_names_exists() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.221.0.0/16").await;
    let flow_log_id = create_one(&router, "dry run", "VPC", &vpc_id, "dry-run-group").await;

    // Each row: the action, what the request names, and the fields it carries.
    let dry_run = |action: &'static str, names: &'static str, params: Vec<(String, String)>| {
        let mut params = params;
        params.push(param("DryRun", "true"));
        DryRunCase {
            action,
            names,
            params,
        }
    };
    let cases: Vec<DryRunCase> = vec![
        dry_run(
            "DeleteFlowLogs",
            "a stored flow log",
            indexed("FlowLogId", &[&flow_log_id]),
        ),
        dry_run(
            "DeleteFlowLogs",
            "an absent flow log",
            indexed("FlowLogId", &[ABSENT_FLOW_LOG_ID]),
        ),
        dry_run(
            "DeleteFlowLogs",
            "one of each",
            indexed("FlowLogId", &[&flow_log_id, ABSENT_FLOW_LOG_ID]),
        ),
        dry_run(
            "CreateFlowLogs",
            "a stored VPC",
            cloud_watch_logs_params("VPC", &vpc_id, "dry-run-group"),
        ),
        dry_run(
            "CreateFlowLogs",
            "an absent VPC",
            cloud_watch_logs_params("VPC", "vpc-0123456789abcdef0", "dry-run-group"),
        ),
    ];

    for row in &cases {
        let case = format!("{action} naming {names}", action = row.action, names = row.names);
        let (status, body) =
            super::ec2_query::call(&router, REGION, row.action, &row.params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: unexpected message in {body}"
        );
        // Nothing was created and nothing was removed: the one flow log the seed made is
        // still the only one.
        let reported = described_ids(&router, &case, &[]).await;
        assert_eq!(
            reported,
            vec![flow_log_id.clone()],
            "case {case:?}: a dry run must perform nothing, but the store now holds \
             {reported:?}"
        );
    }

    assert_eq!(
        engine.state::<Ec2State>().lock().flow_logs.len(),
        1,
        "a dry run must leave the store exactly as it was"
    );
}

// ---------------------------------------------------------------------------
// CreateFlowLogs resolves the whole batch, and the members its destination requires
// ---------------------------------------------------------------------------

/// A create naming a resource this account does not hold is refused whole and stores
/// nothing — whichever resource type it named, and wherever the unknown id sits in the
/// batch.
#[tokio::test]
async fn a_create_naming_an_absent_resource_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.222.0.0/16").await;
    let subnet_id = create_subnet(&router, &vpc_id, "10.222.1.0/24").await;

    // Each row: the resource type, the ids the request names, and the error code it must be
    // refused with.
    let cases: Vec<(&str, &str, Vec<&str>, &str)> = vec![
        (
            "an absent VPC",
            "VPC",
            vec!["vpc-0123456789abcdef0"],
            "InvalidVpcId.NotFound",
        ),
        (
            "a stored VPC and an absent one",
            "VPC",
            vec![vpc_id.as_str(), "vpc-00000000000000000"],
            "InvalidVpcId.NotFound",
        ),
        (
            "an absent subnet",
            "Subnet",
            vec!["subnet-0123456789abcdef0"],
            "InvalidSubnetId.NotFound",
        ),
        (
            "a stored subnet and an absent one",
            "Subnet",
            vec![subnet_id.as_str(), "subnet-00000000000000000"],
            "InvalidSubnetId.NotFound",
        ),
        (
            "an absent network interface",
            "NetworkInterface",
            vec!["eni-0123456789abcdef0"],
            "InvalidNetworkInterfaceId.NotFound",
        ),
        (
            "a VPC id given for the subnet resource type",
            "Subnet",
            vec![vpc_id.as_str()],
            "InvalidSubnetId.NotFound",
        ),
    ];

    for (case, resource_type, resource_ids, expected_code) in &cases {
        let mut params = cloud_watch_logs_params(resource_type, resource_ids[0], "absent-group");
        for (index, resource_id) in resource_ids.iter().enumerate().skip(1) {
            params.push(param(&format!("ResourceId.{}", index + 1), resource_id));
        }
        let (status, body) = create_flow_logs(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            engine.state::<Ec2State>().lock().flow_logs.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// One row of the resolution table: the `FlowLogsResourceType` member, an id of that kind
/// the account holds no resource for, and the error code the flow-log surface refuses it
/// with.
struct AbsentResourceCase {
    /// The `ResourceType` the request names.
    resource_type: &'static str,
    /// The `ResourceIds` entry — an id of that kind this account holds nothing for.
    resource_id: &'static str,
    /// The code the refusal carries, which is the flow-log family's own for that kind.
    expected_code: &'static str,
}

/// The monitored resources a request names are RESOLVED BEFORE the delivery requirements of
/// the destination it asks for: a `ResourceIds` entry the account holds no resource for is
/// refused with that resource kind's own not-found code, naming the id, whether or not the
/// request carries the role a CloudWatch Logs destination needs.
///
/// The rows are every member of `FlowLogsResourceType`
/// (`smithy_query(query="shape",shape="FlowLogsResourceType")`), each crossed with the
/// request carrying the delivery role and omitting it — so neither the resource kind nor
/// the presence of the role can change the answer.
#[tokio::test]
async fn an_absent_monitored_resource_is_refused_before_the_destination_requirements() {
    let (router, engine) = super::ec2_query::emulator();

    let cases: Vec<AbsentResourceCase> = vec![
        AbsentResourceCase {
            resource_type: "VPC",
            resource_id: "vpc-0ffffffffffffffff",
            expected_code: "InvalidVpcId.NotFound",
        },
        AbsentResourceCase {
            resource_type: "Subnet",
            resource_id: "subnet-0ffffffffffffffff",
            expected_code: "InvalidSubnetId.NotFound",
        },
        AbsentResourceCase {
            resource_type: "NetworkInterface",
            resource_id: "eni-0ffffffffffffffff",
            expected_code: "InvalidNetworkInterfaceId.NotFound",
        },
        AbsentResourceCase {
            resource_type: "TransitGateway",
            resource_id: "tgw-0ffffffffffffffff",
            expected_code: "InvalidTransitGatewayId.NotFound",
        },
        AbsentResourceCase {
            resource_type: "TransitGatewayAttachment",
            resource_id: "tgw-attach-0ffffffffffffffff",
            expected_code: "InvalidTransitGatewayAttachmentId.NotFound",
        },
        AbsentResourceCase {
            resource_type: "RegionalNatGateway",
            resource_id: "nat-0ffffffffffffffff",
            expected_code: "NatGatewayNotFound",
        },
    ];

    for row in &cases {
        for with_role in [true, false] {
            let case = format!(
                "{resource_type} with{maybe} the delivery role",
                resource_type = row.resource_type,
                maybe = if with_role { "" } else { "out" },
            );
            let mut params = vec![
                param("ResourceType", row.resource_type),
                param("ResourceId.1", row.resource_id),
                param("LogGroupName", "resolve-the-resource-first"),
            ];
            // "This parameter is not supported for transit gateway resource types. It is
            // required for the other resource types" (`CreateFlowLogsRequest$TrafficType`).
            if !row.resource_type.starts_with("TransitGateway") {
                params.push(param("TrafficType", "ALL"));
            }
            if with_role {
                params.push(param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN));
            }

            let (status, body) = create_flow_logs(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                row.expected_code,
                "case {case:?}: expected {expected_code}, got {body}",
                expected_code = row.expected_code,
            );
            assert_eq!(
                error_message(&body),
                format!("Unknown resource {resource_id}", resource_id = row.resource_id),
                "case {case:?}: unexpected message in {body}"
            );
            assert!(
                engine.state::<Ec2State>().lock().flow_logs.is_empty(),
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

/// One row of the delivery-role table: the case, the form fields that name the CloudWatch
/// Logs destination, and the resource the flow log would monitor.
struct DeliveryRoleCase {
    /// The case name every assertion of this row reports.
    name: &'static str,
    /// The fields that name the destination — the only thing that differs between rows.
    destination: Vec<(String, String)>,
    /// The `ResourceType` the request names.
    resource_type: &'static str,
    /// The id of a resource of that kind this account really holds.
    resource_id: String,
}

/// A create whose monitored resources all resolve and whose CloudWatch Logs destination
/// carries no delivery role is refused as a request whose own members cannot be honoured
/// together — `InvalidParameter`, naming the `LogDestinationType` that made the role
/// mandatory — and the same request with the role is served.
///
/// The rows differ in how the log group was named (by `LogGroupName`, by a log-group ARN in
/// `LogDestination`, and with the destination type left to its documented default) and in
/// the resource kind monitored, so the rule covers every way a CloudWatch Logs destination
/// is asked for: the refusal does not vary with the member that named the destination.
#[tokio::test]
async fn a_cloud_watch_logs_destination_without_a_delivery_role_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.224.0.0/16").await;
    let subnet_id = create_subnet(&router, &vpc_id, "10.224.1.0/24").await;
    let interface_id = create_network_interface(&router, &subnet_id).await;
    let log_group = "needs-a-delivery-role";
    // The log group named as an ARN instead: "specify the ARN of a CloudWatch Logs log
    // group. For example: arn:aws:logs:region:account_id:log-group:my_group"
    // (`CreateFlowLogsRequest$LogDestination`).
    let log_group_arn = format!("arn:aws:logs:{REGION}:000000000000:log-group:{log_group}");
    // The one message every one of these refusals carries: it names the destination type
    // that requires the role, not the member the request named the group with.
    let expected_message = format!(
        "DeliverLogsPermissionArn can't be empty if LogDestinationType is \
         {LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS}."
    );

    let cases: Vec<DeliveryRoleCase> = vec![
        DeliveryRoleCase {
            name: "a log group named by LogGroupName",
            destination: vec![
                param("LogDestinationType", LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
                param("LogGroupName", log_group),
            ],
            resource_type: "VPC",
            resource_id: vpc_id.clone(),
        },
        DeliveryRoleCase {
            name: "a log group named by LogDestination",
            destination: vec![
                param("LogDestinationType", LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
                param("LogDestination", &log_group_arn),
            ],
            resource_type: "Subnet",
            resource_id: subnet_id.clone(),
        },
        DeliveryRoleCase {
            name: "the default destination type",
            destination: vec![param("LogGroupName", log_group)],
            resource_type: "NetworkInterface",
            resource_id: interface_id.clone(),
        },
    ];

    for DeliveryRoleCase {
        name: case,
        destination,
        resource_type,
        resource_id,
    } in &cases
    {
        let mut params = vec![
            param("ResourceType", resource_type),
            param("ResourceId.1", resource_id),
            param("TrafficType", "ALL"),
        ];
        params.extend(destination.iter().cloned());

        let (status, body) = create_flow_logs(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameter",
            "case {case:?}: expected InvalidParameter, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: unexpected message in {body}"
        );
        assert!(
            engine.state::<Ec2State>().lock().flow_logs.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );

        // The very same request WITH the role is served, so the refusal above is the
        // missing role and nothing else.
        params.push(param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN));
        let (status, body) = create_flow_logs(&router, &params).await;
        let flow_log_id = created_id(case, status, &body);
        let described = describe_one(&router, case, &flow_log_id).await;
        assert!(
            reports(&described, "resourceId", resource_id),
            "case {case:?}: expected the flow log to monitor {resource_id}, got {described}"
        );
        assert!(
            reports(&described, "deliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
            "case {case:?}: expected the delivery role to round-trip, got {described}"
        );

        delete_flow_logs(&router, &indexed("FlowLogId", &[&flow_log_id])).await;
    }

    assert!(
        engine.state::<Ec2State>().lock().flow_logs.is_empty(),
        "every flow log this test created was deleted again"
    );
}

/// The members a destination type requires, and the ones it forbids: each row is a request
/// that is well formed against the model and still cannot be served, with the code the
/// service refuses it with.
#[tokio::test]
async fn a_create_is_held_to_the_members_its_resource_and_destination_require() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.223.0.0/16").await;
    let transit_gateway_group = "tgw-flow-logs";

    let refusal = |name: &'static str,
                   params: Vec<(String, String)>,
                   expected_code: &'static str| RefusalCase {
        request: RequestCase { name, params },
        expected_code,
    };
    let cases: Vec<RefusalCase> = vec![
        refusal(
            "no TrafficType for a VPC",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("LogGroupName", "needs-traffic-type"),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
            ],
            "MissingParameter",
        ),
        refusal(
            "no delivery role for a CloudWatch Logs destination",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("LogGroupName", "needs-a-role"),
            ],
            "InvalidParameter",
        ),
        refusal(
            "no destination at all for a CloudWatch Logs destination",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
            ],
            "MissingParameter",
        ),
        refusal(
            "a destination that is not a log group ARN, for a CloudWatch Logs destination",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
                param("LogDestination", "arn:aws:s3:::flow-logs-bucket"),
            ],
            "InvalidParameterValue",
        ),
        refusal(
            "no destination for an S3 destination type",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("LogDestinationType", "s3"),
            ],
            "MissingParameter",
        ),
        refusal(
            "a log group name for an S3 destination type",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("LogDestinationType", "s3"),
                param("LogDestination", "arn:aws:s3:::flow-logs-bucket"),
                param("LogGroupName", "not-for-s3"),
            ],
            "InvalidParameterCombination",
        ),
        refusal(
            "an aggregation interval that is neither 60 nor 600",
            vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("LogGroupName", "bad-interval"),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
                param("MaxAggregationInterval", "300"),
            ],
            "InvalidParameterValue",
        ),
        refusal(
            "a TrafficType for a transit gateway resource type",
            vec![
                param("ResourceType", "TransitGateway"),
                param("ResourceId.1", "tgw-0123456789abcdef0"),
                param("TrafficType", "ALL"),
                param("LogGroupName", transit_gateway_group),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
            ],
            "InvalidParameterCombination",
        ),
        refusal(
            "a ten-minute interval for a transit gateway resource type",
            vec![
                param("ResourceType", "TransitGateway"),
                param("ResourceId.1", "tgw-0123456789abcdef0"),
                param("LogGroupName", transit_gateway_group),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
                param("MaxAggregationInterval", "600"),
            ],
            "InvalidParameterValue",
        ),
    ];

    for case in &cases {
        let (status, body) = create_flow_logs(&router, &case.request.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}",
            case = case.request.name,
        );
        assert_eq!(
            error_code(&body),
            case.expected_code,
            "case {case:?}: expected {expected_code}, got {body}",
            case = case.request.name,
            expected_code = case.expected_code,
        );
        assert!(
            engine.state::<Ec2State>().lock().flow_logs.is_empty(),
            "case {case:?}: a refused create must store nothing",
            case = case.request.name,
        );
    }
}

/// An S3 destination keeps the ARN it was given, reports no log group, and carries the
/// destination options the request asked for with each absent member at its documented
/// default.
#[tokio::test]
async fn an_s3_flow_log_keeps_its_destination_arn_and_options() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.224.0.0/16").await;
    let bucket_arn = "arn:aws:s3:::flow-logs-bucket/prefix/";

    let params = vec![
        param("ResourceType", "VPC"),
        param("ResourceId.1", &vpc_id),
        param("TrafficType", "ALL"),
        param("LogDestinationType", "s3"),
        param("LogDestination", bucket_arn),
        param("DestinationOptions.FileFormat", "parquet"),
    ];
    let (status, body) = create_flow_logs(&router, &params).await;
    let flow_log_id = created_id("an S3 destination", status, &body);

    let reported = describe_one(&router, "an S3 destination", &flow_log_id).await;
    for (member, value) in [
        ("logDestinationType", "s3"),
        ("logDestination", bucket_arn),
        ("fileFormat", "parquet"),
        // "The default is false" (`DestinationOptionsRequest$HiveCompatiblePartitions`,
        // `$PerHourPartition`).
        ("hiveCompatiblePartitions", "false"),
        ("perHourPartition", "false"),
    ] {
        assert!(
            reports(&reported, member, value),
            "an S3 destination: expected <{member}>{value}</{member}>, got {reported}"
        );
    }
    assert!(
        !reported.contains("<logGroupName>"),
        "an S3 destination: expected no logGroupName, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// Delivery health: the outcome is reported beside the flow log's own status
// ---------------------------------------------------------------------------

/// Every flow log the store holds reports its LOG DELIVERY OUTCOME beside its own status:
/// `flowLogStatus` and `deliverLogsStatus` are reported together, the delivery of a flow log
/// whose destination the create resolved is `SUCCESS`, a successful delivery carries no error
/// message, and the documented `deliver-log-status` filter selects on exactly that member —
/// so a client can tell a delivering flow log from a broken one, and list the account's
/// flow logs by delivery health.
///
/// The rows differ in the destination TYPE (the three the model admits, including the one
/// left to its default), in WHICH member named the destination (a log group name, a log
/// group ARN, a bucket ARN, a delivery stream ARN), in the monitored resource KIND (a VPC,
/// a subnet, a network interface), in the traffic type, and in whether a cross-account
/// delivery role or destination options were asked for.
#[tokio::test]
async fn every_flow_log_reports_its_delivery_outcome_beside_its_flow_log_status() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.227.0.0/16").await;
    let subnet_id = create_subnet(&router, &vpc_id, "10.227.1.0/24").await;
    let network_interface_id = create_network_interface(&router, &subnet_id).await;

    // The destination forms `CreateFlowLogsRequest$LogDestination` documents, in Regions and
    // accounts that are deliberately not the emulator's own.
    const GROUP_ARN: &str = "arn:aws:logs:eu-west-1:111122223333:log-group:delivery-group";
    const BUCKET_ARN: &str = "arn:aws:s3:::flow-log-delivery-bucket/vpc/";
    const STREAM_ARN: &str = "arn:aws:firehose:eu-central-1:111122223333:deliverystream:delivered";
    const CROSS_ACCOUNT_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/flow-logs-cross-account";

    let mut cross_account = cloud_watch_logs_params("VPC", &vpc_id, "cross-account-group");
    cross_account.push(param("DeliverCrossAccountRole", CROSS_ACCOUNT_ROLE_ARN));

    let cases: Vec<RequestCase> = vec![
        RequestCase {
            name: "cloud-watch-logs on a VPC, named by LogGroupName",
            params: cloud_watch_logs_params("VPC", &vpc_id, "/aws/vpc/flow-logs"),
        },
        RequestCase {
            name: "the default destination type on a subnet, rejected traffic",
            params: vec![
                param("ResourceType", "Subnet"),
                param("ResourceId.1", &subnet_id),
                param("TrafficType", "REJECT"),
                param("LogGroupName", "subnet-delivery-group"),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
            ],
        },
        RequestCase {
            name: "cloud-watch-logs on a network interface, named by a log group ARN",
            params: vec![
                param("ResourceType", "NetworkInterface"),
                param("ResourceId.1", &network_interface_id),
                param("TrafficType", "ACCEPT"),
                param("LogDestinationType", LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
                param("LogDestination", GROUP_ARN),
                param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
            ],
        },
        RequestCase {
            name: "cloud-watch-logs with a cross-account delivery role",
            params: cross_account,
        },
        RequestCase {
            name: "s3 on a VPC, named by a bucket ARN",
            params: vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ALL"),
                param("LogDestinationType", "s3"),
                param("LogDestination", BUCKET_ARN),
            ],
        },
        RequestCase {
            name: "s3 with hive-compatible partitions and a parquet file format",
            params: vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "REJECT"),
                param("LogDestinationType", "s3"),
                param("LogDestination", BUCKET_ARN),
                param("DestinationOptions.FileFormat", "parquet"),
                param("DestinationOptions.HiveCompatiblePartitions", "true"),
            ],
        },
        RequestCase {
            name: "kinesis-data-firehose on a VPC, named by a delivery stream ARN",
            params: vec![
                param("ResourceType", "VPC"),
                param("ResourceId.1", &vpc_id),
                param("TrafficType", "ACCEPT"),
                param("LogDestinationType", "kinesis-data-firehose"),
                param("LogDestination", STREAM_ARN),
            ],
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for case in &cases {
        let (status, body) = create_flow_logs(&router, &case.params).await;
        let flow_log_id = created_id(case.name, status, &body);
        let reported = describe_one(&router, case.name, &flow_log_id).await;

        let statuses = (
            element(&reported, "flowLogStatus"),
            element(&reported, "deliverLogsStatus"),
        );
        assert_eq!(
            (
                statuses.0.as_deref(),
                statuses.1.as_deref()
            ),
            (
                Some(AWS_FLOW_LOG_STATUS_ACTIVE),
                Some(AWS_DELIVER_LOGS_STATUS_SUCCESS)
            ),
            "case {case:?}: expected (flowLogStatus, deliverLogsStatus) \
             ({AWS_FLOW_LOG_STATUS_ACTIVE:?}, {AWS_DELIVER_LOGS_STATUS_SUCCESS:?}), got \
             {statuses:?} in {reported}",
            case = case.name,
        );
        // "Information about the error that occurred" (`FlowLog$DeliverLogsErrorMessage`) —
        // a delivery that succeeded has no error to report.
        assert!(
            !reported.contains("<deliverLogsErrorMessage>"),
            "case {case:?}: expected no deliverLogsErrorMessage, got {reported}",
            case = case.name,
        );

        // The documented filter selects on exactly that member: this flow log is found
        // under the outcome it reports, and under no other one.
        for (outcome, expected) in [
            (AWS_DELIVER_LOGS_STATUS_SUCCESS, vec![flow_log_id.clone()]),
            (AWS_DELIVER_LOGS_STATUS_FAILED, Vec::new()),
        ] {
            let params = filters(&[
                ("deliver-log-status", &[outcome][..]),
                ("flow-log-id", &[flow_log_id.as_str()][..]),
            ]);
            let selected = described_ids(&router, case.name, &params).await;
            assert_eq!(
                selected, expected,
                "case {case:?}: expected the deliver-log-status {outcome} filter to select \
                 {expected:?}, got {selected:?}",
                case = case.name,
            );
        }
        minted.push(flow_log_id);
    }

    // Every flow log this account holds is delivering, so the delivery-health listing a
    // client polls reports all of them rather than an empty set.
    minted.sort();
    let selected = described_ids(
        &router,
        "every delivering flow log",
        &filters(&[("deliver-log-status", &[AWS_DELIVER_LOGS_STATUS_SUCCESS])]),
    )
    .await;
    assert_eq!(
        selected, minted,
        "every delivering flow log: expected {minted:?}, got {selected:?}"
    );
}

/// How one row of [`destination_arn_reports_only_what_the_request_gave`] names its
/// destination, and what the flow log it creates must then report.
#[derive(Debug)]
struct DestinationCase {
    name: &'static str,
    /// The `LogDestinationType` the request carries, or none at all — "Default:
    /// cloud-watch-logs" (`CreateFlowLogsRequest$LogDestinationType`).
    log_destination_type: Option<&'static str>,
    /// The `LogGroupName` the request carries, if any.
    log_group_name: Option<&'static str>,
    /// The `LogDestination` the request carries, if any.
    log_destination: Option<&'static str>,
    /// The `logDestination` the describe must report, or `None` when it must carry no such
    /// member at all.
    reported_log_destination: Option<&'static str>,
    /// The `logGroupName` the describe must report, or `None` for none.
    reported_log_group_name: Option<&'static str>,
}

/// A flow log reports as its destination ARN exactly the `LogDestination` the request
/// carried, and NO such member when the request carried none — the destination named by
/// `LogGroupName` is a log group name, and an ARN for it could only be composed from a
/// Region and an account the request never gave. So `logDestination` is the member that
/// tells a client the destination really was given as an ARN.
///
/// The rows differ in the destination TYPE (the three the model admits, including the one
/// left to its default), in WHICH member named the destination (the group name, a group ARN,
/// a group ARN carrying the log-stream wildcard, or both members at once), and so in whether
/// each of the two members is reported at all.
#[tokio::test]
async fn destination_arn_reports_only_what_the_request_gave() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.226.0.0/16").await;

    // The ARNs below are the forms `CreateFlowLogsRequest$LogDestination` documents: a
    // CloudWatch Logs log group ("arn:aws:logs:region:account_id:log-group:my_group"), an S3
    // bucket with an optional subfolder ("arn:aws:s3:::my_bucket/my_subfolder/") and a
    // Kinesis Data Firehose delivery stream
    // ("arn:aws:firehose:region:account_id:deliverystream:my_stream"). Their Regions and
    // account ids are deliberately NOT the emulator's own, so a reported ARN can only have
    // come from the request.
    const GROUP_ARN: &str = "arn:aws:logs:eu-west-1:111122223333:log-group:arn-named-group";
    const WILDCARD_GROUP_ARN: &str =
        "arn:aws:logs:ap-south-1:444455556666:log-group:wildcard-group:*";
    const OTHER_GROUP_ARN: &str = "arn:aws:logs:sa-east-1:777788889999:log-group:other-group";
    const BUCKET_ARN: &str = "arn:aws:s3:::flow-log-bucket/vpc-flow-logs/";
    const STREAM_ARN: &str = "arn:aws:firehose:eu-central-1:111122223333:deliverystream:flow-logs";

    let cases = [
        DestinationCase {
            name: "cloud-watch-logs named by LogGroupName",
            log_destination_type: Some(LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
            log_group_name: Some("named-group"),
            log_destination: None,
            reported_log_destination: None,
            reported_log_group_name: Some("named-group"),
        },
        DestinationCase {
            name: "the default destination type named by LogGroupName",
            log_destination_type: None,
            log_group_name: Some("/aws/vpc/flow-logs"),
            log_destination: None,
            reported_log_destination: None,
            reported_log_group_name: Some("/aws/vpc/flow-logs"),
        },
        DestinationCase {
            name: "cloud-watch-logs named by a log group ARN",
            log_destination_type: Some(LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
            log_group_name: None,
            log_destination: Some(GROUP_ARN),
            reported_log_destination: Some(GROUP_ARN),
            reported_log_group_name: Some("arn-named-group"),
        },
        DestinationCase {
            name: "cloud-watch-logs named by a log group ARN with the log-stream wildcard",
            log_destination_type: Some(LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
            log_group_name: None,
            log_destination: Some(WILDCARD_GROUP_ARN),
            reported_log_destination: Some(WILDCARD_GROUP_ARN),
            reported_log_group_name: Some("wildcard-group"),
        },
        DestinationCase {
            name: "cloud-watch-logs named by both members at once",
            log_destination_type: Some(LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
            log_group_name: Some("both-ways-group"),
            log_destination: Some(OTHER_GROUP_ARN),
            reported_log_destination: Some(OTHER_GROUP_ARN),
            reported_log_group_name: Some("both-ways-group"),
        },
        DestinationCase {
            name: "s3 named by a bucket ARN",
            log_destination_type: Some("s3"),
            log_group_name: None,
            log_destination: Some(BUCKET_ARN),
            reported_log_destination: Some(BUCKET_ARN),
            reported_log_group_name: None,
        },
        DestinationCase {
            name: "kinesis-data-firehose named by a delivery stream ARN",
            log_destination_type: Some("kinesis-data-firehose"),
            log_group_name: None,
            log_destination: Some(STREAM_ARN),
            reported_log_destination: Some(STREAM_ARN),
            reported_log_group_name: None,
        },
    ];

    for case in &cases {
        let mut params = vec![
            param("ResourceType", "VPC"),
            param("ResourceId.1", &vpc_id),
            param("TrafficType", "ALL"),
        ];
        if let Some(destination_type) = case.log_destination_type {
            params.push(param("LogDestinationType", destination_type));
        }
        if let Some(group) = case.log_group_name {
            params.push(param("LogGroupName", group));
        }
        if let Some(destination) = case.log_destination {
            params.push(param("LogDestination", destination));
        }
        // "This parameter is required if the destination type is cloud-watch-logs"
        // (`CreateFlowLogsRequest$DeliverLogsPermissionArn`).
        if case
            .log_destination_type
            .unwrap_or(LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS)
            == LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS
        {
            params.push(param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN));
        }

        let (status, body) = create_flow_logs(&router, &params).await;
        let flow_log_id = created_id(case.name, status, &body);
        let reported = describe_one(&router, case.name, &flow_log_id).await;

        for (member, expected) in [
            ("logDestination", case.reported_log_destination),
            ("logGroupName", case.reported_log_group_name),
        ] {
            let actual = element(&reported, member);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?} in {reported}"
            );
        }
    }
}

/// A `ClientToken` makes the create idempotent: a repeat answers the flow logs the first
/// call minted and turns on no second set.
#[tokio::test]
async fn a_repeated_client_token_answers_the_flow_logs_it_already_minted() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.225.0.0/16").await;

    let mut params = cloud_watch_logs_params("VPC", &vpc_id, "idempotent-group");
    params.push(param("ClientToken", "flow-log-token"));

    let (status, first) = create_flow_logs(&router, &params).await;
    let first_ids = {
        assert_eq!(
            status, 200,
            "first create: expected HTTP 200, got {status} with body {first}"
        );
        elements(&first, "item")
    };
    let (status, second) = create_flow_logs(&router, &params).await;
    assert_eq!(
        status, 200,
        "repeat create: expected HTTP 200, got {status} with body {second}"
    );
    let second_ids = elements(&second, "item");
    assert_eq!(
        second_ids, first_ids,
        "repeat create: expected the flow logs {first_ids:?} the token already minted, got \
         {second_ids:?}"
    );

    let reported = described_ids(&router, "after the repeat", &[]).await;
    assert_eq!(
        reported, first_ids,
        "after the repeat: expected only the flow logs the first call minted, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The id list a delete names: its documented bounds, and the batch as a SET
// ---------------------------------------------------------------------------

/// The most flow log ids one delete may name: "Constraint: Maximum of 1000 flow log IDs"
/// (`DeleteFlowLogsRequest$FlowLogIds` —
/// `smithy_query(query="doc",shape="DeleteFlowLogsRequest",member="FlowLogIds")`). Written
/// out here as the DOCUMENTED number rather than read from the crate: comparing against the
/// implementation's own constant could not catch a wrong bound.
const DOCUMENTED_MAX_FLOW_LOG_IDS: usize = 1000;

/// The code this service answers a request that leaves a `@required` member out with: the
/// request never reaches the handler, because the boundary validator refuses it first — the
/// answer every operation of this crate gives for a member the request does not carry.
const BOUNDARY_MISSING_MEMBER: &str = "MalformedInput";

/// `count` DISTINCT well-formed flow log ids this account holds no flow log for.
///
/// 17 lowercase hex digits after the `fl-` prefix, the form `next_id` mints, each starting
/// with `f` — far above any value this store's monotonic id counter reaches, so none of them
/// can collide with an id a create really minted.
fn absent_ids(count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| format!("{FLOW_LOG_ID_PREFIX}-f{index:016x}"))
        .collect()
}

/// Delete the batch `ids` names, asserting it is SERVED with an empty `Unsuccessful`.
async fn delete_served(router: &MultiServiceRouter, case: &str, ids: &[&str]) {
    let (status, body) = delete_flow_logs(router, &indexed("FlowLogId", ids)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    // `UnsuccessfulItem$ResourceId` is the member a reported failure carries, so none of
    // them means nothing failed.
    let failed = elements(&body, "resourceId");
    assert!(
        failed.is_empty(),
        "case {case:?}: expected an empty Unsuccessful, got {failed:?} in {body}"
    );
}

/// Delete the batch `ids` names, asserting it is refused with HTTP 400 and `code`, and
/// answer the refusal's message.
async fn delete_refused(
    router: &MultiServiceRouter,
    case: &str,
    ids: &[&str],
    code: &str,
) -> String {
    let (status, body) = delete_flow_logs(router, &indexed("FlowLogId", ids)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let reported = error_code(&body);
    assert_eq!(
        reported, code,
        "case {case:?}: expected the code {code:?}, got {reported:?} in {body}"
    );
    error_message(&body)
}

/// The refusal a batch naming `unknown` flow logs carries, as the message enumerating their
/// count and then the ids themselves.
fn not_found_message(unknown: &[&str]) -> String {
    format!(
        "These flow log ids in the input list are not found: [TotalCount: {count}] {ids}",
        count = unknown.len(),
        ids = unknown.join(" "),
    )
}

/// The `FlowLogIds` list is held to the bound the member documents, and that bound is
/// checked BEFORE any id is resolved: a request naming no id at all never reaches the
/// handler (ec2Query carries no `FlowLogId.N` field, so the `@required` member is absent and
/// the request boundary refuses it — which is why the handler's own empty-list branch cannot
/// be reached over the wire), a list AT the documented maximum is served, and one id past
/// the maximum is refused with `InvalidParameterValue` whether or not the flow logs it names
/// exist. The rows differ in how many ids the batch names — none, one, the bound itself and
/// one past it — and in whether those ids name a stored flow log or ones this account holds
/// no flow log for.
#[tokio::test]
async fn the_flow_log_id_list_is_held_to_its_documented_bounds() {
    // Each row: how many ids the batch names, whether they all name the stored flow log
    // (`true`) or are all absent (`false`), and the code the refusal carries — `None` for a
    // row that is served, which is HTTP 200, every refusal here being HTTP 400. A batch of
    // NO ids does not carry the member at all, so it is refused at the request boundary
    // rather than by the handler.
    let cases: Vec<(&str, usize, bool, Option<&str>)> = vec![
        ("no id at all", 0, true, Some(BOUNDARY_MISSING_MEMBER)),
        ("one stored id", 1, true, None),
        (
            "the documented maximum, every id the stored flow log",
            DOCUMENTED_MAX_FLOW_LOG_IDS,
            true,
            None,
        ),
        (
            "the documented maximum, every id absent",
            DOCUMENTED_MAX_FLOW_LOG_IDS,
            false,
            Some("InvalidFlowLogId.NotFound"),
        ),
        (
            "one id past the documented maximum, every id the stored flow log",
            DOCUMENTED_MAX_FLOW_LOG_IDS + 1,
            true,
            Some("InvalidParameterValue"),
        ),
        (
            "one id past the documented maximum, every id absent",
            DOCUMENTED_MAX_FLOW_LOG_IDS + 1,
            false,
            Some("InvalidParameterValue"),
        ),
    ];

    for (case, count, name_the_stored_one, expected_code) in cases {
        // One emulator per row: a row that is served deletes, and the rows must not see
        // each other's store.
        let (router, engine) = super::ec2_query::emulator();
        let vpc_id = create_vpc(&router, "10.226.0.0/16").await;
        let flow_log_id = create_one(&router, case, "VPC", &vpc_id, "bounds-group").await;

        let ids: Vec<String> = if name_the_stored_one {
            vec![flow_log_id.clone(); count]
        } else {
            absent_ids(count)
        };
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();

        match expected_code {
            None => {
                delete_served(&router, case, &named).await;
                // A served batch removed the flow log it named, however many times it
                // named it.
                assert!(
                    engine.state::<Ec2State>().lock().flow_logs.is_empty(),
                    "case {case:?}: expected the flow log gone from the store, got {:?}",
                    engine.state::<Ec2State>().lock().flow_logs.keys()
                );
            }
            Some(code) => {
                let message = delete_refused(&router, case, &named, code).await;
                match code {
                    BOUNDARY_MISSING_MEMBER => assert!(
                        message.contains("flowLogIds"),
                        "case {case:?}: expected a message naming the member left out, got \
                         {message:?}"
                    ),
                    "InvalidParameterValue" => {
                        // The refusal reports what was asked for and the maximum allowed.
                        for expected in [count.to_string(), DOCUMENTED_MAX_FLOW_LOG_IDS.to_string()]
                        {
                            assert!(
                                message.contains(&expected),
                                "case {case:?}: expected a message naming {expected:?}, got \
                                 {message:?}"
                            );
                        }
                    }
                    _ => assert_eq!(
                        message,
                        not_found_message(&named),
                        "case {case:?}: expected the not-found message enumerating every id \
                         named, got {message:?}"
                    ),
                }
                // A refused batch deleted nothing.
                let reported = described_ids(&router, case, &[]).await;
                assert_eq!(
                    reported,
                    vec![flow_log_id.clone()],
                    "case {case:?}: a refused batch must delete nothing, but the store now \
                     holds {reported:?}"
                );
            }
        }
    }
}

/// A delete batch is a SET, and a delete is NOT idempotent: one flow log gets ONE answer
/// however many times the batch names it, and once a flow log is deleted the next batch
/// naming it is refused exactly like one naming an id this account never held. The steps
/// differ in how often the batch repeats an id, and in whether that id is stored, absent, or
/// one an earlier step already deleted.
#[tokio::test]
async fn a_repeated_id_gets_one_answer_and_a_deleted_flow_log_stays_deleted() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.227.0.0/16").await;
    // Each seed delivers to its own log group: one resource carries ONE flow log of the
    // same configuration and destination.
    let first = create_one(&router, "seed first", "VPC", &vpc_id, "set-group").await;
    let second = create_one(&router, "seed second", "VPC", &vpc_id, "second-set-group").await;
    let stored = vec![first.clone(), second.clone()];

    // One absent id named twice is ONE unknown id: the refusal counts it once and names it
    // once, rather than reporting the repeat as a second failure.
    let case = "one absent id named twice";
    let message = delete_refused(
        &router,
        case,
        &[ABSENT_FLOW_LOG_ID, ABSENT_FLOW_LOG_ID],
        "InvalidFlowLogId.NotFound",
    )
    .await;
    assert_eq!(
        message,
        not_found_message(&[ABSENT_FLOW_LOG_ID]),
        "case {case:?}: expected the unknown id counted once, got {message:?}"
    );

    // A stored id repeated beside a repeated absent one: still one unknown id, and the whole
    // batch is still refused.
    let case = "stored and absent ids each named twice";
    let message = delete_refused(
        &router,
        case,
        &[&first, ABSENT_FLOW_LOG_ID, &first, ABSENT_FLOW_LOG_ID],
        "InvalidFlowLogId.NotFound",
    )
    .await;
    assert_eq!(
        message,
        not_found_message(&[ABSENT_FLOW_LOG_ID]),
        "case {case:?}: expected the unknown id counted once and no stored id named, got \
         {message:?}"
    );
    let reported = described_ids(&router, case, &[]).await;
    assert_eq!(
        reported, stored,
        "case {case:?}: a refused batch must delete nothing, but the store now holds \
         {reported:?}"
    );

    // The same stored id three times deletes it ONCE and reports nothing unsuccessful — a
    // repeat is not a second, failing delete.
    let case = "one stored id named three times";
    delete_served(&router, case, &[&first, &first, &first]).await;
    let reported = described_ids(&router, case, &[]).await;
    assert_eq!(
        reported,
        vec![second.clone()],
        "case {case:?}: expected only the flow log the batch did not name, got {reported:?}"
    );

    // Deleting is not idempotent: a second delete of that id is refused like any id this
    // account holds no flow log for, and the flow log it did not name is untouched.
    let case = "a second delete of the same id";
    let message = delete_refused(&router, case, &[&first], "InvalidFlowLogId.NotFound").await;
    assert_eq!(
        message,
        not_found_message(&[&first]),
        "case {case:?}: expected a deleted id to be refused as unknown, got {message:?}"
    );
    let reported = described_ids(&router, case, &[]).await;
    assert_eq!(
        reported,
        vec![second.clone()],
        "case {case:?}: a refused batch must delete nothing, but the store now holds \
         {reported:?}"
    );

    // And the last flow log goes the same way, leaving the store empty.
    let case = "the last stored flow log";
    delete_served(&router, case, &[&second]).await;
    assert!(
        engine.state::<Ec2State>().lock().flow_logs.is_empty(),
        "case {case:?}: expected an empty flow log store, got {:?}",
        engine.state::<Ec2State>().lock().flow_logs.keys()
    );
}
