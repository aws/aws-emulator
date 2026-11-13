// SPDX-License-Identifier: CC0-1.0

//! An Amazon EMR cluster's state changes reach the EventBridge rules that match them.
//!
//! The RULE under test: **each state an Amazon EMR cluster ENTERS is announced on the
//! account's DEFAULT EventBridge bus as `source` `aws.emr` / `detail-type`
//! `EMR Cluster State Change`, whose `detail` names the cluster (`clusterId`, `name`), the
//! `state` it has just entered and that state's `stateChangeReason`; EventBridge then routes
//! the envelope by its CONTENT, so a rule whose `EventPattern` matches it delivers to that
//! rule's targets and a rule whose pattern does not match receives none of them.** The
//! states are the modelled machine's
//! (`smithy_query(query="shape", shape="com.amazonaws.emr#ClusterState")`), and a cluster
//! launched with `KeepJobFlowAliveWhenNoSteps` false and no steps walks that machine to the
//! terminal `TERMINATED` with no further call from anybody — so a client that waits for such
//! a cluster on a rule, and never calls Amazon EMR again, still learns that it finished.
//!
//! Only a cross-service suite can pin this: Amazon EMR publishes, EventBridge routes and
//! Amazon SQS holds what arrived, and the publication is made over the emulator's own
//! in-process callback — so WHICH mounted model claims that `events`-scoped call has to be
//! the one a client's own call reaches. It therefore runs against the WHOLE `build_router`
//! service set, as the binary serves it.
//!
//! The rows differ along what the rule generalises over: WHICH dimension of the envelope the
//! matching rule filters on (the source and detail type alone, the cluster id inside
//! `detail`, one state inside `detail`), and WHAT ends the cluster — the clock, for a
//! transient cluster that terminates itself, or a caller's `TerminateJobFlows` for one that
//! was kept alive. Every row also carries a rule naming ANOTHER cluster id, whose own queue
//! must stay empty: the negative half of the same rule.
//!
//! The service crate's own `cluster_self_termination.rs` pins the WALK a cluster makes;
//! what only this suite can reach is the delivery of each state it entered.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` Amazon EMR announces a cluster's state changes under,
/// spelled out here rather than imported from the service crate, so a wrong constant in the
/// implementation cannot make this suite pass. Source: the EventBridge reference page
/// "Amazon EMR events".
const EMR_SOURCE: &str = "aws.emr";
const CLUSTER_STATE_CHANGE: &str = "EMR Cluster State Change";

/// The bus an Amazon Web Services service's own events go to.
const DEFAULT_BUS: &str = "default";

/// The service principal EventBridge delivers to an SQS target as, which is what the queue's
/// resource policy admits.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// How long a row waits for the states the clock owes to arrive on a queue: far above every
/// span this service owes a cluster, and driven off what has ARRIVED rather than off a fixed
/// sleep, so a slow host delays a row instead of breaking it.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(90);

/// One poll of a queue inside [`DELIVERY_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row leaves the non-matching rule's queue alone before it reads it: longer than
/// the span the matching rule's queue took to receive this cluster's whole walk, so an empty
/// read is evidence the rule was no delivery target rather than evidence of looking early.
const QUIET_SPAN: Duration = Duration::from_millis(750);

/// The service role every launch names — "the IAM role that Amazon EMR assumes in order to
/// access Amazon Web Services resources on your behalf" (`RunJobFlow$ServiceRole`).
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// The EC2 instance profile every launch names as its `JobFlowRole`.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";

/// The whole emulator over one engine, exactly as `core/router/src/lib.rs` assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: the router above runs with authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The status and parsed body of one response.
fn answered(response: &http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `X-Amz-Target` RPC call against the service whose signing name is `signing_name`,
/// which must answer 200.
async fn rpc(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let (status, parsed) = answered(&router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Amazon EMR call (`awsJson1_1`, `ElasticMapReduce.<Operation>`).
async fn emr(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(
        router,
        case,
        "elasticmapreduce",
        "ElasticMapReduce",
        "1.1",
        operation,
        body,
    )
    .await
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`).
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, "events", "AWSEvents", "1.1", operation, body).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// One IAM `awsQuery` call, which must answer 200.
async fn iam(router: &MultiServiceRouter, case: &str, body: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("authorization", authorization("iam"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from IAM, got {status} with {text}"
    );
    text
}

/// The service role and the EC2 instance profile every launch below names, built the way AWS
/// builds one — `CreateRole`, `CreateInstanceProfile`, `AddRoleToInstanceProfile` — because
/// `RunJobFlow` resolves its `JobFlowRole` against the account's IAM before it launches
/// anything (the rule `emr_job_flow_role_instance_profile.rs` pins).
async fn launch_roles(router: &MultiServiceRouter, case: &str) {
    let trust = |principal: &str| {
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
        );
        document.replace('"', "%22").replace(' ', "%20")
    };
    iam(
        router,
        case,
        &format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={SERVICE_ROLE}\
             &AssumeRolePolicyDocument={}",
            trust("elasticmapreduce.amazonaws.com")
        ),
    )
    .await;
    iam(
        router,
        case,
        &format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={INSTANCE_PROFILE}\
             &AssumeRolePolicyDocument={}",
            trust("ec2.amazonaws.com")
        ),
    )
    .await;
    iam(
        router,
        case,
        &format!(
            "Action=CreateInstanceProfile&Version=2010-05-08\
             &InstanceProfileName={INSTANCE_PROFILE}&Path=/"
        ),
    )
    .await;
    iam(
        router,
        case,
        &format!(
            "Action=AddRoleToInstanceProfile&Version=2010-05-08\
             &InstanceProfileName={INSTANCE_PROFILE}&RoleName={INSTANCE_PROFILE}"
        ),
    )
    .await;
}

/// A string member of a response, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// An SQS queue called `name`; answers its url and the ARN a rule names it by.
async fn queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({"QueueName": name})).await;
    let url = member(case, &created, "/QueueUrl");
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = member(case, &attributes, "/Attributes/QueueArn");
    (url, arn)
}

/// Let EventBridge send to the queue at `url` on behalf of the rule `rule_arn`, the way a
/// client admits a rule to its queue.
async fn admit_events(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    queue_arn: &str,
    rule_arn: &str,
) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": {"Service": EVENTS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": {"ArnEquals": {"aws:SourceArn": rule_arn}},
        }],
    })
    .to_string();
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
    )
    .await;
}

/// An ENABLED rule called `name` on the default bus carrying `pattern`, with the queue
/// `queue_arn` as its one target; answers the rule's ARN.
async fn rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    pattern: &Value,
    queue_arn: &str,
) -> String {
    let put = events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": DEFAULT_BUS,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    let rule_arn = member(case, &put, "/RuleArn");
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": DEFAULT_BUS,
            "Targets": [{"Id": "t1", "Arn": queue_arn}],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {case:?}: every target must be accepted, got {targeted}"
    );
    rule_arn
}

/// Every message now waiting on the queue at `url`, as the parsed envelopes their bodies
/// carry.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)
        })
        .collect()
}

/// Collect envelopes off the queue at `url` until one carries `final_state` in its
/// `detail.state`, or [`DELIVERY_DEADLINE`] passes; answers everything that arrived.
///
/// Waits for the CONDITION — the state the cluster ends in having been delivered — rather
/// than for a span.
async fn collect_until(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    final_state: &str,
) -> Vec<Value> {
    let started = std::time::Instant::now();
    let mut arrived: Vec<Value> = Vec::new();
    while started.elapsed() < DELIVERY_DEADLINE {
        arrived.extend(drain(router, case, url).await);
        if arrived
            .iter()
            .any(|envelope| envelope["detail"]["state"].as_str() == Some(final_state))
        {
            return arrived;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    arrived
}

/// The `detail.state` of each envelope, in the order they arrived.
fn states(envelopes: &[Value]) -> Vec<String> {
    envelopes
        .iter()
        .map(|envelope| {
            envelope["detail"]["state"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The states delivered once the row's rules were in place: the launch's own `STARTING` is
/// dropped when it arrived, for the reason [`SELF_TERMINATION`] gives.
fn delivered_after_the_rule(envelopes: &[Value]) -> Vec<String> {
    let mut found = states(envelopes);
    if found.first().map(String::as_str) == Some("STARTING") {
        found.remove(0);
    }
    found
}

/// What one row filters on, how its cluster ends, and what it must therefore see.
struct Row {
    case: &'static str,
    /// Whether the launch asks the cluster to stay up, which is what decides whether the
    /// clock or the caller ends it.
    kept_alive: bool,
    /// The `detail` clause the matching rule's pattern carries beyond the source and the
    /// detail type, built over the launched cluster's own id. `None` filters on the source
    /// and detail type alone.
    detail: Option<fn(&str) -> Value>,
    /// The `detail.state` the row waits for, which is the state its cluster ends in.
    final_state: &'static str,
    /// The states the matching rule's queue must receive, in walk order. A rule filtering on
    /// one state receives that state alone.
    expected: &'static [&'static str],
}

/// The walk a transient cluster makes with nothing but the clock called: it comes up, is
/// ready to run steps, finds its (empty) step list complete and shuts itself down.
///
/// `STARTING` is not among them: the launch itself enters that state and announces it, which
/// is before any rule naming the launched cluster's own id can exist, so whether it reaches
/// a rule created after the launch is a race rather than part of the rule under test
/// ([`delivered_after_the_rule`]).
const SELF_TERMINATION: &[&str] = &["BOOTSTRAPPING", "RUNNING", "TERMINATING", "TERMINATED"];

/// The walk a cluster kept alive makes when a caller ends it once it is up.
const CALLER_TERMINATION: &[&str] = &["BOOTSTRAPPING", "WAITING", "TERMINATING", "TERMINATED"];

/// A rule filtering on ONE state receives that state alone.
const TERMINATED_ONLY: &[&str] = &["TERMINATED"];

#[tokio::test]
async fn every_cluster_state_change_reaches_the_rules_that_match_it_and_no_others() {
    let rows = [
        Row {
            case: "a transient cluster, the source and detail type alone",
            kept_alive: false,
            detail: None,
            final_state: "TERMINATED",
            expected: SELF_TERMINATION,
        },
        Row {
            case: "a transient cluster, its id inside the detail",
            kept_alive: false,
            detail: Some(|cluster_id| json!({"clusterId": [cluster_id]})),
            final_state: "TERMINATED",
            expected: SELF_TERMINATION,
        },
        Row {
            case: "a transient cluster, the terminal state inside the detail",
            kept_alive: false,
            detail: Some(|cluster_id| {
                json!({"clusterId": [cluster_id], "state": ["TERMINATED"]})
            }),
            final_state: "TERMINATED",
            expected: TERMINATED_ONLY,
        },
        Row {
            case: "a cluster kept alive, ended by the caller",
            kept_alive: true,
            detail: Some(|cluster_id| json!({"clusterId": [cluster_id]})),
            final_state: "TERMINATED",
            expected: CALLER_TERMINATION,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router();

        // The two queues: the one the matching rule targets, and the one only a rule that
        // cannot match targets.
        let (matching_url, matching_arn) = queue(&router, case, &format!("q-match-{index}")).await;
        let (other_url, other_arn) = queue(&router, case, &format!("q-other-{index}")).await;

        // The roles the launch resolves before it launches anything.
        launch_roles(&router, case).await;

        // The cluster. Its id is only known once the launch has answered, which is also why
        // the rules below are put AFTER it: the envelopes a rule delivers are the ones
        // published while it was in place, and this cluster's walk takes several spans.
        let launched = emr(
            &router,
            case,
            "RunJobFlow",
            &json!({
                "Name": format!("events-{index}"),
                "ReleaseLabel": "emr-6.15.0",
                "ServiceRole": SERVICE_ROLE,
                "JobFlowRole": INSTANCE_PROFILE,
                "Applications": [{"Name": "Hadoop"}],
                "Instances": {
                    "InstanceCount": 1,
                    "MasterInstanceType": "m5.xlarge",
                    "SlaveInstanceType": "m5.xlarge",
                    "KeepJobFlowAliveWhenNoSteps": row.kept_alive,
                    "TerminationProtected": false,
                },
            }),
        )
        .await;
        let cluster_id = member(case, &launched, "/JobFlowId");

        let mut matching_pattern = json!({
            "source": [EMR_SOURCE],
            "detail-type": [CLUSTER_STATE_CHANGE],
        });
        if let Some(clause) = row.detail {
            matching_pattern["detail"] = clause(&cluster_id);
        }
        // A rule naming a cluster id no event of this cluster can carry.
        let other_pattern = json!({
            "source": [EMR_SOURCE],
            "detail-type": [CLUSTER_STATE_CHANGE],
            "detail": {"clusterId": ["j-NOSUCHCLUSTER"]},
        });
        let matching_rule = rule(
            &router,
            case,
            &format!("r-match-{index}"),
            &matching_pattern,
            &matching_arn,
        )
        .await;
        let other_rule = rule(
            &router,
            case,
            &format!("r-other-{index}"),
            &other_pattern,
            &other_arn,
        )
        .await;
        admit_events(&router, case, &matching_url, &matching_arn, &matching_rule).await;
        admit_events(&router, case, &other_url, &other_arn, &other_rule).await;

        if row.kept_alive {
            // A cluster asked to stay up is ended by its caller: it is ended once it has
            // come to rest, so the state it rested in is published too.
            let waiting = collect_until(&router, case, &matching_url, "WAITING").await;
            assert!(
                states(&waiting).contains(&"WAITING".to_string()),
                "case {case:?}: a cluster kept alive must announce WAITING, got {:?}",
                states(&waiting)
            );
            emr(
                &router,
                case,
                "TerminateJobFlows",
                &json!({"JobFlowIds": [&cluster_id]}),
            )
            .await;
            let rest = collect_until(&router, case, &matching_url, row.final_state).await;
            let mut arrived = waiting;
            arrived.extend(rest);
            assert_delivered(case, &cluster_id, &arrived, row.expected);
        } else {
            // Nothing else is called: the cluster's whole walk is the clock's.
            let arrived = collect_until(&router, case, &matching_url, row.final_state).await;
            assert_delivered(case, &cluster_id, &arrived, row.expected);
        }

        // The rule that cannot match delivered nothing — read after the matching rule's
        // queue already held this cluster's whole walk.
        tokio::time::sleep(QUIET_SPAN).await;
        let other = drain(&router, case, &other_url).await;
        assert!(
            other.is_empty(),
            "case {case:?}: a rule whose pattern names another cluster must receive nothing, \
             got {:?}",
            states(&other)
        );
    }
}

/// Assert the envelopes `arrived` are exactly this row's `expected` states, each carrying the
/// envelope Amazon EMR publishes: the source, the detail type and a `detail` naming the
/// cluster.
fn assert_delivered(case: &str, cluster_id: &str, arrived: &[Value], expected: &[&str]) {
    let delivered = delivered_after_the_rule(arrived);
    assert_eq!(
        delivered,
        expected.iter().map(|state| (*state).to_string()).collect::<Vec<String>>(),
        "case {case:?}: expected the states {expected:?} to be delivered in order, got \
         {delivered:?} from {arrived:?}"
    );
    for envelope in arrived {
        assert_eq!(
            envelope["source"].as_str(),
            Some(EMR_SOURCE),
            "case {case:?}: expected the source {EMR_SOURCE}, got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"].as_str(),
            Some(CLUSTER_STATE_CHANGE),
            "case {case:?}: expected the detail-type {CLUSTER_STATE_CHANGE}, got {envelope}"
        );
        assert_eq!(
            envelope["detail"]["clusterId"].as_str(),
            Some(cluster_id),
            "case {case:?}: the detail must name the cluster that moved, got {envelope}"
        );
        assert!(
            envelope["detail"]["name"].as_str().is_some(),
            "case {case:?}: the detail must name the cluster, got {envelope}"
        );
        assert_eq!(
            envelope["account"].as_str(),
            Some(engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: the envelope must name the caller's account, got {envelope}"
        );
        assert_eq!(
            envelope["region"].as_str(),
            Some(REGION),
            "case {case:?}: the envelope must name the calling Region, got {envelope}"
        );
    }
    // The terminal state carries the reason the cluster ended, as the JSON document the
    // member holds a rendering of.
    if let Some(terminal) = arrived
        .iter()
        .find(|envelope| envelope["detail"]["state"].as_str() == Some("TERMINATED"))
    {
        let reason = terminal["detail"]["stateChangeReason"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a terminated cluster announces its reason, got {terminal}")
            });
        let parsed: Value = serde_json::from_str(reason).unwrap_or(Value::Null);
        assert!(
            parsed["code"].as_str().is_some(),
            "case {case:?}: the reason must name its code, got {reason}"
        );
    }
}
