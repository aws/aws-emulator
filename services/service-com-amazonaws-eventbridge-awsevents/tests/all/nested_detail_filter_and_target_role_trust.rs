// SPDX-License-Identifier: CC0-1.0

//! Two rules of the target-invocation path, pinned end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter`, with Amazon SQS, Step Functions, IAM and CloudWatch mounted on the
//! same engine — so each delivery is asserted where a CLIENT observes one: on the target
//! queue, on the state machine's own executions, and on the rule's invocation counters.
//!
//! **Rule one — a publication is matched ENTRY BY ENTRY, and a pattern may name a member
//! nested inside `detail`.** `PutEvents` takes a list of entries
//! (`PutEventsRequestEntryList`, `@length` min 1 max 10) and "returns an `EventId` per
//! accepted entry"; a rule's `EventPattern` is matched against the EVENT, so each entry of
//! one request is routed on its own, and an `EventPattern` whose `detail` states a nested
//! object matches an entry only when the value at that path is one the pattern lists (an
//! event member holding an array matches when the array CONTAINS such a value). Every entry
//! a rule matched is delivered once to each of that rule's targets carrying the whole
//! envelope, and an entry no rule matched is delivered nowhere — while the request as a
//! whole is still accepted with `FailedEntryCount` 0, because an entry matching no rule is
//! not a failed entry.
//!
//! **Rule two — a target naming a `RoleArn` is reached by ASSUMING that role, so that role's
//! trust policy decides the delivery.** `Target$RoleArn` is "the Amazon Resource Name (ARN)
//! of the IAM role to be used for this target when the rule is triggered", and the assume
//! carries the invoking rule as `aws:SourceArn`: a role whose `AssumeRolePolicyDocument` does
//! not let `events.amazonaws.com` assume it FOR THAT RULE cannot be assumed, so no
//! `states:StartExecution` is made, the state machine holds no execution for that event and
//! the delivery is counted on the rule's `FailedInvocations`; a role whose trust admits the
//! service for that rule, and whose identity policies allow `states:StartExecution` on the
//! machine, starts exactly one execution per matched event whose `input` is the event
//! envelope and which the machine's own definition takes to `SUCCEEDED`, counted on
//! `Invocations`. Re-pointing the same target `Id` at another role decides the NEXT event by
//! the document in force then.
//!
//! The rows differ along what each rule generalises over: for the first, which nested path
//! the pattern filters on, how deep it sits, how many members it names, how many entries the
//! one request carries, which of them match and where in the request they sit; for the
//! second, which principal the target role's trust names and which `Condition` scopes it.
//!
//! The harness (the router with EventBridge, Amazon SQS, Step Functions, IAM and CloudWatch
//! on one engine) is `tests/all/target_dispatch.rs`'s.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, FAILED_INVOCATIONS_METRIC, INVOCATIONS_METRIC,
    SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, events, iam, metric_sum, sfn, sqs, xml_text, Emulator, POLL_INTERVAL, REGION,
};

/// The `Source` every entry below carries, and the one each rule matches on.
const SOURCE: &str = "sim2real.distribution.events";

/// The `detail-type` values the rows publish under.
const STATE_CHANGE: &str = "Distribution State Change";
const SIMULATED_CALL: &str = "Distribution Event (Simulated)";

/// A definition whose only state terminates it, written as a client sends one: the
/// execution a delivery starts has to reach a terminal status on this alone.
const PASS_DEFINITION: &str = r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#;

/// The status a Step Functions execution of [`PASS_DEFINITION`] ends in. Spelled out here
/// rather than imported, so a wrong constant in the implementation cannot make this pass.
const SUCCEEDED: &str = "SUCCEEDED";

/// How long an observation of work the clock owes — a delivery, the execution it starts, that
/// execution's own terminal status — is looked for before it is a failed assertion. Generous:
/// what is asserted is that it happens at all, never how fast the host runs.
const DELIVERY_BUDGET: Duration = Duration::from_secs(30);

/// How long a row that expects NO execution keeps looking for one before concluding the
/// delivery was never made. A bound on an absence, which is the one span a slow host may
/// spend here without changing the verdict — the positive rows prove the same wiring starts an
/// execution in a fraction of it.
const ABSENCE_WINDOW: Duration = Duration::from_secs(5);

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// One entry of the single publication a row makes: the `detail` document it carries and
/// whether the row's pattern matches it.
struct Entry {
    detail: Value,
    matched: bool,
}

/// One row of the matching table.
struct FilterRow {
    case: &'static str,
    /// The `detail-type` the row's rule matches and its entries carry.
    detail_type: &'static str,
    /// The `detail` object of the row's `EventPattern`.
    detail_pattern: Value,
    /// The entries of the ONE `PutEvents` request, in request order.
    entries: Vec<Entry>,
}

/// An entry the row's pattern matches.
fn matching(detail: Value) -> Entry {
    Entry {
        detail,
        matched: true,
    }
}

/// An entry the row's pattern does not match.
fn filtered(detail: Value) -> Entry {
    Entry {
        detail,
        matched: false,
    }
}

/// Each entry of one publication is routed on its own, and a pattern naming a member nested
/// inside `detail` delivers exactly the entries whose value at that path it lists.
#[tokio::test]
async fn a_nested_detail_pattern_delivers_exactly_the_entries_it_matches() {
    let rows = [
        FilterRow {
            case: "one nested member, the matching entry last in the request",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "state": ["Deployed"] }),
            entries: vec![
                filtered(json!({ "state": "InProgress" })),
                matching(json!({ "state": "Deployed" })),
            ],
        },
        FilterRow {
            case: "one nested member, the matching entry first in the request",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "state": ["Deployed"] }),
            entries: vec![
                matching(json!({ "state": "Deployed" })),
                filtered(json!({ "state": "InProgress" })),
            ],
        },
        FilterRow {
            case: "a pattern listing two values, two of three entries matching",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "state": ["Deployed", "Disabled"] }),
            entries: vec![
                filtered(json!({ "state": "InProgress" })),
                matching(json!({ "state": "Deployed" })),
                matching(json!({ "state": "Disabled" })),
            ],
        },
        FilterRow {
            case: "a member nested two levels inside detail",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "distribution": { "status": ["Deployed"] } }),
            entries: vec![
                filtered(json!({ "distribution": { "status": "InProgress" } })),
                matching(json!({ "distribution": { "status": "Deployed" } })),
            ],
        },
        FilterRow {
            case: "two nested members, an entry matching only one of them filtered out",
            detail_type: SIMULATED_CALL,
            detail_pattern: json!({ "eventName": ["CreateInvalidation"], "state": ["Deployed"] }),
            entries: vec![
                filtered(json!({ "eventName": "CreateInvalidation", "state": "InProgress" })),
                filtered(json!({ "eventName": "UpdateDistribution", "state": "Deployed" })),
                matching(json!({ "eventName": "CreateInvalidation", "state": "Deployed" })),
            ],
        },
        FilterRow {
            case: "an entry whose nested member is an array containing the listed value",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "states": ["Deployed"] }),
            entries: vec![
                filtered(json!({ "states": ["InProgress", "Disabled"] })),
                matching(json!({ "states": ["InProgress", "Deployed"] })),
            ],
        },
        FilterRow {
            case: "an entry carrying none of the members the pattern names",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "state": ["Deployed"] }),
            entries: vec![filtered(json!({ "distributionOnly": "yes" }))],
        },
        FilterRow {
            case: "no entry of the request matching at all",
            detail_type: STATE_CHANGE,
            detail_pattern: json!({ "state": ["Deployed"] }),
            entries: vec![
                filtered(json!({ "state": "InProgress" })),
                filtered(json!({ "state": "Disabled" })),
            ],
        },
    ];


    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let rule = format!("nested-detail-rule-{index}");
        let pattern = json!({
            "source": [SOURCE],
            "detail-type": [row.detail_type],
            "detail": row.detail_pattern,
        })
        .to_string();
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": pattern, "State": "ENABLED" }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");

        let (url, queue_arn) =
            given_queue_for_rule(&emulator, case, &format!("nested-detail-q-{index}"), &rule).await;
        let (status, body) = events(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": [{ "Id": "sqs-target", "Arn": queue_arn }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutTargets answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutTargets reported a failed entry: {body}"
        );

        // Each entry carries a marker of its own, so which entry a message came from is
        // readable off the message rather than inferred from its position.
        let markers: Vec<String> = (0..row.entries.len())
            .map(|position| format!("case-{index}-entry-{position}"))
            .collect();
        let entries: Vec<Value> = row
            .entries
            .iter()
            .zip(&markers)
            .map(|(entry, marker)| {
                let mut detail = entry.detail.clone();
                detail["distributionId"] = json!(marker);
                json!({
                    "Source": SOURCE,
                    "DetailType": row.detail_type,
                    "Detail": detail.to_string(),
                })
            })
            .collect();
        let (status, body) = events(&emulator, "PutEvents", &json!({ "Entries": entries })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutEvents answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutEvents reported a failed entry: {body}"
        );
        let event_ids: Vec<String> = body["Entries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["EventId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            event_ids.len(),
            row.entries.len(),
            "case {case:?}: expected one EventId per entry, got {body}"
        );

        let expected: Vec<usize> = row
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.matched)
            .map(|(position, _)| position)
            .collect();
        let delivered = delivered_envelopes(&emulator, case, &url, expected.len()).await;
        assert_eq!(
            delivered.len(),
            expected.len(),
            "case {case:?}: expected {} message(s) on the queue, got {delivered:?}",
            expected.len()
        );
        for position in expected {
            let marker = &markers[position];
            let envelope = delivered
                .iter()
                .find(|envelope| envelope["detail"]["distributionId"] == json!(marker))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no envelope for entry {position} in {delivered:?}")
                });
            assert_eq!(
                envelope["id"],
                json!(event_ids[position]),
                "case {case:?}: entry {position} arrived under another id: {envelope}"
            );
            assert_eq!(
                envelope["source"],
                json!(SOURCE),
                "case {case:?}: entry {position} arrived with another source: {envelope}"
            );
            assert_eq!(
                envelope["detail-type"],
                json!(row.detail_type),
                "case {case:?}: entry {position} arrived with another detail-type: {envelope}"
            );
            let mut submitted = row.entries[position].detail.clone();
            submitted["distributionId"] = json!(marker);
            assert_eq!(
                envelope["detail"], submitted,
                "case {case:?}: entry {position} arrived with another detail: {envelope}"
            );
        }
        for (position, entry) in row.entries.iter().enumerate() {
            if entry.matched {
                continue;
            }
            let marker = &markers[position];
            assert!(
                !delivered
                    .iter()
                    .any(|envelope| envelope["detail"]["distributionId"] == json!(marker)),
                "case {case:?}: entry {position} matched no rule yet was delivered: {delivered:?}"
            );
        }
    }
}

/// One row of the target-role table.
struct TrustRow {
    case: &'static str,
    /// The service principal the target role's trust policy names.
    principal: &'static str,
    /// The `Condition` scoping that trust statement, when the row states one; it is written
    /// from the ARN of the rule that invokes.
    condition: Option<fn(&str) -> Value>,
    /// Whether EventBridge starts an execution under that role.
    started: bool,
}

/// A state-machine target is started only under a role whose trust policy lets this service
/// assume it for the invoking rule, and the execution it starts carries the envelope and
/// reaches a terminal status on the definition alone.
#[tokio::test]
async fn a_state_machine_target_is_started_only_under_a_role_that_trusts_the_rule() {
    let rows = [
        TrustRow {
            case: "a trust policy naming another service",
            principal: "lambda.amazonaws.com",
            condition: None,
            started: false,
        },
        TrustRow {
            case: "a trust policy naming the target's own service",
            principal: "states.amazonaws.com",
            condition: None,
            started: false,
        },
        TrustRow {
            case: "a trust policy naming this service",
            principal: SERVICE_PRINCIPAL,
            condition: None,
            started: true,
        },
        TrustRow {
            case: "a trust scoped by ArnEquals to the invoking rule",
            principal: SERVICE_PRINCIPAL,
            condition: Some(|rule_arn: &str| json!({ "ArnEquals": { "aws:SourceArn": rule_arn } })),
            started: true,
        },
        TrustRow {
            case: "a trust scoped by ArnEquals to another rule of the account",
            principal: SERVICE_PRINCIPAL,
            condition: Some(|_: &str| {
                json!({ "ArnEquals": { "aws:SourceArn": another_rule_arn() } })
            }),
            started: false,
        },
        TrustRow {
            case: "a trust scoped by StringEquals to the account the rule belongs to",
            principal: SERVICE_PRINCIPAL,
            condition: Some(|_: &str| {
                json!({ "StringEquals": { "aws:SourceAccount": account() } })
            }),
            started: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let machine = given_state_machine(&emulator, case, &format!("echo-flow-{index}")).await;
        let rule = format!("sfn-target-rule-{index}");
        let invoking = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, &rule);
        let trust = trust_document(row.principal, row.condition.map(|written| written(&invoking)));
        let role = given_role(
            &emulator,
            case,
            &format!("sfn-target-role-{index}"),
            &trust,
            Some(&machine),
        )
        .await;
        given_matching_rule(&emulator, case, &rule).await;
        put_target(&emulator, case, &rule, &machine, &role).await;

        let marker = format!("marker-{index}");
        publish_marker(&emulator, case, &marker).await;

        let started = executions_carrying(&emulator, case, &machine, &marker, row.started).await;
        if row.started {
            assert_eq!(
                started.len(),
                1,
                "case {case:?}: expected one execution carrying {marker}, got {started:?}"
            );
            assert_eq!(
                started[0].status, SUCCEEDED,
                "case {case:?}: the execution ended {:?}, not {SUCCEEDED}",
                started[0].status
            );
            let input: Value = serde_json::from_str(&started[0].input).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the execution input is not JSON ({error}): {:?}",
                    started[0].input
                )
            });
            assert_eq!(
                input["detail"]["marker"],
                json!(marker),
                "case {case:?}: the execution input is not the envelope: {input}"
            );
            assert_eq!(
                input["source"],
                json!(SOURCE),
                "case {case:?}: the execution input is not the envelope: {input}"
            );
            assert_eq!(
                metric_sum(&emulator, INVOCATIONS_METRIC, DEFAULT_EVENT_BUS_NAME, &rule).await,
                1.0,
                "case {case:?}: the accepted handover was not counted on {INVOCATIONS_METRIC}"
            );
        } else {
            assert!(
                started.is_empty(),
                "case {case:?}: a role this service cannot assume started {started:?}"
            );
            assert_eq!(
                metric_sum(
                    &emulator,
                    FAILED_INVOCATIONS_METRIC,
                    DEFAULT_EVENT_BUS_NAME,
                    &rule
                )
                .await,
                1.0,
                "case {case:?}: the failed handover was not counted on \
                 {FAILED_INVOCATIONS_METRIC}"
            );
        }
    }
}

/// Re-pointing one target `Id` at another role decides the NEXT event by the document in
/// force then: the event published under a role this service cannot assume starts nothing,
/// and the one published after the same target names a role it can starts an execution of its
/// own.
#[tokio::test]
async fn re_pointing_a_target_at_a_trusted_role_starts_the_next_event() {
    let emulator = emulator();
    let case = "a target re-pointed from an untrusted role to a trusted one";
    let machine = given_state_machine(&emulator, case, "repointed-flow").await;
    let rule = "repointed-rule";
    given_matching_rule(&emulator, case, rule).await;

    let untrusted = given_role(
        &emulator,
        case,
        "repointed-untrusted",
        &trust_document("lambda.amazonaws.com", None),
        Some(&machine),
    )
    .await;
    put_target(&emulator, case, rule, &machine, &untrusted).await;
    let withheld = "marker-withheld";
    publish_marker(&emulator, case, withheld).await;
    let started = executions_carrying(&emulator, case, &machine, withheld, false).await;
    assert!(
        started.is_empty(),
        "case {case:?}: the untrusted role started {started:?}"
    );

    let trusted = given_role(
        &emulator,
        case,
        "repointed-trusted",
        &trust_document(SERVICE_PRINCIPAL, None),
        Some(&machine),
    )
    .await;
    put_target(&emulator, case, rule, &machine, &trusted).await;
    let delivered = "marker-delivered";
    publish_marker(&emulator, case, delivered).await;
    let started = executions_carrying(&emulator, case, &machine, delivered, true).await;
    assert_eq!(
        started.len(),
        1,
        "case {case:?}: expected one execution once the target named a trusted role, got \
         {started:?}"
    );
    assert_eq!(
        started[0].status, SUCCEEDED,
        "case {case:?}: the execution ended {:?}, not {SUCCEEDED}",
        started[0].status
    );
    // The event published while the target named the untrusted role is still not there: a
    // grant written later does not deliver an event that was already refused.
    let withheld_still = executions_carrying(&emulator, case, &machine, withheld, false).await;
    assert!(
        withheld_still.is_empty(),
        "case {case:?}: the earlier event appeared after the re-point: {withheld_still:?}"
    );
}

/// The ARN of a rule of this account and Region that is NOT the one invoking.
fn another_rule_arn() -> String {
    rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule")
}

/// A queue granting this service `sqs:SendMessage` only for the deliveries of the rule
/// `rule`, which is the grant a client writes for one rule's target; answers its url and ARN.
async fn given_queue_for_rule(
    emulator: &Emulator,
    case: &str,
    name: &str,
    rule: &str,
) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {body}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridgeForThisRule",
            "Effect": "Allow",
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
            "Condition": {
                "ArnEquals": {
                    "aws:SourceArn": rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, rule),
                },
            },
        }],
    })
    .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
    (url, arn)
}

/// The envelopes waiting on the queue at `url`, looked for again until `expected` of them are
/// there or [`DELIVERY_BUDGET`] has passed — and for [`ABSENCE_WINDOW`] when `expected` is
/// none, which is how a row that expects nothing concludes nothing came.
async fn delivered_envelopes(
    emulator: &Emulator,
    case: &str,
    url: &str,
    expected: usize,
) -> Vec<Value> {
    let budget = if expected == 0 {
        ABSENCE_WINDOW
    } else {
        DELIVERY_BUDGET
    };
    let began = Instant::now();
    let mut envelopes: Vec<Value> = Vec::new();
    loop {
        collect_envelopes(emulator, case, url, &mut envelopes).await;
        // A row expecting nothing keeps looking for its whole window; one expecting messages
        // stops as soon as they are all there, and then looks ONCE more, so a message the
        // pattern should have filtered out is seen rather than missed.
        if expected > 0 && envelopes.len() >= expected {
            tokio::time::sleep(POLL_INTERVAL).await;
            collect_envelopes(emulator, case, url, &mut envelopes).await;
            return envelopes;
        }
        if began.elapsed() >= budget {
            return envelopes;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One look at the queue at `url`: every message waiting there, as the envelope its body is.
async fn collect_envelopes(
    emulator: &Emulator,
    case: &str,
    url: &str,
    envelopes: &mut Vec<Value>,
) {
    for message in drain(emulator, url).await {
        let body = message["Body"].as_str().unwrap_or_default();
        envelopes.push(serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("case {case:?}: the delivered body is not JSON ({error}): {body:?}")
        }));
    }
}

/// A state machine whose definition is [`PASS_DEFINITION`], under an execution role of its
/// own; answers its ARN.
async fn given_state_machine(emulator: &Emulator, case: &str, name: &str) -> String {
    let role = given_role(
        emulator,
        case,
        &format!("{name}-execution-role"),
        &trust_document("states.amazonaws.com", None),
        None,
    )
    .await;
    let (status, created) = sfn(
        emulator,
        "CreateStateMachine",
        &json!({ "name": name, "definition": PASS_DEFINITION, "roleArn": role }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStateMachine answered {status} {created}"
    );
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"))
        .to_string()
}

/// A trust policy allowing `principal` to assume the role, scoped by `condition` when one is
/// given — written as a client sends it.
fn trust_document(principal: &str, condition: Option<Value>) -> String {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": principal },
        "Action": "sts:AssumeRole",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string()
}

/// A role called `name` carrying the trust document `trust`, plus an inline policy allowing
/// `states:StartExecution` on `start_execution_on` when a row names a machine; answers the ARN
/// IAM persisted.
async fn given_role(
    emulator: &Emulator,
    case: &str,
    name: &str,
    trust: &str,
    start_execution_on: Option<&str>,
) -> String {
    let (status, created) = iam(
        emulator,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    let arn =
        xml_text(&created, "Arn").unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));
    if let Some(machine) = start_execution_on {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "states:StartExecution",
                "Resource": machine,
            }],
        })
        .to_string();
        let (status, written) = iam(
            emulator,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "start-execution"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutRolePolicy answered {status}: {written}"
        );
    }
    arn
}

/// A rule matching the entries [`publish_marker`] publishes, on a nested `detail` member.
async fn given_matching_rule(emulator: &Emulator, case: &str, rule: &str) {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [SIMULATED_CALL],
        "detail": { "eventName": ["CreateInvalidation"] },
    })
    .to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({ "Name": rule, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");
}

/// Point the one target `Id` of `rule` at the state machine `machine`, reached as `role`.
async fn put_target(emulator: &Emulator, case: &str, rule: &str, machine: &str, role: &str) {
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [{ "Id": "sfn-target", "Arn": machine, "RoleArn": role }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
}

/// Publish one entry the rule matches, carrying `marker` in its `detail`.
async fn publish_marker(emulator: &Emulator, case: &str, marker: &str) {
    let detail = json!({ "eventName": "CreateInvalidation", "marker": marker }).to_string();
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": SOURCE,
                "DetailType": SIMULATED_CALL,
                "Detail": detail,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
}

/// One execution of a state machine, as a client reads it back.
#[derive(Debug)]
struct Execution {
    status: String,
    input: String,
}

/// Every execution of `machine` whose `input` carries `marker`, looked for again until one is
/// there and terminal (`expected`), or for [`ABSENCE_WINDOW`] when none is expected.
async fn executions_carrying(
    emulator: &Emulator,
    case: &str,
    machine: &str,
    marker: &str,
    expected: bool,
) -> Vec<Execution> {
    let budget = if expected {
        DELIVERY_BUDGET
    } else {
        ABSENCE_WINDOW
    };
    let began = Instant::now();
    loop {
        let found = described_executions(emulator, case, machine, marker).await;
        let settled = !found.is_empty() && found.iter().all(|one| one.status == SUCCEEDED);
        if (expected && settled) || began.elapsed() >= budget {
            return found;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One pass of [`executions_carrying`]: what `ListExecutions` and `DescribeExecution` report
/// about the executions of `machine` whose input carries `marker`.
async fn described_executions(
    emulator: &Emulator,
    case: &str,
    machine: &str,
    marker: &str,
) -> Vec<Execution> {
    let (status, listed) = sfn(
        emulator,
        "ListExecutions",
        &json!({ "stateMachineArn": machine }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListExecutions answered {status} {listed}"
    );
    let mut found = Vec::new();
    for execution in listed["executions"].as_array().cloned().unwrap_or_default() {
        let arn = execution["executionArn"].as_str().unwrap_or_default();
        let (status, described) = sfn(
            emulator,
            "DescribeExecution",
            &json!({ "executionArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeExecution answered {status} {described}"
        );
        let input = described["input"].as_str().unwrap_or_default().to_string();
        if input.contains(marker) {
            found.push(Execution {
                status: described["status"].as_str().unwrap_or_default().to_string(),
                input,
            });
        }
    }
    found
}
