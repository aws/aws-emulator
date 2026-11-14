// SPDX-License-Identifier: CC0-1.0

//! The DRIFT DETECTION EVENTS CloudFormation publishes onto the account's default
//! EventBridge bus, observed where a client observes them: on the Amazon SQS queue a rule on
//! that bus delivers to.
//!
//! The rule pinned here: **every status a drift-detection operation reaches is published onto
//! the account's default event bus with `source` `aws.cloudformation` and `detail-type`
//! "CloudFormation Drift Detection Status Change", in the stack's own account and Region,
//! naming the stack's id both in the event's `resources` and in `detail.stack-id` and the id
//! the API call answered in `detail.stack-drift-detection-id` — one event when the detection
//! is accepted (`detail.status-details.detection-status` `DETECTION_IN_PROGRESS`, with no
//! verdict and no count yet: `stack-drift-status` `""` and
//! `detail.drift-detection-details.drifted-stack-resource-count` `-1`) and one when it
//! concludes (`DETECTION_COMPLETE`, with the stack-level verdict and the number of the
//! stack's resources that really drifted).** Each event states the detection record a read of
//! the same instant answers from, so `DescribeStackDriftDetectionStatus` and the events cannot
//! disagree — and because the events go through the BUS, a rule whose pattern requires
//! `detail.status-details.stack-drift-status` `DRIFTED` receives a drifted detection's
//! conclusion and nothing of a detection that found the stack in sync.
//!
//! Four crates have to agree for that to hold — CloudFormation publishes, EventBridge matches
//! the event against the rules of the bus and hands it to their targets, Amazon SQS holds what
//! was delivered and is also the service that owns one of the drifted resources, and Systems
//! Manager owns the other — so the suite lives here rather than in any one of them. Nothing is
//! asserted on an emulator internal: every observation is a client call.
//!
//! The rows differ along what the rule generalises over: whether the stack drifted at all, how
//! many of its resources were changed outside it and therefore which count the conclusion
//! carries (0, 1 of 1, 1 of 2, 2 of 2), the type of the resource changed out of band (an
//! Amazon SQS queue's attribute, an AWS Systems Manager parameter's value) and how many
//! resources the detection covers. The recorded case — one queue, found in sync — is the first
//! row.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element, physical_ids_of};

/// The Region every call below is signed for, which is the Region the stacks of this suite
/// belong to and the Region their events are published in.
const REGION: &str = "us-west-2";

/// The account this emulator serves, which is the account the events carry.
const ACCOUNT: &str = "000000000000";

/// The `source` every event CloudFormation publishes carries.
const SOURCE: &str = "aws.cloudformation";

/// The one `detail-type` a drift detection's status changes are published under.
const DRIFT_CHANGE: &str = "CloudFormation Drift Detection Status Change";

/// The two detection statuses asserted on, as the model spells them.
const DETECTION_IN_PROGRESS: &str = "DETECTION_IN_PROGRESS";
const DETECTION_COMPLETE: &str = "DETECTION_COMPLETE";

/// The two stack-level verdicts a concluded detection reaches, as the model spells them.
const IN_SYNC: &str = "IN_SYNC";
const DRIFTED: &str = "DRIFTED";

/// What a detection that has not concluded states where the verdict and the count go: no
/// verdict yet, and a count no real detection reports.
const NO_VERDICT_YET: &str = "";
const NO_COUNT_YET: i64 = -1;

/// How long a poll or a drain waits for something the clock owes, and how often it looks.
const BUDGET: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// The whole emulator over one engine, exactly as the binary assembles it: which service
/// claims each call — CloudFormation's own in-process `PutEvents` and EventBridge's delivery
/// into Amazon SQS included — is decided by the production mount.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service` and `region`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call, as its status and XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name, REGION))
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name, REGION))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `(key, value)` request member, owned so a row can build names like `Tags.member.1`.
fn member(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One CloudFormation call that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(String, String)],
) -> String {
    let (status, body) = query(router, "cloudformation", "2010-05-15", action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// One EventBridge call that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Systems Manager call that must answer 200.
async fn ssm(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "ssm", "AmazonSSM", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Systems Manager {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// A queue whose own policy admits the delivering service, and a rule on the default bus that
/// delivers every event matching `pattern` to it: the queue's url.
///
/// "For Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
/// resource-based policies" (`PutTargets`), so the queue carries the statement that admits
/// `events.amazonaws.com`; the rule is created BEFORE the stack, since an event published
/// while the bus carries no matching rule reaches no target.
async fn wired_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    pattern: &Value,
) -> String {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    events(
        router,
        case,
        "PutRule",
        &json!({ "Name": format!("{name}-rule"), "EventPattern": pattern.to_string() }),
    )
    .await;
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": format!("{name}-rule"),
            "Targets": [{ "Id": "queue", "Arn": arn }],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "case {case:?}: the queue target must be accepted, got {targeted}"
    );
    url
}

/// The pattern of the rule that observes EVERY drift-detection event of this service.
fn every_drift_event() -> Value {
    json!({ "source": [SOURCE], "detail-type": [DRIFT_CHANGE] })
}

/// The pattern of the rule that observes only an event whose detection found the stack
/// DRIFTED — the operator's "tell me when something has drifted" rule.
fn only_drifted_events() -> Value {
    json!({
        "source": [SOURCE],
        "detail-type": [DRIFT_CHANGE],
        "detail": { "status-details": { "stack-drift-status": [DRIFTED] } },
    })
}

/// Every event body waiting on the queue at `url`, parsed, and taken off the queue.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|message| message["Body"].as_str())
        .map(|body| serde_json::from_str(body).unwrap_or(Value::Null))
        .collect()
}

/// Drain the queue at `url` into `seen` until `enough` is satisfied or [`BUDGET`] runs out.
async fn collect_until(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    seen: &mut Vec<Value>,
    enough: impl Fn(&[Value]) -> bool,
) {
    let deadline = Instant::now() + BUDGET;
    loop {
        seen.extend(drain(router, case, url).await);
        if enough(seen) || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The detection status the event reports.
fn detection_status_of(event: &Value) -> &str {
    event["detail"]["status-details"]["detection-status"]
        .as_str()
        .unwrap_or_default()
}

/// The detection id the event is joined to its call by.
fn detection_id_of(event: &Value) -> &str {
    event["detail"]["stack-drift-detection-id"]
        .as_str()
        .unwrap_or_default()
}

/// The instant the event carries.
///
/// Compared as TEXT where this suite orders two events: the envelope's `time` is an instant at
/// whole-second precision closed by `Z` (`2026-01-01T00:00:00Z`), one fixed-width form for
/// every event, so lexicographic order over it is chronological order.
fn time_of(event: &Value) -> &str {
    event["time"].as_str().unwrap_or_default()
}

/// The one event of `detection_id` reporting `status`, or `None` for none.
fn one_with_status<'a>(seen: &'a [Value], detection_id: &str, status: &str) -> Option<&'a Value> {
    seen.iter().find(|event| {
        event["detail-type"] == DRIFT_CHANGE
            && detection_id_of(event) == detection_id
            && detection_status_of(event) == status
    })
}

/// What every drift-detection event must carry, whichever status it reports.
fn assert_envelope(case: &str, event: &Value, stack_id: &str, detection_id: &str) {
    for (member, expected) in [
        ("source", json!(SOURCE)),
        ("detail-type", json!(DRIFT_CHANGE)),
        ("account", json!(ACCOUNT)),
        ("region", json!(REGION)),
        ("resources", json!([stack_id])),
    ] {
        assert_eq!(
            event[member], expected,
            "case {case:?}: expected {member} {expected}, got {event}"
        );
    }
    for member in ["version", "id", "time"] {
        let carried = event[member].as_str().unwrap_or_default();
        assert!(
            !carried.is_empty(),
            "case {case:?}: the envelope must carry a {member}, got {event}"
        );
    }
    assert_eq!(
        event["detail"]["stack-id"], json!(stack_id),
        "case {case:?}: expected detail.stack-id {stack_id:?}, got {event}"
    );
    assert_eq!(
        event["detail"]["stack-drift-detection-id"],
        json!(detection_id),
        "case {case:?}: expected detail.stack-drift-detection-id {detection_id:?}, got {event}"
    );
}

/// What one event states about the detection's verdict and count.
fn assert_verdict(case: &str, event: &Value, verdict: &str, count: i64) {
    let reported = event["detail"]["status-details"]["stack-drift-status"].clone();
    assert_eq!(
        reported, json!(verdict),
        "case {case:?}: expected detail.status-details.stack-drift-status {verdict:?}, got \
         {event}"
    );
    let reported = event["detail"]["drift-detection-details"]["drifted-stack-resource-count"]
        .as_i64()
        .unwrap_or_default();
    assert_eq!(
        reported, count,
        "case {case:?}: expected drifted-stack-resource-count {count}, got {event}"
    );
}

/// Create the stack `name` from `template` and poll it to a terminal status: its stack id.
async fn deployed_stack(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    template: &str,
) -> String {
    let created = cloudformation(
        router,
        case,
        "CreateStack",
        &[member("StackName", name), member("TemplateBody", template)],
    )
    .await;
    let stack_id = opt_element(&created, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateStack answered no StackId: {created}"));
    let deadline = Instant::now() + BUDGET;
    loop {
        let body = cloudformation(
            router,
            case,
            "DescribeStacks",
            &[member("StackName", &stack_id)],
        )
        .await;
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            assert_eq!(
                reported, "CREATE_COMPLETE",
                "case {case:?}: the stack must deploy, got {reported} with {body}"
            );
            return stack_id;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack was still {reported} at the deadline"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The physical id the stack's resource of `logical_id` was deployed under.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack_id: &str,
    logical_id: &str,
) -> String {
    let body = cloudformation(
        router,
        case,
        "DescribeStackResources",
        &[member("StackName", stack_id)],
    )
    .await;
    physical_ids_of(&body)
        .get(logical_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack deployed no {logical_id}: {body}"))
}

/// Start a detection on `stack_id` and answer the `StackDriftDetectionId` it was accepted
/// under.
async fn detect_drift(router: &MultiServiceRouter, case: &str, stack_id: &str) -> String {
    let body = cloudformation(
        router,
        case,
        "DetectStackDrift",
        &[member("StackName", stack_id)],
    )
    .await;
    opt_element(&body, "StackDriftDetectionId").unwrap_or_else(|| {
        panic!("case {case:?}: DetectStackDrift answered no StackDriftDetectionId: {body}")
    })
}

/// Poll `DescribeStackDriftDetectionStatus` until the detection concludes: its
/// `(DetectionStatus, StackDriftStatus, DriftedStackResourceCount)`.
async fn detection_settles(
    router: &MultiServiceRouter,
    case: &str,
    detection_id: &str,
) -> (String, String, i64) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let body = cloudformation(
            router,
            case,
            "DescribeStackDriftDetectionStatus",
            &[member("StackDriftDetectionId", detection_id)],
        )
        .await;
        let status = opt_element(&body, "DetectionStatus").unwrap_or_default();
        if status != DETECTION_IN_PROGRESS || Instant::now() >= deadline {
            let verdict = opt_element(&body, "StackDriftStatus").unwrap_or_default();
            let count = opt_element(&body, "DriftedStackResourceCount")
                .and_then(|text| text.parse::<i64>().ok())
                .unwrap_or(NO_COUNT_YET);
            return (status, verdict, count);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// What a row does to the stack's resources, outside the stack, before the detection runs.
#[derive(Debug, Clone, Copy)]
enum Tamper {
    /// Set the queue of this logical id to another `VisibilityTimeout`, in Amazon SQS.
    QueueVisibilityTimeout(&'static str, &'static str),
    /// Write another value into the parameter of this logical id, in Systems Manager.
    ParameterValue(&'static str, &'static str),
}

impl Tamper {
    /// Which logical id of the stack this change is made to.
    fn logical_id(self) -> &'static str {
        match self {
            Self::QueueVisibilityTimeout(logical_id, _) | Self::ParameterValue(logical_id, _) => {
                logical_id
            }
        }
    }

    /// Make the change, through the owning service's own API.
    async fn apply(self, router: &MultiServiceRouter, case: &str, physical_id: &str) {
        match self {
            Self::QueueVisibilityTimeout(_, seconds) => {
                sqs(
                    router,
                    case,
                    "SetQueueAttributes",
                    &json!({
                        "QueueUrl": physical_id,
                        "Attributes": { "VisibilityTimeout": seconds },
                    }),
                )
                .await;
            }
            Self::ParameterValue(_, value) => {
                ssm(
                    router,
                    case,
                    "PutParameter",
                    &json!({
                        "Name": physical_id,
                        "Value": value,
                        "Type": "String",
                        "Overwrite": true,
                    }),
                )
                .await;
            }
        }
    }
}

/// One row of the table: a stack to deploy, what is changed outside it, and the verdict and
/// count its detection must therefore reach.
struct Row {
    case: &'static str,
    /// The stack's name, which also names this row's queues and rules.
    stack: &'static str,
    template: String,
    tampers: Vec<Tamper>,
    verdict: &'static str,
    drifted: i64,
}

/// The class the rule covers: any stack, with any number of its resources changed outside it,
/// of either resource type an owning service here reports a configuration for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one queue, nothing touched outside the stack",
            stack: "drift-events-in-sync",
            template: json!({
                "Resources": {
                    "Subject": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": {
                            "QueueName": "drift-events-in-sync-subject",
                            "VisibilityTimeout": 30,
                        },
                    },
                },
            })
            .to_string(),
            tampers: Vec::new(),
            verdict: IN_SYNC,
            drifted: 0,
        },
        Row {
            case: "one queue whose VisibilityTimeout was changed outside the stack",
            stack: "drift-events-one-queue",
            template: json!({
                "Resources": {
                    "Subject": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": {
                            "QueueName": "drift-events-one-queue-subject",
                            "VisibilityTimeout": 30,
                        },
                    },
                },
            })
            .to_string(),
            tampers: vec![Tamper::QueueVisibilityTimeout("Subject", "45")],
            verdict: DRIFTED,
            drifted: 1,
        },
        Row {
            case: "two queues, one of them changed outside the stack",
            stack: "drift-events-two-queues",
            template: json!({
                "Resources": {
                    "Subject": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": {
                            "QueueName": "drift-events-two-queues-subject",
                            "VisibilityTimeout": 30,
                        },
                    },
                    "Untouched": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": {
                            "QueueName": "drift-events-two-queues-untouched",
                            "VisibilityTimeout": 30,
                        },
                    },
                },
            })
            .to_string(),
            tampers: vec![Tamper::QueueVisibilityTimeout("Subject", "60")],
            verdict: DRIFTED,
            drifted: 1,
        },
        Row {
            case: "two parameters, both changed outside the stack",
            stack: "drift-events-two-parameters",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/drift-events/first",
                            "Type": "String",
                            "Value": "declared",
                        },
                    },
                    "Second": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/drift-events/second",
                            "Type": "String",
                            "Value": "declared",
                        },
                    },
                },
            })
            .to_string(),
            tampers: vec![
                Tamper::ParameterValue("First", "changed-by-hand"),
                Tamper::ParameterValue("Second", "also-changed"),
            ],
            verdict: DRIFTED,
            drifted: 2,
        },
    ]
}

/// Every drift detection publishes its start and its conclusion onto the default bus, and the
/// DRIFTED-only rule receives a drifted detection's conclusion and nothing of an in-sync one.
#[tokio::test]
async fn a_drift_detection_publishes_its_start_and_its_conclusion_onto_the_default_bus() {
    for row in rows() {
        let case = row.case;
        // A fresh emulator per row, so one row's events never decide another's.
        let router = router();
        let control = wired_queue(
            &router,
            case,
            &format!("{}-control", row.stack),
            &every_drift_event(),
        )
        .await;
        let claim = wired_queue(
            &router,
            case,
            &format!("{}-claim", row.stack),
            &only_drifted_events(),
        )
        .await;

        let stack_id = deployed_stack(&router, case, row.stack, &row.template).await;
        for tamper in &row.tampers {
            let physical = physical_id(&router, case, &stack_id, tamper.logical_id()).await;
            tamper.apply(&router, case, &physical).await;
        }

        let detection_id = detect_drift(&router, case, &stack_id).await;

        // 1. The pair of events the detection owes, observed where a client observes them.
        let mut seen: Vec<Value> = Vec::new();
        collect_until(&router, case, &control, &mut seen, |seen| {
            one_with_status(seen, &detection_id, DETECTION_IN_PROGRESS).is_some()
                && one_with_status(seen, &detection_id, DETECTION_COMPLETE).is_some()
        })
        .await;

        let in_progress = one_with_status(&seen, &detection_id, DETECTION_IN_PROGRESS)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {DRIFT_CHANGE:?} event reporting \
                     {DETECTION_IN_PROGRESS} for {detection_id}, got {seen:?}"
                )
            })
            .clone();
        let complete = one_with_status(&seen, &detection_id, DETECTION_COMPLETE)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {DRIFT_CHANGE:?} event reporting \
                     {DETECTION_COMPLETE} for {detection_id}, got {seen:?}"
                )
            })
            .clone();

        assert_envelope(case, &in_progress, &stack_id, &detection_id);
        assert_envelope(case, &complete, &stack_id, &detection_id);
        // A detection that has not concluded has reached no verdict and counted nothing.
        assert_verdict(case, &in_progress, NO_VERDICT_YET, NO_COUNT_YET);
        assert_verdict(case, &complete, row.verdict, row.drifted);
        assert!(
            time_of(&in_progress) <= time_of(&complete),
            "case {case:?}: the start cannot be stamped after the conclusion, got {} and {}",
            time_of(&in_progress),
            time_of(&complete)
        );

        // 2. What the API reports for the same detection is what the events stated.
        let (status, verdict, count) = detection_settles(&router, case, &detection_id).await;
        assert_eq!(
            (status.as_str(), verdict.as_str(), count),
            (DETECTION_COMPLETE, row.verdict, row.drifted),
            "case {case:?}: the events and DescribeStackDriftDetectionStatus must agree"
        );

        // 3. The DRIFTED-only rule. Both events have already reached the control queue, so
        // EventBridge has matched each of them against every rule of the bus: what this rule
        // did not match, it never will.
        let claimed = drain(&router, case, &claim).await;
        let expected: Vec<&str> = if row.verdict == DRIFTED {
            vec![DETECTION_COMPLETE]
        } else {
            Vec::new()
        };
        let reported: Vec<&str> = claimed.iter().map(detection_status_of).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the DRIFTED-only rule must receive {expected:?}, got {claimed:?}"
        );
        for event in &claimed {
            assert_envelope(case, event, &stack_id, &detection_id);
            assert_verdict(case, event, row.verdict, row.drifted);
        }
    }
}

/// Two detections of the same stack are two operations: each publishes its own pair, joined to
/// its own call by the id that call answered, and the DRIFTED-only rule receives the
/// conclusion of the one that found drift and nothing of the one that did not — the operator's
/// rule going from silent to firing because the stack's queue was changed out of band.
#[tokio::test]
async fn each_detection_publishes_the_pair_joined_to_its_own_detection_id() {
    let case = "two detections of one stack, the second after an out-of-band change";
    let router = router();
    let stack = "drift-events-twice";
    let control = wired_queue(
        &router,
        case,
        &format!("{stack}-control"),
        &every_drift_event(),
    )
    .await;
    let claim = wired_queue(&router, case, &format!("{stack}-claim"), &only_drifted_events()).await;

    let template = json!({
        "Resources": {
            "Subject": {
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": "drift-events-twice-subject",
                    "VisibilityTimeout": 30,
                },
            },
        },
    })
    .to_string();
    let stack_id = deployed_stack(&router, case, stack, &template).await;

    // The first detection, of a stack nothing touched.
    let first = detect_drift(&router, case, &stack_id).await;
    let mut seen: Vec<Value> = Vec::new();
    collect_until(&router, case, &control, &mut seen, |seen| {
        one_with_status(seen, &first, DETECTION_IN_PROGRESS).is_some()
            && one_with_status(seen, &first, DETECTION_COMPLETE).is_some()
    })
    .await;
    let first_complete = one_with_status(&seen, &first, DETECTION_COMPLETE)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the first detection published no conclusion: {seen:?}")
        })
        .clone();
    assert_verdict(case, &first_complete, IN_SYNC, 0);
    let claimed = drain(&router, case, &claim).await;
    assert!(
        claimed.is_empty(),
        "case {case:?}: the DRIFTED-only rule must stay silent for an in-sync detection, got \
         {claimed:?}"
    );

    // The queue the stack deployed is changed out of band, and the stack is detected again.
    let queue = physical_id(&router, case, &stack_id, "Subject").await;
    Tamper::QueueVisibilityTimeout("Subject", "45")
        .apply(&router, case, &queue)
        .await;
    let second = detect_drift(&router, case, &stack_id).await;
    assert_ne!(
        second, first,
        "case {case:?}: each detection is its own operation, so it carries its own id"
    );
    let mut seen: Vec<Value> = Vec::new();
    collect_until(&router, case, &control, &mut seen, |seen| {
        one_with_status(seen, &second, DETECTION_IN_PROGRESS).is_some()
            && one_with_status(seen, &second, DETECTION_COMPLETE).is_some()
    })
    .await;
    let second_complete = one_with_status(&seen, &second, DETECTION_COMPLETE)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the second detection published no conclusion: {seen:?}")
        })
        .clone();
    assert_envelope(case, &second_complete, &stack_id, &second);
    assert_verdict(case, &second_complete, DRIFTED, 1);

    // The operator's rule now fires, and only on the drifted detection's conclusion.
    let mut claimed: Vec<Value> = Vec::new();
    collect_until(&router, case, &claim, &mut claimed, |claimed| {
        one_with_status(claimed, &second, DETECTION_COMPLETE).is_some()
    })
    .await;
    let reported: Vec<(&str, &str)> = claimed
        .iter()
        .map(|event| (detection_id_of(event), detection_status_of(event)))
        .collect();
    assert_eq!(
        reported,
        vec![(second.as_str(), DETECTION_COMPLETE)],
        "case {case:?}: the DRIFTED-only rule must receive the drifted detection's conclusion \
         alone, got {claimed:?}"
    );
}
