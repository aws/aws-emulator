// SPDX-License-Identifier: CC0-1.0

//! WHICH member of a published `detail` carries the state the moved record went into.
//!
//! The RULE under test: every `detail` member of an AWS Backup state-change event is named
//! after the member the MOVED RECORD's own API reports it under, with a lower-case first
//! letter — so the member carrying the state a record moved into is `state` for a record the
//! model gives a `State` (a backup job's `BackupJob$State`, a copy job's `CopyJob$State`) and
//! `status` for one it gives a `Status` (a recovery point's `RecoveryPoint$Status`, of the
//! enum `RecoveryPointStatus`; a restore job's `Status`) — and no envelope carries the OTHER
//! kind's spelling of it. A client therefore filters a recovery point's envelope on
//! `detail.status` and a job's on `detail.state`, and each value is the one a read of that
//! same record reports.
//!
//! Source for the member names: this service's own model, which gives a backup job and a copy
//! job a `State` and a recovery point a `Status`
//! (`smithy_query(query="operation", operation="DescribeBackupJob,DescribeCopyJob,DescribeRecoveryPoint",
//! model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")`);
//! the EventBridge envelope itself is in no Smithy model, so every value asserted here is
//! compared against what the control plane reports for the same record.
//!
//! The first test spans that rule along what it generalises over: the KIND of record that
//! moved — a backup job, a copy job, and the recovery point EACH of those two wrote — all
//! reached from one pair of starts and one rule matching the source alone, so the rows differ
//! only in the record whose move they are about. (`restore_job_state_change_events.rs` is the
//! suite for the fourth kind, whose `Status` the same rule spells `status`.)
//!
//! The second test is the observation a client makes of it: a rule whose pattern filters a
//! recovery point's envelope on `detail.status` delivers to an Amazon SNS topic target, whose
//! own `sqs`-protocol subscriber then holds the notification carrying that envelope — while a
//! sibling rule on the same bus, identical but for filtering on `detail.state`, receives
//! nothing, because a recovery point's detail carries no such member. The notification's
//! arrival is the positive control for that silence, so the empty queue is a refusal to match
//! rather than a look taken too early. Both policies in that leg are written the way AWS
//! documents them — the topic admits `events.amazonaws.com` only for the invoking RULE and
//! the queue admits `sns.amazonaws.com` only for the publishing TOPIC — so the delivery is
//! judged against conditions a client really writes.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared harness in
//! `vault_and_backup_job.rs`, and EventBridge, SQS and Amazon SNS are mounted beside Backup on
//! the one shared engine (`recovery_point_state_change_events.rs` owns that mounting), because
//! the publication reaches EventBridge through the in-process callback and a rule on that bus
//! delivers to a queue and to a topic.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_state_change_events::{
    delivered, delivered_until, given_queue, given_rule, of_type, sqs_in,
};
use crate::recovery_point_state_change_events::{element, emulator, notified_of_type, sns};
use crate::vault_and_backup_job::{
    call, create_vault, start_job, string_at, vault_arn, ACCOUNT, REGION,
};

/// The `source` and the `detail-type` values AWS Backup publishes under, spelled out here
/// rather than imported, so a wrong constant in the implementation cannot make a test pass.
/// Source: the EventBridge reference for AWS Backup, whose events carry
/// `"source": "aws.backup"` with `"detail-type"` of the kind of record that moved.
const EVENT_SOURCE: &str = "aws.backup";
const BACKUP_JOB_STATE_CHANGE: &str = "Backup Job State Change";
const COPY_JOB_STATE_CHANGE: &str = "Copy Job State Change";
const RECOVERY_POINT_STATE_CHANGE: &str = "Recovery Point State Change";

/// The terminal state a backup job, a copy job and the recovery point each of them wrote all
/// reach, as `BackupJobState`, `CopyJobState` and `RecoveryPointStatus` spell it.
const COMPLETED: &str = "COMPLETED";

/// The vault every case backs its resource up into, and the vault the copy writes into.
const SOURCE_VAULT: &str = "detail-member-source";
const DESTINATION_VAULT: &str = "detail-member-destination";

/// The rules the cases install: one matching the source alone, and the two whose patterns
/// differ ONLY in which member of a recovery point's detail they filter the status on.
const EVERY_EVENT_RULE: &str = "every-backup-event";
const STATUS_FILTERED_RULE: &str = "point-filtered-on-status";
const STATE_FILTERED_RULE: &str = "point-filtered-on-state";

/// The role every job here runs as, as this account's IAM holds it: AWS Backup assumes that
/// role before it mints a job, and the policy it carries allows the action that creates a
/// table's recovery point, so the job completes and writes that point. SOURCE: the
/// per-resource-type grants AWS publishes for the backup role (AWS Backup, *Managed
/// policies*) — the one action that creates a DynamoDB table's recovery point.
async fn given_role(router: &MultiServiceRouter, name: &str) -> String {
    crate::plan_and_selection_refusals::given_backup_job_role(
        router,
        "/service-role/",
        name,
        "dynamodb:CreateBackup",
    )
    .await
}

/// The protected resource a case backs up: a table of this account in this Region.
fn table_arn(name: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{name}")
}

/// One row of the table: the KIND of record whose move the envelope reports, the `detail`
/// member that carries the state it moved into, the other kind's spelling of that member —
/// which such an envelope must NOT carry — and how many envelopes of this kind the two starts
/// owe.
struct Row {
    case: &'static str,
    detail_type: &'static str,
    member: &'static str,
    absent: &'static str,
    owed: usize,
}

/// The rows: every kind of record one backup and one copy of its point move.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a backup job, whose model member is State",
            detail_type: BACKUP_JOB_STATE_CHANGE,
            member: "state",
            absent: "status",
            owed: 1,
        },
        Row {
            case: "a copy job, whose model member is State",
            detail_type: COPY_JOB_STATE_CHANGE,
            member: "state",
            absent: "status",
            owed: 1,
        },
        Row {
            case: "the recovery points, whose model member is Status",
            detail_type: RECOVERY_POINT_STATE_CHANGE,
            member: "status",
            absent: "state",
            // The backup wrote one, and the copy of it wrote another.
            owed: 2,
        },
    ]
}

/// Each published envelope names the state its own record moved into under the member that
/// record's own API reports it by, and carries no other spelling of it.
#[tokio::test]
async fn each_detail_names_the_state_member_the_moved_records_own_api_reports() {
    let case = "one backup, the copy of its point, and both points";
    let router = emulator();
    for vault in [SOURCE_VAULT, DESTINATION_VAULT] {
        let (status, created) = create_vault(&router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBackupVault {vault:?} answered {status} {created}"
        );
    }
    // One rule matching the source alone, so every envelope the two starts produce lands on
    // the same queue and the rows differ only in the record they are about.
    let (url, queue_arn) = given_queue(&router, case, "detail-member-target").await;
    given_rule(
        &router,
        case,
        EVERY_EVENT_RULE,
        &json!({ "source": [EVENT_SOURCE] }),
        &[queue_arn],
    )
    .await;

    let role = given_role(&router, "detail-member-role").await;
    let (status, started) = start_job(
        &router,
        &json!({
            "BackupVaultName": SOURCE_VAULT,
            "ResourceArn": table_arn("orders"),
            "IamRoleArn": &role,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBackupJob answered {status} {started}"
    );
    let recovery_point_arn = string_at(&started, "RecoveryPointArn");

    // The copy reads a point the backup has finished writing, and the move of that point is
    // published when it has: waiting for the envelope is therefore the wait for the point.
    let mut envelopes: Vec<Value> = Vec::new();
    delivered_until(
        &router,
        case,
        &url,
        &[
            (BACKUP_JOB_STATE_CHANGE, 1),
            (RECOVERY_POINT_STATE_CHANGE, 1),
        ],
        &mut envelopes,
    )
    .await;

    let (status, copy_started) = call(
        &router,
        "PUT",
        "/copy-jobs",
        Some(
            &json!({
                "RecoveryPointArn": recovery_point_arn,
                "SourceBackupVaultName": SOURCE_VAULT,
                "DestinationBackupVaultArn": vault_arn(REGION, DESTINATION_VAULT),
                "IamRoleArn": &role,
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartCopyJob answered {status} {copy_started}"
    );
    delivered_until(
        &router,
        case,
        &url,
        &[
            (COPY_JOB_STATE_CHANGE, 1),
            (RECOVERY_POINT_STATE_CHANGE, 2),
        ],
        &mut envelopes,
    )
    .await;

    for row in rows() {
        let case = row.case;
        let published = of_type(&envelopes, row.detail_type);
        assert_eq!(
            published.len(),
            row.owed,
            "case {case:?}: expected {} {:?} envelope(s), got {published:?}",
            row.owed,
            row.detail_type
        );
        for envelope in &published {
            let detail = &envelope["detail"];
            let read = read_state_of(&router, case, envelope).await;
            assert_eq!(
                detail[row.member], read,
                "case {case:?}: expected detail.{} to be the state the record's own read \
                 reports ({read}), got {detail}",
                row.member
            );
            assert_eq!(
                detail[row.member],
                json!(COMPLETED),
                "case {case:?}: expected the terminal state the record moved into under \
                 detail.{}, got {detail}",
                row.member
            );
            assert!(
                detail.get(row.absent).is_none(),
                "case {case:?}: expected no detail.{} on this kind of envelope, got {detail}",
                row.absent
            );
        }
    }
}

/// What a client that filters a recovery point's envelope on `detail.status` observes: the
/// envelope arrives at the subscriber of the SNS topic the matching rule targets, and the rule
/// filtering on `detail.state` instead receives nothing.
#[tokio::test]
async fn a_status_filtered_rules_topic_subscriber_holds_the_points_envelope() {
    let case = "a status-filtered rule with an SNS topic target";
    let router = emulator();
    let (status, created) = create_vault(&router, SOURCE_VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault answered {status} {created}"
    );

    // The topic the rule publishes to, admitting the service that delivers a target —
    // EventBridge publishes as its own service principal — and only for the rule whose
    // deliveries the grant is written for.
    let topic = sns(&router, "CreateTopic", &[("Name", "point-notifications")]).await;
    let topic_arn = element(&topic, "TopicArn");
    let rule_arn = format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/{STATUS_FILTERED_RULE}");
    let topic_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "SNS:Publish",
            "Resource": &topic_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": &rule_arn } },
        }],
    })
    .to_string();
    sns(
        &router,
        "SetTopicAttributes",
        &[
            ("TopicArn", &topic_arn),
            ("AttributeName", "Policy"),
            ("AttributeValue", &topic_policy),
        ],
    )
    .await;

    // The subscriber queue, admitting the service that delivers a subscription — Amazon SNS
    // makes its own `sqs:SendMessage` — and only for the topic it is subscribed to.
    let (subscriber_url, subscriber_arn) = given_subscriber_queue(&router, case, &topic_arn).await;
    sns(
        &router,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &subscriber_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;

    // The two rules: the same pattern but for the member it filters the point's status on.
    given_rule(
        &router,
        case,
        STATUS_FILTERED_RULE,
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [RECOVERY_POINT_STATE_CHANGE],
            "detail": { "status": [COMPLETED] },
        }),
        std::slice::from_ref(&topic_arn),
    )
    .await;
    let (state_url, state_arn) = given_queue(&router, case, "filtered-on-state").await;
    given_rule(
        &router,
        case,
        STATE_FILTERED_RULE,
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [RECOVERY_POINT_STATE_CHANGE],
            "detail": { "state": [COMPLETED] },
        }),
        &[state_arn],
    )
    .await;

    let resource = table_arn("continuously-backed-up");
    let role = given_role(&router, "detail-member-status-role").await;
    let (status, started) = start_job(
        &router,
        &json!({
            "BackupVaultName": SOURCE_VAULT,
            "ResourceArn": &resource,
            "IamRoleArn": &role,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBackupJob answered {status} {started}"
    );

    let (notification, envelope) =
        notified_of_type(&router, case, &subscriber_url, RECOVERY_POINT_STATE_CHANGE).await;
    assert_eq!(
        notification["TopicArn"],
        json!(topic_arn),
        "case {case:?}: expected the notification of the topic the rule targets, got \
         {notification}"
    );
    assert_eq!(
        envelope["source"],
        json!(EVENT_SOURCE),
        "case {case:?}: expected the source AWS Backup publishes under, got {envelope}"
    );
    let detail = &envelope["detail"];
    assert_eq!(
        detail["resourceArn"],
        json!(resource),
        "case {case:?}: expected the protected resource the job was given, got {envelope}"
    );
    assert_eq!(
        detail["status"],
        json!(COMPLETED),
        "case {case:?}: expected the status the point moved into, got {envelope}"
    );
    assert_eq!(
        detail["backupVaultArn"],
        json!(vault_arn(REGION, SOURCE_VAULT)),
        "case {case:?}: expected the ARN of the vault holding the point, got {envelope}"
    );

    // The delivery above is the positive control for this silence: the envelope WAS published
    // and routed, and the rule filtering on a member this detail does not carry is owed none
    // of it.
    let leaked = delivered(&router, case, &state_url).await;
    assert!(
        leaked.is_empty(),
        "case {case:?}: expected a rule filtering on detail.state to receive nothing for a \
         recovery point, got {leaked:?}"
    );
}

/// An SQS queue subscribed to `topic_arn` may receive that topic's deliveries and nothing
/// else: Amazon SNS makes the delivery as its own service principal and relies on the queue's
/// resource policy for it. Answers the queue's url and the ARN a subscription names it by.
async fn given_subscriber_queue(
    router: &MultiServiceRouter,
    case: &str,
    topic_arn: &str,
) -> (String, String) {
    let (status, created) = sqs_in(
        router,
        REGION,
        "CreateQueue",
        json!({ "QueueName": "topic-subscriber" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let (status, attributes) = sqs_in(
        router,
        REGION,
        "GetQueueAttributes",
        json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {attributes}"
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": &arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    })
    .to_string();
    let (status, set) = sqs_in(
        router,
        REGION,
        "SetQueueAttributes",
        json!({ "QueueUrl": &url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {set}"
    );
    (url, arn)
}

/// The state the record an `envelope` is about reports for ITSELF, read through that record's
/// own control-plane operation: a backup job's `State`, a copy job's `State`, a recovery
/// point's `Status`.
async fn read_state_of(router: &MultiServiceRouter, case: &str, envelope: &Value) -> Value {
    let detail = &envelope["detail"];
    let member_of = |member: &str| {
        detail[member]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no detail.{member} in {envelope}"))
            .to_string()
    };
    match envelope["detail-type"].as_str().unwrap_or_default() {
        BACKUP_JOB_STATE_CHANGE => {
            let path = format!("/backup-jobs/{}", member_of("backupJobId"));
            read_member(router, case, &path, &["State"]).await
        }
        COPY_JOB_STATE_CHANGE => {
            let path = format!("/copy-jobs/{}", member_of("copyJobId"));
            read_member(router, case, &path, &["CopyJob", "State"]).await
        }
        RECOVERY_POINT_STATE_CHANGE => {
            let path = format!(
                "/backup-vaults/{}/recovery-points/{}",
                member_of("backupVaultName"),
                member_of("recoveryPointArn")
            );
            read_member(router, case, &path, &["Status"]).await
        }
        other => panic!("case {case:?}: unexpected detail-type {other:?} in {envelope}"),
    }
}

/// `GET path` on this service, answering the value at `member` of the body it reports.
async fn read_member(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    member: &[&str],
) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GET {path} answered {status} {body}"
    );
    let mut read = &body;
    for name in member {
        read = &read[*name];
    }
    read.clone()
}
