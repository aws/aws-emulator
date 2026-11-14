// SPDX-License-Identifier: CC0-1.0

//! A Config rule's compliance verdict CHANGING, across three services: the verdict is
//! delivered to Config, Config publishes the change onto the account's default
//! EventBridge bus, and the bus delivers it to the SQS queue a rule targets.
//!
//! This suite needs more than one service by construction: the event is published by
//! the Config crate, routed by the EventBridge crate and read out of the SQS crate, so
//! a single-service test cannot see the hand-off at all.
//!
//! The rule under test: when a Config rule's evaluation verdict for a resource
//! changes, Config publishes ONE event with source `aws.config` and detail-type
//! `Config Rules Compliance Change` on the default bus, whose `detail` carries
//! `messageType` `ComplianceChangeNotification`, the verdict that now holds at
//! `detail.newEvaluationResult.complianceType` and the rule's name at
//! `detail.newEvaluationResult.evaluationResultIdentifier.evaluationResultQualifier.configRuleName`,
//! and which carries `oldEvaluationResult` exactly when the change REPLACED a previous
//! verdict. A verdict delivered again unchanged is not a change and publishes nothing.
//! An EventBridge rule matching on source and detail-type alone, targeting an SQS
//! queue, therefore receives one message per verdict change.
//!
//! The rows differ along the sequence of verdicts delivered — how many, which values,
//! and whether a delivery repeats the verdict already recorded — so the number of
//! events and the old/new pair each one carries are both derived from the sequence
//! rather than from one recorded case.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for; the ARNs and the event envelope must
/// all name it.
const REGION: &str = "eu-west-1";

/// The account every emulated resource belongs to.
const ACCOUNT_ID: &str = "000000000000";

/// The `source` Config publishes its events under, and the `detail-type` of the
/// compliance-change event (the EventBridge reference for Config).
const CONFIG_EVENT_SOURCE: &str = "aws.config";
/// See [`CONFIG_EVENT_SOURCE`].
const COMPLIANCE_CHANGE_DETAIL_TYPE: &str = "Config Rules Compliance Change";
/// The `detail.messageType` a compliance-change notification carries.
const COMPLIANCE_CHANGE_MESSAGE_TYPE: &str = "ComplianceChangeNotification";

/// A managed rule Config DOES offer to `PutConfigRule`, from *List of Config Managed
/// Rules*. The process check rule a delivered evaluation needs is seeded from it through
/// the Config crate's own seam (`state::point_at_process_check`), because Config has
/// withdrawn the process check managed rule from `PutConfigRule`.
const OFFERED_MANAGED_RULE: &str = "S3_BUCKET_VERSIONING_ENABLED";

/// The resource type an external evaluation is recorded against
/// (`ExternalEvaluation$ComplianceResourceType`: "Config accepts AWS::::Account
/// resource type").
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// The `ComplianceType`s a deliverer may send
/// (`ExternalEvaluation$ComplianceType`: "The valid values are COMPLIANT,
/// NON_COMPLIANT, and NOT_APPLICABLE").
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The rule the Config rule, and the EventBridge rule, are named after in each row.
const CONFIG_RULE_NAME: &str = "account-process-check";

/// The three services this hand-off needs, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// What EventBridge's routing log recorded: one line per routed event, naming the
/// rules it matched and the target ARNs those rules carry.
///
/// The log is EventBridge's own observable record of what it ROUTED, so reading it
/// separates "the event never reached the bus" from "it reached the bus and matched
/// nothing" from "it matched and the target was not invoked" — three failures a queue
/// that stays empty cannot tell apart.
fn routed(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, Vec<String>, Vec<String>)> {
    let state = engine.state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    let store = state.lock();
    store
        .delivered
        .iter()
        .map(|event| {
            (
                event.detail_type.clone().unwrap_or_default(),
                event.matched_rules.clone(),
                event.target_arns.clone(),
            )
        })
        .collect()
}

/// One RPC call: `POST /` carrying `X-Amz-Target: <target_prefix>.<operation>`, signed
/// for `signing_name` in [`REGION`], as its status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization)
        .header("content-type", format!("application/x-amz-json-{json_version}"))
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

/// Call one of the three services and require a 200, returning the body.
async fn ok(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = call(
        router,
        signing_name,
        target_prefix,
        json_version,
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A Config call (`awsJson1_1`, `StarlingDoveService.<Operation>`).
async fn config(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok(router, "config", "StarlingDoveService", "1.1", operation, body).await
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`).
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok(router, "events", "AWSEvents", "1.1", operation, body).await
}

/// An SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok(router, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// Create a queue and an EventBridge rule on the default bus that targets it, matching
/// on `source` and `detail-type` alone, and hand back the queue's url.
async fn queue_targeted_by_a_compliance_change_rule(
    router: &MultiServiceRouter,
    queue_name: &str,
) -> String {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": queue_name })).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a QueueArn, got {attributes}"))
        .to_string();
    assert_eq!(
        queue_arn,
        format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{queue_name}"),
        "the queue ARN must name the signed Region and the account"
    );
    // The grant a rule's queue target needs: EventBridge makes the target's
    // `sqs:SendMessage` as its own service principal and "relies on resource-based
    // policies" for a queue (`PutTargets`), so a queue that has granted it nothing
    // receives nothing. The helper asserts the write's own 200.
    sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": {
                "Policy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "events.amazonaws.com"},
                        "Action": "sqs:SendMessage",
                        "Resource": queue_arn,
                    }],
                })
                .to_string(),
            },
        }),
    )
    .await;

    events(
        router,
        "PutRule",
        &json!({
            "Name": "config-compliance-change",
            "EventPattern": json!({
                "source": [CONFIG_EVENT_SOURCE],
                "detail-type": [COMPLIANCE_CHANGE_DETAIL_TYPE],
            })
            .to_string(),
        }),
    )
    .await;
    events(
        router,
        "PutTargets",
        &json!({
            "Rule": "config-compliance-change",
            "Targets": [{ "Id": "the-queue", "Arn": queue_arn }],
        }),
    )
    .await;
    queue_url
}

/// Put the recorder, the delivery channel and the process check rule a delivered
/// evaluation needs.
///
/// The rule is the one an account holds from before Config WITHDREW the
/// `AWS_CONFIG_PROCESS_CHECK` managed rule from `PutConfigRule` (recorded behaviour,
/// pinned by the Config crate's `withdrawn_managed_rule` suite): it is created as an
/// instance of a managed rule Config does offer and then pointed at the process check
/// identifier in the store the handlers share, which is the population
/// `PutExternalEvaluation` still applies to.
async fn config_rule_ready_for_external_evaluations(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) {
    config(
        router,
        "PutConfigurationRecorder",
        &json!({
            "ConfigurationRecorder": {
                "roleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/config-role"),
            }
        }),
    )
    .await;
    config(
        router,
        "PutDeliveryChannel",
        &json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
    )
    .await;
    config(
        router,
        "PutConfigRule",
        &json!({
            "ConfigRule": {
                "ConfigRuleName": CONFIG_RULE_NAME,
                "Source": { "Owner": "AWS", "SourceIdentifier": OFFERED_MANAGED_RULE },
                "Scope": { "ComplianceResourceTypes": [ACCOUNT_RESOURCE_TYPE] },
            }
        }),
    )
    .await;
    let config_state = service_com_amazonaws_configservice_starlingdoveservice::state::service_state(
        &**engine,
    )
    .expect("the test engine is the shared engine");
    let mut store = config_state.lock();
    let rule = store
        .config_rules
        .get_mut(CONFIG_RULE_NAME)
        .expect("the rule was created through PutConfigRule");
    service_com_amazonaws_configservice_starlingdoveservice::state::point_at_process_check(
        &mut rule.source,
    );
}

/// Deliver one verdict for the account resource of the process check rule.
async fn deliver(router: &MultiServiceRouter, compliance_type: &str, ordering_timestamp: f64) {
    config(
        router,
        "PutExternalEvaluation",
        &json!({
            "ConfigRuleName": CONFIG_RULE_NAME,
            "ExternalEvaluation": {
                "ComplianceResourceType": ACCOUNT_RESOURCE_TYPE,
                "ComplianceResourceId": ACCOUNT_ID,
                "ComplianceType": compliance_type,
                "OrderingTimestamp": ordering_timestamp,
            },
        }),
    )
    .await;
}

/// Every message on the queue, as the parsed event each one carries, deleting each so
/// a later drain reports only what arrived after it.
async fn drain_events(router: &MultiServiceRouter, queue_url: &str) -> Vec<Value> {
    let mut events_read: Vec<Value> = Vec::new();
    // The delivery is made by the `PutEvents` call the verdict change drove, so
    // everything owed is already on the queue; a handful of receives is enough to
    // empty it, and the loop stops as soon as one comes back empty.
    for _ in 0..10 {
        let received = sqs(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if messages.is_empty() {
            break;
        }
        for message in messages {
            let body = message["Body"].as_str().unwrap_or_default();
            events_read.push(serde_json::from_str(body).unwrap_or(Value::Null));
            if let Some(handle) = message["ReceiptHandle"].as_str() {
                sqs(
                    router,
                    "DeleteMessage",
                    &json!({ "QueueUrl": queue_url, "ReceiptHandle": handle }),
                )
                .await;
            }
        }
    }
    events_read
}

/// The verdict an event reports as holding now, and the one it reports it replaced.
fn verdicts_of(event: &Value) -> (Option<&str>, Option<&str>) {
    (
        event["detail"]["newEvaluationResult"]["complianceType"].as_str(),
        event["detail"]["oldEvaluationResult"]["complianceType"].as_str(),
    )
}

/// One row of the verdict table: the verdicts delivered in order, and the (new, old)
/// pair of each event the queue must then receive.
struct Case {
    case: &'static str,
    verdicts: &'static [&'static str],
    expected: &'static [(&'static str, Option<&'static str>)],
}

#[tokio::test]
async fn a_verdict_change_reaches_the_sqs_target_of_an_eventbridge_rule() {
    let cases: [Case; 5] = [
        Case {
            case: "one verdict, the first for the resource",
            verdicts: &[COMPLIANT],
            expected: &[(COMPLIANT, None)],
        },
        Case {
            case: "a flip from compliant to noncompliant",
            verdicts: &[COMPLIANT, NON_COMPLIANT],
            expected: &[(COMPLIANT, None), (NON_COMPLIANT, Some(COMPLIANT))],
        },
        Case {
            case: "a flip the other way",
            verdicts: &[NON_COMPLIANT, COMPLIANT],
            expected: &[(NON_COMPLIANT, None), (COMPLIANT, Some(NON_COMPLIANT))],
        },
        Case {
            case: "a repeated verdict is not a change",
            verdicts: &[NON_COMPLIANT, NON_COMPLIANT, COMPLIANT],
            expected: &[(NON_COMPLIANT, None), (COMPLIANT, Some(NON_COMPLIANT))],
        },
        Case {
            case: "three changes, ending not applicable",
            verdicts: &[COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE],
            expected: &[
                (COMPLIANT, None),
                (NON_COMPLIANT, Some(COMPLIANT)),
                (NOT_APPLICABLE, Some(NON_COMPLIANT)),
            ],
        },
    ];

    for Case {
        case,
        verdicts,
        expected,
    } in cases
    {
        let (router, engine) = router();
        let queue_url = queue_targeted_by_a_compliance_change_rule(&router, "compliance-events")
            .await;
        config_rule_ready_for_external_evaluations(&router, &engine).await;

        // Nothing has changed a verdict yet, so nothing has been published.
        let before = drain_events(&router, &queue_url).await;
        assert!(
            before.is_empty(),
            "case {case:?}: no event is owed before a verdict changes, got {before:?}"
        );

        for (index, verdict) in verdicts.iter().enumerate() {
            // Each delivery stamps its own OrderingTimestamp, so the notification's
            // timestamps come from the delivery that produced it.
            deliver(&router, verdict, 1_700_000_000.0 + index as f64).await;
        }

        // What EventBridge routed, which says WHERE a missing message was lost: an
        // event that never reached the bus, one that matched no rule, or one that
        // matched and reached a target.
        let routed_events = routed(&engine);
        assert_eq!(
            routed_events.len(),
            expected.len(),
            "case {case:?}: expected one event on the bus per verdict CHANGE out of \
             {verdicts:?}, got {routed_events:?}"
        );
        for (position, (detail_type, matched_rules, target_arns)) in
            routed_events.iter().enumerate()
        {
            assert_eq!(
                (detail_type.as_str(), matched_rules.len(), target_arns.len()),
                (COMPLIANCE_CHANGE_DETAIL_TYPE, 1, 1),
                "case {case:?}: event {position} must have matched the one rule and its \
                 one queue target, got {routed_events:?}"
            );
        }

        let received = drain_events(&router, &queue_url).await;
        assert_eq!(
            received.len(),
            expected.len(),
            "case {case:?}: expected one message per verdict CHANGE out of {verdicts:?}, \
             got {} message(s): {received:?}",
            received.len()
        );

        for (position, (new_verdict, old_verdict)) in expected.iter().enumerate() {
            let event = &received[position];
            assert_eq!(
                (
                    event["source"].as_str(),
                    event["detail-type"].as_str(),
                    event["region"].as_str(),
                    event["account"].as_str(),
                ),
                (
                    Some(CONFIG_EVENT_SOURCE),
                    Some(COMPLIANCE_CHANGE_DETAIL_TYPE),
                    Some(REGION),
                    Some(ACCOUNT_ID),
                ),
                "case {case:?}: event {position} must be a Config compliance change in \
                 this Region, got {event}"
            );
            assert!(
                event["id"].as_str().is_some_and(|id| !id.is_empty())
                    && event["time"].as_str().is_some_and(|at| at.contains('T')),
                "case {case:?}: event {position} must carry an event id and time, got {event}"
            );
            assert_eq!(
                event["detail"]["messageType"].as_str(),
                Some(COMPLIANCE_CHANGE_MESSAGE_TYPE),
                "case {case:?}: event {position} must be a compliance-change \
                 notification, got {event}"
            );
            assert_eq!(
                event["detail"]["newEvaluationResult"]["evaluationResultIdentifier"]
                    ["evaluationResultQualifier"]["configRuleName"]
                    .as_str(),
                Some(CONFIG_RULE_NAME),
                "case {case:?}: event {position} must name the rule whose verdict \
                 changed, got {event}"
            );
            let (reported_new, reported_old) = verdicts_of(event);
            assert_eq!(
                (reported_new, reported_old),
                (Some(*new_verdict), *old_verdict),
                "case {case:?}: event {position} must report the new verdict and the \
                 one it replaced, got {event}"
            );
            assert_eq!(
                (
                    event["detail"]["resourceType"].as_str(),
                    event["detail"]["resourceId"].as_str(),
                ),
                (Some(ACCOUNT_RESOURCE_TYPE), Some(ACCOUNT_ID)),
                "case {case:?}: event {position} must name the evaluated resource, \
                 got {event}"
            );
        }

        // Draining is a read: once the queue is empty, no further event appears.
        let after = drain_events(&router, &queue_url).await;
        assert!(
            after.is_empty(),
            "case {case:?}: every published event was delivered once, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_eventbridge_rule_that_does_not_match_receives_no_compliance_change() {
    // The delivery goes to the targets of the rules an event MATCHED, so a rule
    // filtering on anything a Config compliance change does not carry receives
    // nothing. The rows differ along which member of the envelope the pattern filters
    // on and what it asks for.
    let patterns: [(&str, Value); 3] = [
        (
            "another service's source",
            json!({ "source": ["aws.codepipeline"] }),
        ),
        (
            "another detail-type of this source",
            json!({
                "source": [CONFIG_EVENT_SOURCE],
                "detail-type": ["Config Configuration Item Change"],
            }),
        ),
        (
            "a detail member the notification does not carry that value for",
            json!({
                "source": [CONFIG_EVENT_SOURCE],
                "detail": { "messageType": ["ConfigurationItemChangeNotification"] },
            }),
        ),
    ];

    for (case, pattern) in patterns {
        let (router, engine) = router();
        let created = sqs(&router, "CreateQueue", &json!({ "QueueName": "other-events" })).await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();
        let queue_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:other-events");
        // The queue admits EventBridge, so what this row shows is the PATTERN
        // deciding the delivery and nothing else.
        sqs(
            &router,
            "SetQueueAttributes",
            &json!({
                "QueueUrl": &queue_url,
                "Attributes": {
                    "Policy": json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "events.amazonaws.com"},
                            "Action": "sqs:SendMessage",
                            "Resource": &queue_arn,
                        }],
                    })
                    .to_string(),
                },
            }),
        )
        .await;
        events(
            &router,
            "PutRule",
            &json!({ "Name": "unmatching", "EventPattern": pattern.to_string() }),
        )
        .await;
        events(
            &router,
            "PutTargets",
            &json!({
                "Rule": "unmatching",
                "Targets": [{ "Id": "the-queue", "Arn": queue_arn }],
            }),
        )
        .await;

        config_rule_ready_for_external_evaluations(&router, &engine).await;
        deliver(&router, NON_COMPLIANT, 1_700_000_000.0).await;

        let received = drain_events(&router, &queue_url).await;
        assert!(
            received.is_empty(),
            "case {case:?}: a rule this event does not match delivers nothing, got \
             {received:?}"
        );
    }
}

#[tokio::test]
async fn a_client_may_not_publish_an_event_claiming_an_aws_service_as_its_source() {
    // The reserved `aws.` namespace belongs to the Amazon Web Services services
    // themselves, which in this emulator publish through the in-process callback. A
    // request a CLIENT makes carries no such provenance, so it is refused however it
    // spells the namespace — including the exact source Config's own notification
    // uses, which is what makes the Config leg above a service publication and not a
    // hole a caller can reach through.
    //
    // The rows differ along which service the source claims, including the one this
    // suite's real events carry.
    let sources: [(&str, &str); 3] = [
        ("the source Config publishes under", CONFIG_EVENT_SOURCE),
        ("another service's source", "aws.codepipeline"),
        ("a partner source", "aws.partner/example.com/1/stream"),
    ];

    for (case, source) in sources {
        let (router, engine) = router();
        let (status, answered) = call(
            &router,
            "events",
            "AWSEvents",
            "1.1",
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": source,
                    "DetailType": COMPLIANCE_CHANGE_DETAIL_TYPE,
                    "Detail": json!({ "messageType": COMPLIANCE_CHANGE_MESSAGE_TYPE })
                        .to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused ENTRY is reported in a 200, got {status} \
             with {answered}"
        );
        assert_eq!(
            (
                answered["FailedEntryCount"].as_i64(),
                answered["Entries"][0]["ErrorCode"].as_str(),
            ),
            (Some(1), Some("NotAuthorizedForSourceException")),
            "case {case:?}: a client's entry claiming the reserved namespace must fail \
             on its own row, got {answered}"
        );
        assert!(
            routed(&engine).is_empty(),
            "case {case:?}: a refused entry reaches no bus, got {:?}",
            routed(&engine)
        );
    }
}
