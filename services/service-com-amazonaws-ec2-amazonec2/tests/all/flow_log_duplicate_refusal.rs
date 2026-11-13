// SPDX-License-Identifier: CC0-1.0

//! The conflicting-state precondition `CreateFlowLogs` checks before it stores anything,
//! driven over the wire the service speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! ONE rule is under test: a monitored resource carries ONE flow log of a given capture
//! configuration delivered to a given destination, so a create that asks for a flow log the
//! account already holds is refused HTTP 400 `FlowLogAlreadyExists` — "Error. There is an
//! existing Flow Log with the same configuration and log destination." — and stores nothing,
//! while a create that differs in ANY part of that configuration or in its destination is
//! served and mints a flow log of its own.
//!
//! The rows differ along what the rule generalises over:
//!
//! * which member names the destination (the log group given outright, and the same group
//!   read out of a log group ARN), and the members that name no monitoring of their own at
//!   all (tags, the delivery role, a fresh `ClientToken`) — every one of those a REFUSAL;
//! * the member a documented default is spelled out in (the record format, the aggregation
//!   interval, the destination options), which asks for the SAME flow log — a refusal;
//! * the monitored resource, its kind, the traffic type, the log group, the destination
//!   type, the record format, the aggregation interval and each destination option — every
//!   one of those a SERVED create;
//! * where in a batch the duplicate sits, so a call naming one resource that already has the
//!   flow log stores nothing at all for the resources beside it;
//! * and a `ClientToken` replay, which reports the flow logs it already minted rather than
//!   being refused as a duplicate of them — including the token a create that provided none
//!   is answered, which names exactly the flow logs that call stored.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_FLOW_LOG_FORMAT,
    DEFAULT_MAX_AGGREGATION_INTERVAL, LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The IAM role a CloudWatch Logs flow log publishes under: "This parameter is required if
/// the destination type is cloud-watch-logs"
/// (`CreateFlowLogsRequest$DeliverLogsPermissionArn`).
const DELIVER_LOGS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/flow-logs-delivery";

/// A second delivery role, so a row can differ in the role alone — which names no different
/// monitoring and no different destination.
const OTHER_DELIVER_LOGS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/flow-logs-other";

/// The log group the seeded flow log delivers to.
const SEEDED_GROUP: &str = "/aws/vpc/seeded-flow-logs";

/// The code AWS refuses a duplicate flow log with, written out here rather than read from the
/// crate: comparing against the implementation's own constant could not catch a wrong code.
const AWS_FLOW_LOG_ALREADY_EXISTS: &str = "FlowLogAlreadyExists";

/// The message AWS words that refusal with — fixed text with no variable part, written out
/// here for the same reason the code above is.
const AWS_FLOW_LOG_ALREADY_EXISTS_MESSAGE: &str =
    "Error. There is an existing Flow Log with the same configuration and log destination.";

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

/// A VPC created over the wire, by id.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// A subnet of `vpc_id` created over the wire, by id — a monitored resource of another KIND.
async fn create_subnet(router: &MultiServiceRouter, vpc_id: &str, cidr_block: &str) -> String {
    let params = [param("VpcId", vpc_id), param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateSubnet", &params).await;
    assert_eq!(
        status, 200,
        "CreateSubnet {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "subnetId").unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// The members a CloudWatch Logs flow log on one VPC is created with: everything the
/// destination type requires, and nothing else — the baseline every row below varies.
fn baseline(resource_id: &str, log_group_name: &str) -> Vec<(String, String)> {
    vec![
        param("ResourceType", "VPC"),
        param("ResourceId.1", resource_id),
        param("TrafficType", "ALL"),
        param("LogDestinationType", LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS),
        param("LogGroupName", log_group_name),
        param("DeliverLogsPermissionArn", DELIVER_LOGS_ROLE_ARN),
    ]
}

/// `params` with every field named `name` replaced by one carrying `value`, and the field
/// appended when it carried none — so a row states only what it differs in.
fn with(params: &[(String, String)], name: &str, value: &str) -> Vec<(String, String)> {
    let mut changed: Vec<(String, String)> = params
        .iter()
        .filter(|(field, _)| field != name)
        .cloned()
        .collect();
    changed.push(param(name, value));
    changed
}

/// `params` without the field named `name`.
fn without(params: &[(String, String)], name: &str) -> Vec<(String, String)> {
    params
        .iter()
        .filter(|(field, _)| field != name)
        .cloned()
        .collect()
}

/// The flow log ids the account holds, in the order `DescribeFlowLogs` reports them.
async fn stored_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeFlowLogs", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeFlowLogs expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "flowLogId")
}

/// Assert `params` is REFUSED as a duplicate, and that the refusal stored nothing: the
/// account still holds exactly `held`.
async fn refused_as_duplicate(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
    held: &[String],
) {
    let (status, body) = create_flow_logs(router, params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        AWS_FLOW_LOG_ALREADY_EXISTS,
        "case {case:?}: expected {AWS_FLOW_LOG_ALREADY_EXISTS}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        AWS_FLOW_LOG_ALREADY_EXISTS_MESSAGE,
        "case {case:?}: expected the message {AWS_FLOW_LOG_ALREADY_EXISTS_MESSAGE:?}, got {body}"
    );
    let stored = stored_ids(router, case).await;
    assert_eq!(
        stored,
        held.to_vec(),
        "case {case:?}: a refused create must store nothing, but the account now holds \
         {stored:?}"
    );
}

/// Assert `params` is SERVED and minted exactly one flow log, and answer that flow log's id.
async fn served(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = create_flow_logs(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let minted = elements(&body, "item");
    assert_eq!(
        minted.len(),
        1,
        "case {case:?}: expected exactly one flow log id, got {minted:?} in {body}"
    );
    minted[0].clone()
}

/// Delete the flow logs `ids` names, so the next row decides against the seeded store alone.
async fn delete(router: &MultiServiceRouter, case: &str, ids: &[&str]) {
    let params = indexed("FlowLogId", ids);
    let (status, body) =
        super::ec2_query::call(router, REGION, "DeleteFlowLogs", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteFlowLogs expected HTTP 200, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// The same configuration and destination, however the request spells it
// ---------------------------------------------------------------------------

/// One destination-options row: the request it sends, and whether that request asks for the
/// flow log the seed already is.
struct OptionCase {
    name: &'static str,
    params: Vec<(String, String)>,
    duplicate: bool,
}

/// A create that asks for the flow log the account already holds is REFUSED, however the
/// request spells that monitoring: the destination named by group name or read out of a log
/// group ARN, a documented default spelled out rather than omitted, and members that name no
/// monitoring of their own (tags, the delivery role, a `ClientToken` never used before).
/// Nothing is stored, and the flow log already serving the resource survives untouched.
#[tokio::test]
async fn a_create_asking_for_a_flow_log_the_account_already_holds_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.60.0.0/16").await;
    let seed = baseline(&vpc_id, SEEDED_GROUP);
    let seeded = served(&router, "the seeded flow log", &seed).await;
    let held = vec![seeded.clone()];

    // "arn:aws:logs:region:account_id:log-group:my_group" (`CreateFlowLogsRequest$
    // LogDestination`) — the same group the seed named by name, named the other documented
    // way, with the log-stream wildcard a CloudWatch Logs ARN conventionally ends with.
    let account = ::engine::emulator_account_id();
    let group_arn = format!("arn:aws:logs:{REGION}:{account}:log-group:{SEEDED_GROUP}");
    let group_arn_with_streams = format!("{group_arn}:*");

    // Each row: the request, which must be refused as a duplicate of the seed.
    let mut cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("the very same request again", seed.clone()),
        (
            "the same log group, named by its ARN",
            with(&without(&seed, "LogGroupName"), "LogDestination", &group_arn),
        ),
        (
            "the same log group, named by an ARN carrying the log-stream wildcard",
            with(
                &without(&seed, "LogGroupName"),
                "LogDestination",
                &group_arn_with_streams,
            ),
        ),
        (
            "the same monitoring with the documented default record format spelled out",
            with(&seed, "LogFormat", DEFAULT_FLOW_LOG_FORMAT),
        ),
        (
            "the same monitoring with the documented default aggregation interval spelled out",
            with(
                &seed,
                "MaxAggregationInterval",
                &DEFAULT_MAX_AGGREGATION_INTERVAL.to_string(),
            ),
        ),
        (
            "the same monitoring under another delivery role",
            with(
                &seed,
                "DeliverLogsPermissionArn",
                OTHER_DELIVER_LOGS_ROLE_ARN,
            ),
        ),
        (
            "the same monitoring with a cross-account delivery role beside it",
            with(
                &seed,
                "DeliverCrossAccountRole",
                "arn:aws:iam::111122223333:role/flow-logs-cross-account",
            ),
        ),
        (
            "the same monitoring with a ClientToken never used before",
            with(&seed, "ClientToken", "a-token-of-its-own"),
        ),
    ];
    // The same monitoring carrying tags, which name no destination and no captured traffic.
    let mut tagged = seed.clone();
    tagged.push(param("TagSpecification.1.ResourceType", "vpc-flow-log"));
    tagged.push(param("TagSpecification.1.Tag.1.Key", "Owner"));
    tagged.push(param("TagSpecification.1.Tag.1.Value", "TeamA"));
    cases.push(("the same monitoring carrying tags", tagged));

    for (case, params) in &cases {
        refused_as_duplicate(&router, case, params, &held).await;
    }

    assert_eq!(
        engine.state::<Ec2State>().lock().flow_logs.len(),
        1,
        "every refusal must leave the store holding only the seeded flow log"
    );
}

/// A create that differs in ANY part of the capture configuration or in the destination is
/// SERVED and mints a flow log of its own. The rows differ in the monitored resource, its
/// kind, the traffic type, the log group, the destination type, the record format and the
/// aggregation interval — each row otherwise identical to the seeded flow log.
#[tokio::test]
async fn a_create_differing_in_the_configuration_or_the_destination_is_served() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.61.0.0/16").await;
    let other_vpc_id = create_vpc(&router, "10.62.0.0/16").await;
    let subnet_id = create_subnet(&router, &vpc_id, "10.61.1.0/24").await;
    let seed = baseline(&vpc_id, SEEDED_GROUP);
    let seeded = served(&router, "the seeded flow log", &seed).await;

    // Each row: the one thing it differs from the seed in, and the request.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "another monitored resource of the same kind",
            baseline(&other_vpc_id, SEEDED_GROUP),
        ),
        (
            "a monitored resource of another kind",
            with(
                &with(&seed, "ResourceType", "Subnet"),
                "ResourceId.1",
                &subnet_id,
            ),
        ),
        ("another traffic type", with(&seed, "TrafficType", "REJECT")),
        (
            "another log group",
            with(&seed, "LogGroupName", "/aws/vpc/another-group"),
        ),
        (
            "another record format",
            with(&seed, "LogFormat", "${srcaddr} ${dstaddr} ${action}"),
        ),
        (
            "another aggregation interval",
            with(&seed, "MaxAggregationInterval", "60"),
        ),
        (
            "another destination type",
            with(
                &with(
                    &without(&without(&seed, "LogGroupName"), "DeliverLogsPermissionArn"),
                    "LogDestinationType",
                    "s3",
                ),
                "LogDestination",
                "arn:aws:s3:::flow-logs-bucket/vpc/",
            ),
        ),
        (
            "another destination of the same type",
            with(
                &with(
                    &without(&without(&seed, "LogGroupName"), "DeliverLogsPermissionArn"),
                    "LogDestinationType",
                    "kinesis-data-firehose",
                ),
                "LogDestination",
                "arn:aws:firehose:us-east-1:000000000000:deliverystream/flow-logs",
            ),
        ),
    ];

    for (case, params) in &cases {
        let minted = served(&router, case, params).await;
        assert_ne!(
            minted, seeded,
            "case {case:?}: a served create must mint a flow log of its own, got the seeded \
             {seeded}"
        );
        // The row's flow log is given back, so every row decides against the seed alone and
        // the rows cannot differ from each other instead.
        delete(&router, case, &[&minted]).await;
        let stored = stored_ids(&router, case).await;
        assert_eq!(
            stored,
            vec![seeded.clone()],
            "case {case:?}: expected only the seeded flow log left, got {stored:?}"
        );
    }
}

/// An S3 flow log's DESTINATION OPTIONS are part of the configuration that makes two flow
/// logs the same: options left out and options spelled out at their documented defaults ask
/// for the same flow log, and a request differing in any one option is served. The rows
/// differ in which option they change and in which direction.
#[tokio::test]
async fn the_destination_options_are_part_of_the_configuration_compared() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.63.0.0/16").await;
    const BUCKET_ARN: &str = "arn:aws:s3:::flow-logs-bucket/vpc/";

    let seed = vec![
        param("ResourceType", "VPC"),
        param("ResourceId.1", &vpc_id),
        param("TrafficType", "ALL"),
        param("LogDestinationType", "s3"),
        param("LogDestination", BUCKET_ARN),
    ];
    let seeded = served(&router, "the seeded S3 flow log", &seed).await;
    let held = vec![seeded.clone()];

    // Each row: the request, and whether it asks for the flow log the seed already is. "The
    // default is plain-text" (`DestinationOptionsRequest$FileFormat`), "The default is false"
    // (`$HiveCompatiblePartitions`, `$PerHourPartition`).
    let cases: Vec<OptionCase> = vec![
        OptionCase {
            name: "the very same request again",
            params: seed.clone(),
            duplicate: true,
        },
        OptionCase {
            name: "every destination option spelled out at its documented default",
            params: {
                let mut same = seed.clone();
                same.push(param("DestinationOptions.FileFormat", "plain-text"));
                same.push(param("DestinationOptions.HiveCompatiblePartitions", "false"));
                same.push(param("DestinationOptions.PerHourPartition", "false"));
                same
            },
            duplicate: true,
        },
        OptionCase {
            name: "another file format",
            params: with(&seed, "DestinationOptions.FileFormat", "parquet"),
            duplicate: false,
        },
        OptionCase {
            name: "Hive-compatible prefixes",
            params: with(&seed, "DestinationOptions.HiveCompatiblePartitions", "true"),
            duplicate: false,
        },
        OptionCase {
            name: "per-hour partitions",
            params: with(&seed, "DestinationOptions.PerHourPartition", "true"),
            duplicate: false,
        },
    ];

    for OptionCase {
        name: case,
        params,
        duplicate,
    } in &cases
    {
        if *duplicate {
            refused_as_duplicate(&router, case, params, &held).await;
        } else {
            let minted = served(&router, case, params).await;
            delete(&router, case, &[&minted]).await;
            let stored = stored_ids(&router, case).await;
            assert_eq!(
                stored,
                held,
                "case {case:?}: expected only the seeded flow log left, got {stored:?}"
            );
        }
    }
}

/// The precondition is checked for the WHOLE batch before anything is stored: a call naming
/// one resource that already has the flow log stores none of them, and the resources beside
/// it are left exactly as they were. Once the existing flow log is deleted the same batch is
/// served and mints one flow log per resource. The rows differ in where in the batch the
/// duplicate sits.
#[tokio::test]
async fn one_duplicate_anywhere_in_the_batch_refuses_the_whole_create() {
    let (router, engine) = super::ec2_query::emulator();
    let first_vpc = create_vpc(&router, "10.64.0.0/16").await;
    let second_vpc = create_vpc(&router, "10.65.0.0/16").await;
    let third_vpc = create_vpc(&router, "10.66.0.0/16").await;
    let seeded = served(
        &router,
        "the seeded flow log",
        &baseline(&second_vpc, SEEDED_GROUP),
    )
    .await;
    let held = vec![seeded.clone()];

    // Each row: the resources the batch names, with the one that already has the flow log in
    // a different position each time.
    let cases: Vec<(&str, Vec<&String>)> = vec![
        ("the duplicate alone", vec![&second_vpc]),
        ("the duplicate first", vec![&second_vpc, &first_vpc]),
        ("the duplicate last", vec![&first_vpc, &second_vpc]),
        (
            "the duplicate between two fresh resources",
            vec![&first_vpc, &second_vpc, &third_vpc],
        ),
    ];

    for (case, resources) in &cases {
        let mut params = baseline(resources[0], SEEDED_GROUP);
        for (index, resource_id) in resources.iter().enumerate().skip(1) {
            params.push(param(&format!("ResourceId.{}", index + 1), resource_id));
        }
        refused_as_duplicate(&router, case, &params, &held).await;
    }

    // The flow log that blocked the batch is given back, and the batch that was refused for
    // it is then served whole: one flow log per resource it names.
    delete(&router, "the batch after the delete", &[&seeded]).await;
    let mut params = baseline(&first_vpc, SEEDED_GROUP);
    params.push(param("ResourceId.2", &second_vpc));
    params.push(param("ResourceId.3", &third_vpc));
    let (status, body) = create_flow_logs(&router, &params).await;
    assert_eq!(
        status, 200,
        "the batch after the delete: expected HTTP 200, got {status} with body {body}"
    );
    let minted = elements(&body, "item");
    assert_eq!(
        minted.len(),
        3,
        "the batch after the delete: expected one flow log per resource, got {minted:?} in \
         {body}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().flow_logs.len(),
        3,
        "the batch after the delete: expected the three flow logs it minted in the store"
    );
}

/// A `ClientToken` REPLAY is not a duplicate: "Unique, case-sensitive identifier that you
/// provide to ensure the idempotency of the request" (`CreateFlowLogsRequest$ClientToken`),
/// so a repeat of the same request under the same token reports the flow logs that token
/// already minted and turns on no second set — while the same request under a token of its
/// own is refused as the duplicate it is.
#[tokio::test]
async fn a_client_token_replay_reports_its_flow_logs_rather_than_being_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.67.0.0/16").await;
    let params = with(&baseline(&vpc_id, SEEDED_GROUP), "ClientToken", "a-token");

    let minted = served(&router, "the first create", &params).await;

    // Each step: the request, and the flow log ids it must answer — the replay reports what
    // the token minted, however many times it is repeated.
    for case in ["the first replay", "the second replay"] {
        let (status, body) = create_flow_logs(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "item"),
            vec![minted.clone()],
            "case {case:?}: expected the flow log {minted} the token already minted, got {body}"
        );
        assert_eq!(
            engine.state::<Ec2State>().lock().flow_logs.len(),
            1,
            "case {case:?}: a replay must turn on no second flow log"
        );
    }

    // The same monitoring asked for under another token is the duplicate it is.
    refused_as_duplicate(
        &router,
        "the same monitoring under another token",
        &with(&params, "ClientToken", "another-token"),
        std::slice::from_ref(&minted),
    )
    .await;
}

/// A create that provided NO `ClientToken` is answered the token its flow logs are recorded
/// under: "Unique, case-sensitive identifier that you provide to ensure the idempotency of
/// the request" (`CreateFlowLogsResult$ClientToken`) is reported whether or not the request
/// carried one, each call gets a token of its own, and the token a call was answered replays
/// exactly that call's flow logs rather than turning on a second set. The rows differ in the
/// monitored resource and the log group, so two calls that differ cannot share a token.
#[tokio::test]
async fn a_create_that_provided_no_client_token_is_answered_the_one_its_flow_logs_answer_to() {
    let (router, engine) = super::ec2_query::emulator();
    let first_vpc = create_vpc(&router, "10.68.0.0/16").await;
    let second_vpc = create_vpc(&router, "10.69.0.0/16").await;

    // Each row: the request, which carries no `ClientToken` at all.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("a flow log on one VPC", baseline(&first_vpc, SEEDED_GROUP)),
        (
            "a flow log on another VPC, to another log group",
            baseline(&second_vpc, "/aws/vpc/another-group"),
        ),
    ];

    let mut answered: Vec<String> = Vec::new();
    for (case, params) in &cases {
        let (status, body) = create_flow_logs(&router, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let minted = elements(&body, "item");
        assert_eq!(
            minted.len(),
            1,
            "case {case:?}: expected exactly one flow log id, got {minted:?} in {body}"
        );
        let token = element(&body, "clientToken").unwrap_or_default();
        assert!(
            !token.is_empty(),
            "case {case:?}: expected a clientToken on the answer, got {body}"
        );

        // The token NAMES those flow logs: a create carrying it reports them and turns on
        // nothing new, which is what makes the answered token worth having.
        let held = engine.state::<Ec2State>().lock().flow_logs.len();
        let replay = with(params, "ClientToken", &token);
        let (status, body) = create_flow_logs(&router, &replay).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replay expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "item"),
            minted,
            "case {case:?}: expected the replay to report {minted:?}, got {body}"
        );
        assert_eq!(
            engine.state::<Ec2State>().lock().flow_logs.len(),
            held,
            "case {case:?}: a replay under the answered token must turn on no second flow log"
        );
        answered.push(token);
    }

    // Two calls that asked for different flow logs were answered different tokens: the token
    // is the service's own value, not one read off the request.
    assert_ne!(
        answered[0], answered[1],
        "each create must be answered a token of its own, both got {token:?}",
        token = answered[0]
    );
}
