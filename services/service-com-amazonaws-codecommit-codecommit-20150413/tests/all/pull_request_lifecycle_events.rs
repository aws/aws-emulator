// SPDX-License-Identifier: CC0-1.0

//! What EventBridge is told when a pull request changes.
//!
//! THE RULE UNDER TEST: every change CodeCommit records on a pull request is also
//! PUBLISHED onto the account's default event bus, with no opt-in and no trigger
//! configuration, as `source` `aws.codecommit` with `detail-type` `CodeCommit Pull
//! Request State Change`, naming the REPOSITORY's ARN as the one entry of `resources`
//! and the change in `detail.event`, over the pull request as the API itself reports
//! it. So a rule whose `EventPattern` pins `resources` to one repository's ARN receives
//! that repository's pull-request events and no other repository's, and a call that
//! recorded nothing — one refused, or one that asked for the state the pull request
//! already held — publishes nothing.
//!
//! The table below spans what the rule generalises over: the KIND of change and the
//! operation that made it (opening a pull request, a commit written to its source
//! branch through `PutFile` and through `CreateCommit`, an approval rule created on it
//! and deleted from it, a reviewer's approval, an override of its rules, and the status
//! update that closed it), each with the `detail.event` AWS names it by.
//!
//! Everything is driven over the real wire path: this service, EventBridge and Amazon
//! SQS are mounted on ONE shared engine, the rule and its target are created through
//! EventBridge's and SQS's own APIs, and every delivered envelope is read back through
//! SQS's.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::pull_request_events::{
    EVENT_PULL_REQUEST_APPROVAL_RULE_CREATED, EVENT_PULL_REQUEST_APPROVAL_RULE_DELETED,
    EVENT_PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN, EVENT_PULL_REQUEST_APPROVAL_STATE_CHANGED,
    EVENT_PULL_REQUEST_CREATED, EVENT_PULL_REQUEST_MERGE_STATUS_UPDATED,
    EVENT_PULL_REQUEST_SOURCE_BRANCH_UPDATED, EVENT_PULL_REQUEST_STATUS_CHANGED, EVENT_SOURCE,
    PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE,
};

use crate::repository_lifecycle::{base64, string_at, REGION};
use crate::wire::{caller_arn, fresh_client_request_token, hold_long_term_key, ok_as, call_as};

/// The caller every CodeCommit call below is signed as.
const CALLER: &str = "AKIDEVENTS";

/// The caller the CONSUMER side — EventBridge and Amazon SQS — is driven as: the
/// account's own credential, which is the principal an emulator publishes for its
/// operator, so wiring a rule to a queue needs no identity-based policy of its own.
const CONSUMER: &str = "AKIDACCOUNT";

/// A second principal of the account: a pull request cannot be approved by the caller
/// who created it, so the approval below is made by a reviewer of its own.
const REVIEWER: &str = "AKIDREVIEWER";

/// The branches a pull request is between: `main` takes the first commit, so it is the
/// repository's default branch and the destination; `feature` is the source.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// How long a case polls for a delivery before it gives up.
///
/// Generous on purpose: it bounds a POLL LOOP that ends as soon as the condition holds,
/// so a fast machine never waits for it and a machine running the whole workspace's
/// tests at once is not failed by it.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll loop waits between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// This service beside EventBridge and Amazon SQS on one shared engine, so a rule's
/// target really receives what this service publishes.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    hold_long_term_key(&engine, CALLER);
    hold_long_term_key(&engine, REVIEWER);
    router
}

/// One awsJson call to a mounted service other than this one, the way an SDK sends it.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={CONSUMER}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// One EventBridge call.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "EventBridge {operation}: expected 200, got {status} with {payload}"
    );
    payload
}

/// One Amazon SQS call.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "Amazon SQS {operation}: expected 200, got {status} with {payload}"
    );
    payload
}

/// A queue behind a rule matching `pattern`; answers the queue's url.
async fn given_target(router: &MultiServiceRouter, name: &str, pattern: &Value) -> String {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = string_at(&created, "/QueueUrl");
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = string_at(&attributes, "/Attributes/QueueArn");
    let rule = events(
        router,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
    )
    .await;
    let rule_arn = string_at(&rule, "/RuleArn");
    // EventBridge delivers as its own service principal, so the queue's
    // resource-based policy is what lets the target through.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": rule_arn } },
        }],
    })
    .to_string();
    sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    let targets = events(
        router,
        "PutTargets",
        &json!({ "Rule": name, "Targets": [{ "Id": "sqs-target", "Arn": queue_arn }] }),
    )
    .await;
    assert_eq!(
        targets["FailedEntryCount"],
        json!(0),
        "PutTargets reported a failed entry: {targets}"
    );
    url
}

/// Every envelope waiting on the queue at `url`.
async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null)
        })
        .collect()
}

/// Poll the queue until it has handed over at least `wanted` envelopes, or the generous
/// deadline passes. Waits for the CONDITION, so a fast machine never waits.
async fn delivered(router: &MultiServiceRouter, url: &str, wanted: usize) -> Vec<Value> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    let mut collected: Vec<Value> = Vec::new();
    loop {
        collected.extend(drain(router, url).await);
        if collected.len() >= wanted || std::time::Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The pattern a rule for one repository's pull-request events carries: the source and
/// detail-type this service publishes under, scoped to that repository's ARN.
fn pull_request_pattern(repository_arn: &str) -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE],
        "resources": [repository_arn],
    })
}

/// An approval rule document, as compact JSON text.
fn document(approvals: u32) -> String {
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [{
            "Type": STATEMENT_TYPE,
            "NumberOfApprovalsNeeded": approvals,
        }],
    }))
    .expect("a JSON document")
}

/// Create the repository `name`, answering its ARN as `CreateRepository` reported it.
async fn given_repository(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok_as(
        router,
        CALLER,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
    string_at(&created, "/repositoryMetadata/Arn")
}

/// Write one commit on `branch` through `PutFile`, reporting its id.
async fn put_file(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    parent: Option<&str>,
    content: &[u8],
) -> String {
    let mut body = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": "README.md",
        "fileContent": base64(content),
        "commitMessage": "a revision",
    });
    if let Some(parent) = parent {
        body["parentCommitId"] = json!(parent);
    }
    string_at(&ok_as(router, CALLER, "PutFile", &body).await, "/commitId")
}

/// A repository with `main` and `feature` at the same commit, and one OPEN pull request
/// from `feature` into `main`: the base commit, the source tip and the pull request id.
struct Opened {
    base: String,
    source_tip: String,
    pull_request_id: String,
}

async fn given_pull_request(router: &MultiServiceRouter, repository: &str) -> Opened {
    let base = put_file(router, repository, DESTINATION, None, b"alpha\n").await;
    ok_as(
        router,
        CALLER,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    let source_tip = put_file(
        router,
        repository,
        SOURCE,
        Some(&base),
        b"alpha\nbeta\n",
    )
    .await;
    let created = ok_as(
        router,
        CALLER,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": repository,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    Opened {
        base,
        source_tip,
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
    }
}

/// The envelopes among `delivered` whose `detail.event` is `event`.
fn of_event<'a>(delivered: &'a [Value], event: &str) -> Vec<&'a Value> {
    delivered
        .iter()
        .filter(|envelope| envelope["detail"]["event"] == json!(event))
        .collect()
}

/// Assert the envelope's own members: the source, the detail-type, the repository ARN as
/// the only resource, the account and the Region the caller called in, and the pull
/// request and repository the change is about.
fn assert_envelope(
    case: &str,
    envelope: &Value,
    repository_arn: &str,
    repository: &str,
    pull_request_id: &str,
) {
    assert_eq!(
        envelope["source"],
        json!(EVENT_SOURCE),
        "case {case:?}: expected the source {EVENT_SOURCE}, got {envelope}"
    );
    assert_eq!(
        envelope["detail-type"],
        json!(PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE),
        "case {case:?}: expected the detail-type \
         {PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE}, got {envelope}"
    );
    assert_eq!(
        envelope["resources"],
        json!([repository_arn]),
        "case {case:?}: expected the repository ARN as the one resource, got {envelope}"
    );
    assert_eq!(
        envelope["account"],
        json!(engine::emulator_account_id()),
        "case {case:?}: expected the caller's account, got {envelope}"
    );
    assert_eq!(
        envelope["region"],
        json!(REGION),
        "case {case:?}: expected the caller's Region, got {envelope}"
    );
    assert_eq!(
        envelope["detail"]["pullRequestId"],
        json!(pull_request_id),
        "case {case:?}: expected the pull request the API reported, got {envelope}"
    );
    assert_eq!(
        envelope["detail"]["repositoryNames"],
        json!([repository]),
        "case {case:?}: expected the repository name in a list, got {envelope}"
    );
    assert_eq!(
        envelope["detail"]["author"],
        json!(caller_arn(CALLER)),
        "case {case:?}: expected the pull request's author, got {envelope}"
    );
    assert!(
        envelope["time"].is_string(),
        "case {case:?}: expected the event stamped with when the change happened, got \
         {envelope}"
    );
}

/// One row of the lifecycle table: the change made, the `detail.event` it must be
/// published as, the `detail` member that tells it from another change of the same kind
/// (`None` where the kind happened once), and the `detail` members that change itself
/// decides.
struct Row {
    case: &'static str,
    event: &'static str,
    key: Option<(&'static str, Value)>,
    fields: Vec<(&'static str, Value)>,
}

/// The envelopes among `delivered` that are the change `row` describes: its kind, and
/// its key where two changes of that kind happened.
fn envelopes_for<'a>(delivered: &'a [Value], row: &Row) -> Vec<&'a Value> {
    of_event(delivered, row.event)
        .into_iter()
        .filter(|envelope| {
            row.key.as_ref().is_none_or(|(member, expected)| {
                &envelope["detail"][member] == expected
            })
        })
        .collect()
}

#[tokio::test]
async fn every_recorded_pull_request_change_reaches_the_default_bus() {
    let router = emulator();
    let watched = "watched-repository";
    let ignored = "ignored-repository";
    let watched_arn = given_repository(&router, watched).await;
    let ignored_arn = given_repository(&router, ignored).await;

    // One queue behind a rule scoped to the watched repository's ARN, and one behind a
    // rule written for another service's events.
    let scoped = given_target(
        &router,
        "codecommit-pull-requests",
        &pull_request_pattern(&watched_arn),
    )
    .await;
    let unrelated = given_target(
        &router,
        "another-services-events",
        &json!({ "source": ["aws.ecs"], "detail-type": ["ECS Task State Change"] }),
    )
    .await;

    let Opened {
        base,
        source_tip,
        pull_request_id,
    } = given_pull_request(&router, watched).await;
    let mut rows = vec![Row {
        case: "the pull request was opened",
        event: EVENT_PULL_REQUEST_CREATED,
        key: None,
        fields: vec![
            ("sourceCommit", json!(source_tip)),
            ("destinationCommit", json!(base)),
            ("pullRequestStatus", json!("OPEN")),
        ],
    }];

    // A commit written to the source branch through `PutFile` …
    let through_put_file = put_file(
        &router,
        watched,
        SOURCE,
        Some(&source_tip),
        b"alpha\nbeta\ngamma\n",
    )
    .await;
    rows.push(Row {
        case: "a commit was written to the source branch through PutFile",
        event: EVENT_PULL_REQUEST_SOURCE_BRANCH_UPDATED,
        key: Some(("sourceCommit", json!(through_put_file))),
        fields: vec![("beforeCommitId", json!(source_tip))],
    });

    // … and one written through `CreateCommit`: the rule is about the branch moving,
    // not about which operation moved it.
    let committed = ok_as(
        &router,
        CALLER,
        "CreateCommit",
        &json!({
            "repositoryName": watched,
            "branchName": SOURCE,
            "parentCommitId": through_put_file,
            "commitMessage": "another revision",
            "putFiles": [{
                "filePath": "NOTES.md",
                "fileContent": base64(b"delta\n"),
            }],
        }),
    )
    .await;
    let through_create_commit = string_at(&committed, "/commitId");
    rows.push(Row {
        case: "a commit was written to the source branch through CreateCommit",
        event: EVENT_PULL_REQUEST_SOURCE_BRANCH_UPDATED,
        key: Some(("sourceCommit", json!(through_create_commit))),
        fields: vec![("beforeCommitId", json!(through_put_file))],
    });

    // An approval rule created on the pull request, a reviewer's approval of it, the
    // override of its rules, and the rule's deletion.
    let rule = ok_as(
        &router,
        CALLER,
        "CreatePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": "two-reviewers",
            "approvalRuleContent": document(2),
        }),
    )
    .await;
    let approval_rule_id = string_at(&rule, "/approvalRule/approvalRuleId");
    rows.push(Row {
        case: "an approval rule was created on the pull request",
        event: EVENT_PULL_REQUEST_APPROVAL_RULE_CREATED,
        key: None,
        fields: vec![
            ("approvalRuleId", json!(approval_rule_id)),
            ("approvalRuleName", json!("two-reviewers")),
        ],
    });

    let revision_id = string_at(
        &ok_as(
            &router,
            CALLER,
            "GetPullRequest",
            &json!({ "pullRequestId": pull_request_id }),
        )
        .await,
        "/pullRequest/revisionId",
    );
    ok_as(
        &router,
        REVIEWER,
        "UpdatePullRequestApprovalState",
        &json!({
            "pullRequestId": pull_request_id,
            "revisionId": revision_id,
            "approvalState": "APPROVE",
        }),
    )
    .await;
    rows.push(Row {
        case: "a reviewer approved the pull request",
        event: EVENT_PULL_REQUEST_APPROVAL_STATE_CHANGED,
        key: None,
        fields: vec![
            ("approvalStatus", json!("APPROVE")),
            // The identity that made the change, which is not the author here.
            ("callerUserArn", json!(caller_arn(REVIEWER))),
        ],
    });

    ok_as(
        &router,
        CALLER,
        "OverridePullRequestApprovalRules",
        &json!({
            "pullRequestId": pull_request_id,
            "revisionId": revision_id,
            "overrideStatus": "OVERRIDE",
        }),
    )
    .await;
    rows.push(Row {
        case: "the pull request's approval rules were overridden",
        event: EVENT_PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN,
        key: None,
        fields: vec![("overrideStatus", json!("OVERRIDE"))],
    });

    ok_as(
        &router,
        CALLER,
        "DeletePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": "two-reviewers",
        }),
    )
    .await;
    rows.push(Row {
        case: "the approval rule was deleted from the pull request",
        event: EVENT_PULL_REQUEST_APPROVAL_RULE_DELETED,
        key: None,
        fields: vec![("approvalRuleName", json!("two-reviewers"))],
    });

    // The status update that closes it, which is the last change its log records.
    let closed = ok_as(
        &router,
        CALLER,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": pull_request_id,
            "pullRequestStatus": "CLOSED",
        }),
    )
    .await;
    let reported_status = string_at(&closed, "/pullRequest/pullRequestStatus");
    rows.push(Row {
        case: "the pull request was closed",
        event: EVENT_PULL_REQUEST_STATUS_CHANGED,
        key: None,
        fields: vec![("pullRequestStatus", json!(reported_status))],
    });

    // A call that recorded nothing publishes nothing: the status the pull request
    // already holds, and a status move the model admits no transition for.
    ok_as(
        &router,
        CALLER,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": pull_request_id,
            "pullRequestStatus": "CLOSED",
        }),
    )
    .await;
    let (refused_status, refused) = call_as(
        &router,
        CALLER,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": pull_request_id,
            "pullRequestStatus": "OPEN",
        }),
    )
    .await;
    assert_eq!(
        refused_status, 400,
        "case \"a reopen the status admits no transition for\": expected 400, got \
         {refused_status} with {refused}"
    );

    // The SECOND repository's pull request: a rule scoped to the first repository's ARN
    // must receive none of it.
    let elsewhere = given_pull_request(&router, ignored).await;
    ok_as(
        &router,
        CALLER,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": elsewhere.pull_request_id,
            "pullRequestStatus": "CLOSED",
        }),
    )
    .await;

    let envelopes = delivered(&router, &scoped, rows.len()).await;
    assert_eq!(
        envelopes.len(),
        rows.len(),
        "expected exactly one envelope per recorded change, got {envelopes:?}"
    );
    for row in &rows {
        let Row {
            case,
            event,
            key: _,
            fields,
        } = row;
        let mine = envelopes_for(&envelopes, row);
        assert_eq!(
            mine.len(),
            1,
            "case {case:?}: expected exactly one {event} envelope, got {envelopes:?}"
        );
        let envelope = mine[0];
        assert_envelope(case, envelope, &watched_arn, watched, &pull_request_id);
        for (member, expected) in fields {
            assert_eq!(
                &envelope["detail"][member],
                expected,
                "case {case:?}: expected detail.{member} to be {expected}, got {envelope}"
            );
        }
    }

    // Nothing of the unwatched repository's pull request, and nothing at all for a rule
    // written for another service's events.
    for envelope in &envelopes {
        assert_ne!(
            envelope["resources"],
            json!([ignored_arn]),
            "a rule scoped to one repository must not receive another's, got {envelope}"
        );
        assert_ne!(
            envelope["detail"]["pullRequestId"],
            json!(elsewhere.pull_request_id),
            "a rule scoped to one repository must not receive another's pull request, got \
             {envelope}"
        );
    }
    let others = drain(&router, &unrelated).await;
    assert!(
        others.is_empty(),
        "case \"a rule for another service\": expected no delivery, got {others:?}"
    );
}

#[tokio::test]
async fn merging_a_pull_request_publishes_its_merge_and_its_close() {
    let router = emulator();
    let repository = "merged-pull-requests";
    let repository_arn = given_repository(&router, repository).await;
    let url = given_target(
        &router,
        "codecommit-merges",
        &pull_request_pattern(&repository_arn),
    )
    .await;

    let Opened {
        pull_request_id, ..
    } = given_pull_request(&router, repository).await;
    let merged = ok_as(
        &router,
        CALLER,
        "MergePullRequestByThreeWay",
        &json!({
            "pullRequestId": pull_request_id,
            "repositoryName": repository,
        }),
    )
    .await;
    let merge_commit_id = string_at(
        &merged,
        "/pullRequest/pullRequestTargets/0/mergeMetadata/mergeCommitId",
    );
    let status = string_at(&merged, "/pullRequest/pullRequestStatus");

    // Opening it, then the merge of its target and the close that merge caused.
    let envelopes = delivered(&router, &url, 3).await;
    let rows = [
        Row {
            case: "the merge of the pull request's target",
            event: EVENT_PULL_REQUEST_MERGE_STATUS_UPDATED,
            key: None,
            fields: vec![
                ("mergeCommitId", json!(merge_commit_id)),
                ("mergeOption", json!("THREE_WAY_MERGE")),
            ],
        },
        Row {
            case: "the close the merge caused",
            event: EVENT_PULL_REQUEST_STATUS_CHANGED,
            key: None,
            fields: vec![("pullRequestStatus", json!(status))],
        },
    ];
    for row in &rows {
        let Row {
            case,
            event,
            key: _,
            fields,
        } = row;
        let mine = envelopes_for(&envelopes, row);
        assert_eq!(
            mine.len(),
            1,
            "case {case:?}: expected exactly one {event} envelope, got {envelopes:?}"
        );
        let envelope = mine[0];
        assert_envelope(case, envelope, &repository_arn, repository, &pull_request_id);
        for (member, expected) in fields {
            assert_eq!(
                &envelope["detail"][member],
                expected,
                "case {case:?}: expected detail.{member} to be {expected}, got {envelope}"
            );
        }
    }
}
