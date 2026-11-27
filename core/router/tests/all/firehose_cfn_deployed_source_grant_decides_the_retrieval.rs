// SPDX-License-Identifier: CC0-1.0

//! A stack that declares Kinesis-sourced Firehose streams whose source roles are then narrowed
//! to differ in what they may read: the stack deploys, and what each stream retrieves follows
//! its own grant.
//!
//! The rule under test: **`CreateDeliveryStream` resolves the RESOURCES a configuration names,
//! the roles themselves and the retrieval's own first call on the source — so a
//! `KinesisStreamSourceConfiguration` whose `RoleARN` may make the Kinesis Data Streams
//! calls the retrieval is made of (`kinesis:DescribeStream`, `kinesis:GetShardIterator`,
//! `kinesis:GetRecords`) is written, the stream it writes reports itself `ACTIVE`, and a
//! CloudFormation template declaring such a stream deploys to `CREATE_COMPLETE`. Which calls
//! the role may make LATER is read at each RETRIEVAL instead: one it may not make retrieves
//! nothing,
//! delivers nothing, and is reported as a log event whose `errorCode` is the fixed
//! `Kinesis.AccessDenied` in the log group and log stream the destination's
//! `CloudWatchLoggingOptions` name, naming the FIRST call the role is short of; granting that
//! call in place delivers the records the stream never took, with nothing recreated.**
//!
//! So one stack, one Kinesis data stream and one bucket, with three legs whose roles are
//! narrowed, once their streams are live, to differ only in their grant: the leg that may make
//! the whole read delivers the record and reports
//! nothing, and each leg short of one call delivers nothing and reports that call.
//!
//! Nothing the template declares carries a physical name: the bucket, the source stream, the
//! log group, the log streams, the roles and the delivery streams are all named by
//! CloudFormation, read back out of the stack's `Outputs`, and reached only by the names it
//! reports — so what the legs are asserted about is the pipeline the stack really built,
//! including the `Fn::GetAtt` and `Fn::Sub` references its policies and configurations are
//! written with.
//!
//! Only a cross-service suite can ask this: the flow spans CloudFormation (which deploys the
//! roles, the log destinations and the streams), IAM (which holds those roles and decides each
//! read), Amazon Kinesis Data Streams (the shared source), Firehose (which retrieves, buffers
//! and delivers), Amazon S3 (which holds the delivered objects) and CloudWatch Logs (which
//! holds the reported events), mounted on ONE engine exactly as `core/router/src/lib.rs`
//! mounts them. `firehose_create_resolves_its_configuration` pins what the create does and
//! does not resolve, and `firehose_kinesis_source_read_refusal` tables the same grants for a
//! stream a client created itself.
//!
//! Nothing here is asserted about how fast the host ran: every wait polls for the CONDITION
//! under a generous deadline, and what must stay empty is re-read at every one of those
//! readings.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The request builders the cross-service CloudFormation suites share, so this suite speaks the
// same wire as the ones beside it rather than building a second encoder of its own.
use crate::cloudformation_metric_stream_exports_to_s3::{cloudformation, send};
use crate::query_wire::{element, elements, form_encode, outputs_of};

/// The one stack every stream below is a resource of.
const STACK: &str = "cfn-firehose-source-grant";

/// How long the stack is given to report itself settled, and a stream to report itself usable.
const DEPLOY_DEADLINE: Duration = Duration::from_secs(120);

/// How long a delivery, or the report of a read that was refused, is waited for: far longer
/// than the emulator's own source poll, authorization span and buffer window together.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long what must hold NOTHING goes on being re-read once the conditions hold.
const SILENCE_WINDOW: Duration = Duration::from_secs(2);

/// How often a stack, a stream, a destination or a log stream is read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `StackStatus` a deployment the template declares correctly settles in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `DeliveryStreamStatus` a stream retrieves and delivers data in.
const ACTIVE: &str = "ACTIVE";

/// The `errorCode` a source read the stream's role may not make is reported under — fixed
/// text.
const REFUSED_ERROR_CODE: &str = "Kinesis.AccessDenied";

/// The trust policy every role carries: Firehose assumes the role a stream's source and
/// destination name, so the service principal has to be able to.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "firehose.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The name of the one inline policy the template puts on each role, and the name the repair
/// that RESTATES that policy re-states it under.
const POLICY_NAME: &str = "delivery";

/// The name of the policy a repair that ADDS one puts on the role beside the template's.
const REPAIR_POLICY_NAME: &str = "source-read-repair";

/// The Kinesis Data Streams calls a consumer of a stream is granted, as a role written for a
/// Firehose source states them — the read-only access to a data stream, of which the retrieval
/// makes the describe, the shard iterator and the records read.
const SOURCE_READS: &[&str] = &[
    "kinesis:DescribeStream",
    "kinesis:DescribeStreamSummary",
    "kinesis:GetShardIterator",
    "kinesis:ListShards",
    "kinesis:GetRecords",
];

/// The Amazon S3 calls a Firehose destination role is granted, of which the delivery itself
/// makes the object write.
const OBJECT_WRITES: &[&str] = &[
    "s3:AbortMultipartUpload",
    "s3:GetBucketLocation",
    "s3:GetObject",
    "s3:ListBucket",
    "s3:ListBucketMultipartUploads",
    "s3:PutObject",
];

/// The records the producer writes to the shared source in ONE `PutRecords`, which each
/// stream retrieves for itself: more than one, so what a leg delivers is every record the
/// source took and not just the first.
fn markers() -> Vec<Vec<u8>> {
    (0..2)
        .map(|at| format!("{STACK}-marker-{at}\n").into_bytes())
        .collect()
}

/// How the repair states the grant a leg's role was short of.
///
/// Both shapes a client repairs a role with, since the permissions in force on a role are
/// every policy it carries: the one inline policy the template wrote is REPLACED by a document
/// carrying the whole read, or a SECOND inline policy is put beside it stating only the call
/// that was missing.
#[derive(Debug, Clone, Copy)]
enum Repair {
    /// Restate the template's own policy, with the whole read in it.
    RestateThePolicy,
    /// Leave that policy as it is and add one that states the missing call alone.
    AddAPolicy,
}

/// One leg of the stack: a role, the log stream its destination reports into, and the
/// Kinesis-sourced stream itself.
///
/// The rows differ along what the rule generalises over: WHICH of the calls the retrieval is
/// made of the leg's role is short of — none, the records read, and the describe that comes
/// first — so what is asserted is the first call the role may not make and not one fixed
/// action.
#[derive(Debug, Clone, Copy)]
struct Leg {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The infix of this leg's logical ids and key prefix.
    key: &'static str,
    /// The call the leg's role is NOT granted once its stream is live, or `None` for the leg
    /// that may make the whole read. Every role is DEPLOYED with the whole read, because the
    /// create resolves the retrieval's first call under it.
    withheld: Option<&'static str>,
    /// How that call is granted once the deployed streams have been observed.
    repair: Repair,
}

/// The legs the template declares.
const LEGS: &[Leg] = &[
    Leg {
        case: "a role that may make the whole source read",
        key: "Good",
        withheld: None,
        repair: Repair::RestateThePolicy,
    },
    Leg {
        case: "a role one call short of the source read: the records",
        key: "Records",
        withheld: Some("kinesis:GetRecords"),
        // Granted by a policy put BESIDE the one the template wrote.
        repair: Repair::AddAPolicy,
    },
    Leg {
        case: "a role one call short of the source read: the describe",
        key: "Describe",
        withheld: Some("kinesis:DescribeStream"),
        // Granted by restating the policy the template wrote.
        repair: Repair::RestateThePolicy,
    },
];

impl Leg {
    /// The `Prefix` this leg's destination declares, which no other leg shares.
    fn prefix(self) -> String {
        format!("{}/", self.key.to_lowercase())
    }

    /// The logical id of one of this leg's declared resources.
    fn logical_id(self, kind: &str) -> String {
        format!("{kind}{}", self.key)
    }

    /// The source read calls this leg's role is left granted once its stream is live: the
    /// consumer's read minus whatever it is short of.
    fn granted(self) -> Vec<&'static str> {
        SOURCE_READS
            .iter()
            .copied()
            .filter(|action| Some(*action) != self.withheld)
            .collect()
    }
}

/// What the stack built, as the names it reports for it: nothing here is a name this suite
/// chose.
#[derive(Debug)]
struct Deployed {
    /// The destination bucket's name.
    bucket: String,
    /// The source Kinesis data stream's name.
    source: String,
    /// The log group both destinations report into.
    log_group: String,
    /// Per leg, the Firehose stream's name, its log stream's name and its role's name.
    legs: BTreeMap<&'static str, (String, String, String)>,
}

impl Deployed {
    /// The Firehose stream `leg` was deployed as.
    fn stream(&self, leg: Leg) -> &str {
        &self.legs[leg.key].0
    }

    /// The log stream `leg`'s destination reports into.
    fn log_stream(&self, leg: Leg) -> &str {
        &self.legs[leg.key].1
    }

    /// The role `leg`'s stream reads its source and writes its destination under.
    fn role(&self, leg: Leg) -> &str {
        &self.legs[leg.key].2
    }
}

/// The identity policy the template puts on a leg's role: the destination write on the
/// bucket the stack creates, the log write of its own reporting, and `reads` on the source
/// stream the stack creates — every resource named by reference, never by a name this suite
/// wrote.
fn policy_document(reads: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": OBJECT_WRITES,
                "Resource": [
                    { "Fn::GetAtt": ["Bucket", "Arn"] },
                    { "Fn::Sub": "${Bucket.Arn}/*" },
                ],
            },
            { "Effect": "Allow", "Action": "logs:PutLogEvents", "Resource": "*" },
            {
                "Effect": "Allow",
                "Action": reads,
                "Resource": { "Fn::GetAtt": ["Source", "Arn"] },
            },
        ],
    })
}

/// The stack's template: the one source, the one bucket, the one log group, and — per leg — a
/// role carrying that leg's own grant, the log stream its destination reports into, and the
/// Kinesis-sourced stream itself. Every physical name is CloudFormation's to choose, and the
/// `Outputs` report the ones it chose.
fn template() -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "Source".to_string(),
        json!({
            "Type": "AWS::Kinesis::Stream",
            "Properties": { "ShardCount": 1 },
        }),
    );
    resources.insert(
        "Bucket".to_string(),
        json!({ "Type": "AWS::S3::Bucket" }),
    );
    resources.insert(
        "LogGroup".to_string(),
        json!({
            "Type": "AWS::Logs::LogGroup",
            "Properties": { "RetentionInDays": 1 },
        }),
    );
    let mut outputs = serde_json::Map::new();
    outputs.insert(
        "BucketName".to_string(),
        json!({ "Value": { "Ref": "Bucket" } }),
    );
    outputs.insert(
        "SourceName".to_string(),
        json!({ "Value": { "Ref": "Source" } }),
    );
    outputs.insert(
        "LogGroupName".to_string(),
        json!({ "Value": { "Ref": "LogGroup" } }),
    );
    for leg in LEGS {
        resources.insert(
            leg.logical_id("Role"),
            json!({
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": trust_policy(),
                    "Policies": [{
                        "PolicyName": POLICY_NAME,
                        // The leg's OWN grant, as the template declares it: the condition
                        // under test is deployed, not stated afterwards.
                        "PolicyDocument": policy_document(SOURCE_READS),
                    }],
                },
            }),
        );
        resources.insert(
            leg.logical_id("LogStream"),
            json!({
                "Type": "AWS::Logs::LogStream",
                // By reference, so the group holding the stream is created first.
                "Properties": { "LogGroupName": { "Ref": "LogGroup" } },
            }),
        );
        resources.insert(
            leg.logical_id("Stream"),
            json!({
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamType": "KinesisStreamAsSource",
                    "KinesisStreamSourceConfiguration": {
                        "KinesisStreamARN": { "Fn::GetAtt": ["Source", "Arn"] },
                        "RoleARN": { "Fn::GetAtt": [leg.logical_id("Role"), "Arn"] },
                    },
                    "S3DestinationConfiguration": {
                        "BucketARN": { "Fn::GetAtt": ["Bucket", "Arn"] },
                        "RoleARN": { "Fn::GetAtt": [leg.logical_id("Role"), "Arn"] },
                        "CompressionFormat": "UNCOMPRESSED",
                        "Prefix": leg.prefix(),
                        // The shortest wait `IntervalInSeconds` admits (published range
                        // 0..900) with the smallest `SizeInMBs` (1..128): the legs are about
                        // WHETHER a record is retrieved, not about a window they would wait
                        // out.
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                        "CloudWatchLoggingOptions": {
                            "Enabled": true,
                            "LogGroupName": { "Ref": "LogGroup" },
                            "LogStreamName": { "Ref": leg.logical_id("LogStream") },
                        },
                    },
                },
                // The destination reports into that log stream, so the channel exists before
                // the stream writing into it does.
                "DependsOn": [leg.logical_id("LogStream")],
            }),
        );
        outputs.insert(
            format!("Stream{}", leg.key),
            json!({ "Value": { "Ref": leg.logical_id("Stream") } }),
        );
        outputs.insert(
            format!("LogStream{}", leg.key),
            json!({ "Value": { "Ref": leg.logical_id("LogStream") } }),
        );
        outputs.insert(
            format!("Role{}", leg.key),
            json!({ "Value": { "Ref": leg.logical_id("Role") } }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "Kinesis-sourced Firehose streams whose source grants differ",
        "Resources": Value::Object(resources),
        "Outputs": Value::Object(outputs),
    })
    .to_string()
}

/// The six services this flow spans, on ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not any of these services'.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

#[tokio::test]
async fn a_stack_whose_source_roles_are_narrowed_once_live_reports_the_refusal() {
    let router = emulator();

    // ---- The stack: every leg's role, log destination and delivery stream, each role
    // declaring the whole source read — which is what lets every leg's stream be written at all,
    // since the create resolves the retrieval's first call under the role it names. ----
    let body = template();
    let (status, answered) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &body),
            // The template declares IAM roles, which CloudFormation names itself.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );
    let described = await_settled(&router).await;
    let reported = element(&described, "StackStatus");
    if reported != CREATE_COMPLETE {
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", STACK)]).await;
        let reasons = elements(&events, "ResourceStatusReason");
        panic!(
            "expected the stack to reach {CREATE_COMPLETE}, got {reported} with resource \
             failures {reasons:?}"
        );
    }
    let deployed = deployed_names(&described);
    for leg in LEGS {
        await_stream_active(&router, &deployed, *leg).await;
    }

    // ---- Now each leg's role is NARROWED to the grant its row states, replacing the policy the
    // template put on it: what is in force on a role is read at the next retrieval, so from here
    // on each leg retrieves exactly what its own policy allows. ----
    for leg in LEGS.iter().filter(|leg| leg.withheld.is_some()) {
        let (status, answered) = iam(
            &router,
            "PutRolePolicy",
            &[
                ("RoleName", deployed.role(*leg)),
                ("PolicyName", POLICY_NAME),
                ("PolicyDocument", &narrowed_document(&deployed, *leg)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the narrowed policy must be put, got {status} with {answered}",
            leg.case
        );
    }

    // ---- Each leg short of a call reports it, under the one error code that condition
    // carries, naming the first call its role may not make, the source and the role. The
    // record goes to the source only once every such report is there, so every reading below
    // is of a retrieval the deployed policies decided. ----
    for leg in LEGS.iter().filter(|leg| leg.withheld.is_some()) {
        let withheld = leg.withheld.unwrap_or_default();
        let document = await_reported(&router, &deployed, *leg).await;
        assert_eq!(
            document["errorCode"],
            json!(REFUSED_ERROR_CODE),
            "case {:?}: expected the reported errorCode {REFUSED_ERROR_CODE:?}, got {document}",
            leg.case
        );
        let stream_arn = document["deliveryStreamARN"].as_str().unwrap_or_default();
        assert!(
            stream_arn.starts_with("arn:aws:firehose:")
                && stream_arn.ends_with(&format!(":deliverystream/{}", deployed.stream(*leg))),
            "case {:?}: expected the event to name its own stream, got {document}",
            leg.case
        );
        let message = document["message"].as_str().unwrap_or_default().to_string();
        for named in [withheld, &deployed.source, deployed.role(*leg)] {
            assert!(
                message.contains(named),
                "case {:?}: expected the message to name {named:?}, got {message:?}",
                leg.case
            );
        }
    }

    // ---- The records on the shared source, written in one call as a producer writes a
    // batch, which each deployed stream retrieves for itself — if its role lets it. ----
    let (status, answered) = json_rpc(
        &router,
        "kinesis",
        "Kinesis_20131202.PutRecords",
        &json!({
            "StreamName": deployed.source,
            "Records": markers()
                .iter()
                .enumerate()
                .map(|(at, record)| json!({
                    "PartitionKey": format!("pk-marker-{at}"),
                    "Data": ::aws_smithy_types::base64::encode(record.clone()),
                }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        (status, &answered["FailedRecordCount"]),
        (200, &json!(0)),
        "expected the source to accept the records, got {status} with {answered}"
    );

    // ---- The leg whose role may read delivers every record the source took. ----
    let permitted = LEGS
        .iter()
        .find(|leg| leg.withheld.is_none())
        .copied()
        .expect("one leg may make the whole read");
    await_markers_delivered(&router, &deployed, permitted).await;

    // ---- And every refused retrieval delivers nothing, while the permitted stream reports
    // nothing at all, at every reading over one more window. ----
    let until = Instant::now() + SILENCE_WINDOW;
    while Instant::now() < until {
        for leg in LEGS.iter().filter(|leg| leg.withheld.is_some()) {
            let keys = keys_under(&router, &deployed, &leg.prefix()).await;
            assert!(
                keys.is_empty(),
                "case {:?}: a refused retrieval delivers nothing, got {keys:?}",
                leg.case
            );
        }
        let reported = reported_events(&router, &deployed, permitted).await;
        assert!(
            reported.is_empty(),
            "case {:?}: a retrieval that was made reports nothing, got {reported:?}",
            permitted.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // ---- The repair: the call each role was short of is granted IN PLACE, on the role the
    // deployed stream already names — by restating that role's own policy for one leg and by
    // putting a second policy beside it for the other, since what is in force on a role is
    // every policy it carries. The records the streams never took are still in the source, so
    // the retrieval they could not make is made now and each destination holds them. ----
    for leg in LEGS.iter().filter(|leg| leg.withheld.is_some()) {
        let withheld = leg.withheld.unwrap_or_default();
        let (policy_name, document) = match leg.repair {
            Repair::RestateThePolicy => (POLICY_NAME, repair_document(&deployed)),
            Repair::AddAPolicy => (REPAIR_POLICY_NAME, granted_call_document(withheld)),
        };
        let (status, answered) = iam(
            &router,
            "PutRolePolicy",
            &[
                ("RoleName", deployed.role(*leg)),
                ("PolicyName", policy_name),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from PutRolePolicy, got {status} with {answered}",
            leg.case
        );
    }
    for leg in LEGS.iter().filter(|leg| leg.withheld.is_some()) {
        await_markers_delivered(&router, &deployed, *leg).await;
    }
}

/// Wait until every record the source took is held under `leg`'s prefix.
///
/// One retrieved record is one delivered record, however many objects the destination's
/// buffering wrote them as, so what is asserted is the bytes the prefix holds altogether.
async fn await_markers_delivered(router: &MultiServiceRouter, deployed: &Deployed, leg: Leg) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let expected = markers();
    loop {
        let mut held = Vec::new();
        for key in keys_under(router, deployed, &leg.prefix()).await {
            held.extend(object(router, deployed, &key).await);
        }
        if expected.iter().all(|marker| holds_subslice(&held, marker)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected every record the source took under {:?} within \
             {DELIVERY_DEADLINE:?}, got {:?}",
            leg.case,
            leg.prefix(),
            String::from_utf8_lossy(&held)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether `held` carries `wanted` somewhere in it.
fn holds_subslice(held: &[u8], wanted: &[u8]) -> bool {
    held.windows(wanted.len()).any(|window| window == wanted)
}

/// The policy a repair that ADDS one states: the single call the role was short of, on every
/// resource — a grant stated beside the policy the role already carries rather than instead of
/// it.
fn granted_call_document(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
    })
    .to_string()
}

/// The policy a leg's role is NARROWED to once its stream is live, in place of the whole read
/// the template deployed it with: the same destination write and log write, and only the source
/// read calls its row is left with — written over the names the stack reported, since the
/// template's references are not available to a client calling IAM directly.
fn narrowed_document(deployed: &Deployed, leg: Leg) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": OBJECT_WRITES,
                "Resource": [
                    format!("arn:aws:s3:::{}", deployed.bucket),
                    format!("arn:aws:s3:::{}/*", deployed.bucket),
                ],
            },
            { "Effect": "Allow", "Action": "logs:PutLogEvents", "Resource": "*" },
            { "Effect": "Allow", "Action": leg.granted(), "Resource": "*" },
        ],
    })
    .to_string()
}

/// The policy the repair states in place of the leg's own: the same destination write and log
/// write, with the WHOLE source read — written over the names the stack reported, since the
/// template's references are not available to a client calling IAM directly.
fn repair_document(deployed: &Deployed) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": OBJECT_WRITES,
                "Resource": [
                    format!("arn:aws:s3:::{}", deployed.bucket),
                    format!("arn:aws:s3:::{}/*", deployed.bucket),
                ],
            },
            { "Effect": "Allow", "Action": "logs:PutLogEvents", "Resource": "*" },
            { "Effect": "Allow", "Action": SOURCE_READS, "Resource": "*" },
        ],
    })
    .to_string()
}

/// The names the stack reports for what it built, read out of its `Outputs`.
fn deployed_names(described: &str) -> Deployed {
    let outputs = outputs_of(described);
    let read = |key: &str| -> String {
        outputs
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("expected the stack to report {key:?}, got {outputs:?}"))
    };
    Deployed {
        bucket: read("BucketName"),
        source: read("SourceName"),
        log_group: read("LogGroupName"),
        legs: LEGS
            .iter()
            .map(|leg| {
                (
                    leg.key,
                    (
                        read(&format!("Stream{}", leg.key)),
                        read(&format!("LogStream{}", leg.key)),
                        read(&format!("Role{}", leg.key)),
                    ),
                )
            })
            .collect(),
    }
}

/// Poll the stack until it first reports a settled status, and answer the `DescribeStacks`
/// body that status was read from.
async fn await_settled(router: &MultiServiceRouter) -> String {
    let deadline = Instant::now() + DEPLOY_DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", STACK)]).await;
        assert_eq!(
            status, 200,
            "expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        let reported = element(&body, "StackStatus");
        if reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED") {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "expected the stack to settle in {CREATE_COMPLETE}, last saw {reported:?} in {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll Firehose until it reports `leg`'s stream `ACTIVE`, reading the source and the role the
/// template handed it.
async fn await_stream_active(router: &MultiServiceRouter, deployed: &Deployed, leg: Leg) {
    let deadline = Instant::now() + DEPLOY_DEADLINE;
    loop {
        let (status, body) = json_rpc(
            router,
            "firehose",
            "Firehose_20150804.DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": deployed.stream(leg) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DescribeDeliveryStream, got {status} with {body}",
            leg.case
        );
        let description = &body["DeliveryStreamDescription"];
        if description["DeliveryStreamStatus"] == json!(ACTIVE) {
            let source = &description["Source"]["KinesisStreamSourceDescription"];
            assert!(
                source["KinesisStreamARN"]
                    .as_str()
                    .is_some_and(|arn| arn.ends_with(&format!("stream/{}", deployed.source)))
                    && source["RoleARN"]
                        .as_str()
                        .is_some_and(|arn| arn.ends_with(&format!("role/{}", deployed.role(leg)))),
                "case {:?}: expected the stream to report the source it reads and the role it \
                 reads it under, got {description}",
                leg.case
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected the stream to be {ACTIVE}, got {description}",
            leg.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until `leg`'s log stream carries its first reported event, and answer that document.
async fn await_reported(router: &MultiServiceRouter, deployed: &Deployed, leg: Leg) -> Value {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        if let Some(document) = reported_events(router, deployed, leg)
            .await
            .into_iter()
            .next()
        {
            return document;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected a reported event in {:?} / {:?} within {DELIVERY_DEADLINE:?}",
            leg.case,
            deployed.log_group,
            deployed.log_stream(leg)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every document `leg`'s log stream holds, read through CloudWatch Logs' own `GetLogEvents`.
async fn reported_events(
    router: &MultiServiceRouter,
    deployed: &Deployed,
    leg: Leg,
) -> Vec<Value> {
    let (status, body) = json_rpc(
        router,
        "logs",
        "Logs_20140328.GetLogEvents",
        &json!({
            "logGroupName": deployed.log_group,
            "logStreamName": deployed.log_stream(leg),
            "startFromHead": true,
        }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|event| event["message"].as_str())
        .filter_map(|message| serde_json::from_str::<Value>(message).ok())
        .collect()
}

/// Every key the shared bucket holds under `prefix`, read through Amazon S3's own
/// `ListObjectsV2`. Each leg delivers at most one object, so one page holds them all.
async fn keys_under(
    router: &MultiServiceRouter,
    deployed: &Deployed,
    prefix: &str,
) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{}?list-type=2&prefix={prefix}", deployed.bucket),
        &[],
        Vec::new(),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListObjectsV2 on {} under {prefix:?}, got {status} with {text}",
        deployed.bucket
    );
    let mut keys = elements(&text, "Key");
    keys.sort();
    keys
}

/// The bytes one delivered object holds, read through `GetObject`.
async fn object(router: &MultiServiceRouter, deployed: &Deployed, key: &str) -> Vec<u8> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{}/{key}", deployed.bucket),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "expected HTTP 200 from GetObject for {key:?}, got {status} with {}",
        String::from_utf8_lossy(&body)
    );
    body
}

/// One `awsJson1_1` operation of `signing_name`, answering its status and parsed body.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    request: &Value,
) -> (u16, Value) {
    let (status, body) = send(
        router,
        signing_name,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        request.to_string().into_bytes(),
    )
    .await;
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One IAM action over its `awsQuery` wire, answering its status and body text.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = send(
        router,
        "iam",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await;
    (status, String::from_utf8_lossy(&answered).to_string())
}
