// SPDX-License-Identifier: CC0-1.0

//! AWS Backup announces a RECOVERY POINT's state changes on the account's default
//! EventBridge bus, beside the job's own.
//!
//! The RULE under test: a recovery point is a resource of its own, with a status of its
//! own, so every status it MOVES INTO after the call that created it is published once onto
//! the account's default event bus as `source` `aws.backup` with `detail-type`
//! `Recovery Point State Change`, carrying that point's own stored members as the event
//! `detail` — the status it moved into, under the member name `DescribeRecoveryPoint`
//! reports it by, its ARN, the vault holding it, the protected
//! resource and that resource's Backup type — and the point itself in `resources`.
//! EventBridge then routes it by CONTENT: a rule whose `EventPattern` matches hands the
//! envelope UNCHANGED to every one of that rule's targets, and a rule whose pattern cannot
//! match receives nothing. The status the CREATE call stored is not a move — it is what
//! that call wrote — so the envelope the bus is owed is the transition that follows it, and
//! it is owed to the clock rather than to a reader: nobody has to poll the point for it to
//! arrive.
//!
//! The first table spans that rule along what it generalises over: WHICH dimension of the
//! envelope the rule's pattern filters on (the source alone, the source plus this
//! detail-type, a status inside `detail`, the vault's ARN inside `detail`, the point's own
//! ARN in `resources`) and therefore whether that rule is owed the envelope at all — the
//! rows that must receive NOTHING carry a positive control on the same bus, so what says
//! the envelope was published is the control's own delivery rather than a look taken too
//! early.
//!
//! The published `detail` is checked against what `DescribeRecoveryPoint` reports for the
//! same point, so the event is the point's own stored state and not a second, invented
//! copy of it.
//!
//! The last test pins the same routing for a job that FAILS: the terminal state of a job
//! whose backup role cannot create the recovery point is published like any other move, so
//! a rule whose pattern is scoped to `detail.state` of exactly that state receives it —
//! and the delivery holds for a target queue whose policy grants this service's principal
//! only for the invoking RULE (`aws:SourceArn`), which is how such a grant is written.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared harness in
//! `vault_and_backup_job.rs`, EventBridge and SQS are mounted beside Backup on the one
//! shared engine (the publication reaches EventBridge through the in-process callback and a
//! rule on that bus delivers to an SQS queue target), and IAM is mounted too, because what
//! a job's own role may do is put to IAM and the documents it decides from are the ones
//! `CreateRole` wrote through IAM's own wire.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_state_change_events::{
    delivered, delivered_of_type, given_queue, given_rule, of_type, sqs_in,
    BACKUP_JOB_STATE_CHANGE, COMPLETED, EVENT_SOURCE,
};
use crate::plan_and_selection_refusals::{create_role, form_value};
use crate::vault_and_backup_job::{call, create_vault, start_job, string_at, ACCOUNT, REGION};

/// The `detail-type` AWS Backup publishes a recovery point's state changes under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass. Source: the EventBridge reference for AWS Backup, whose recovery point events
/// carry `"source": "aws.backup"` with `"detail-type": "Recovery Point State Change"`.
const RECOVERY_POINT_STATE_CHANGE: &str = "Recovery Point State Change";

/// The terminal `RecoveryPointStatus` a point written by a backup reaches, and one the
/// model declares that a point of this suite never takes
/// (`smithy_query(query="shape", shape="com.amazonaws.backup#RecoveryPointStatus")`:
/// `COMPLETED` | `PARTIAL` | `DELETING` | `EXPIRED` | `AVAILABLE` | `STOPPED` |
/// `CREATING`).
const EXPIRED: &str = "EXPIRED";

/// The terminal `BackupJobState` a job whose role cannot create the recovery point reaches
/// (model enum `BackupJobState`).
const FAILED: &str = "FAILED";

/// The vault every case backs its resource up into.
const VAULT: &str = "recovery-point-events";

/// The rule each row installs its pattern under, and the rule of the positive control.
const ROW_RULE: &str = "recovery-point-row";
const CONTROL_RULE: &str = "recovery-point-control";

/// The service principal a backup role has to admit for AWS Backup to assume it.
/// Source: AWS Backup's service principal, as the trust policy of its service role
/// carries it.
const SERVICE_PRINCIPAL: &str = "backup.amazonaws.com";

/// Backup, EventBridge, SQS and IAM on ONE shared engine, each claiming only requests
/// scoped to its own signing name.
///
/// `pub(crate)` for the sibling suite that pins which MEMBER of a published `detail`
/// carries the state a record moved into (`state_change_detail_member_names.rs`), so the
/// services mounted beside Backup are one list rather than two that can drift apart.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One Amazon SNS call, made the way a client makes one: a form-encoded `awsQuery` POST
/// signed for that service in [`REGION`]. Answers the XML body, asserted to have been a
/// success.
///
/// `pub(crate)` for the sibling suite named above, which stands a topic up the same way.
pub(crate) async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in members {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8_lossy(response.into_body().as_ref()).to_string();
    assert_eq!(status, 200, "{action}: answered {status} {xml}");
    xml
}

/// The text of the first `<tag>` of an `awsQuery` answer.
pub(crate) fn element(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml
        .find(&open)
        .unwrap_or_else(|| panic!("expected {open} in {xml}"))
        + open.len();
    let end = xml[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("expected {close} in {xml}"));
    xml[start..start + end].to_string()
}

/// The ARN of a backup vault of this account in [`REGION`], as the model documents the
/// form ("for example, `arn:aws:backup:us-east-1:123456789012:backup-vault:aBackupVault`").
fn vault_arn(name: &str) -> String {
    format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{name}")
}

/// The protected resource a case backs up: a table of this account in this Region, named
/// per case so one case's recovery points are never another's.
fn table_arn(name: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{name}")
}

/// The role the backups of this suite are performed under, as this account's IAM holds it:
/// AWS Backup assumes that role before it mints a job, and the policy it carries allows the
/// action that creates a table's recovery point, so the job completes and writes its point.
/// SOURCE: the per-resource-type grants AWS publishes for the backup role (AWS Backup,
/// *Managed policies*) — the one action that creates a DynamoDB table's recovery point.
async fn completing_role(router: &MultiServiceRouter, name: &str) -> String {
    crate::plan_and_selection_refusals::given_backup_job_role(
        router,
        "/service-role/",
        name,
        "dynamodb:CreateBackup",
    )
    .await
}

/// Start a backup of `resource` under `role` into [`VAULT`], and answer the
/// `RecoveryPointArn` the call reported.
async fn started_point(
    router: &MultiServiceRouter,
    case: &str,
    resource: &str,
    role: &str,
) -> String {
    let (status, started) = start_job(
        router,
        &json!({
            "BackupVaultName": VAULT,
            "ResourceArn": resource,
            "IamRoleArn": role,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBackupJob answered {status} {started}"
    );
    string_at(&started, "RecoveryPointArn")
}

/// `GET /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}`, as the model
/// binds `DescribeRecoveryPoint`.
async fn describe_recovery_point(
    router: &MultiServiceRouter,
    case: &str,
    point_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/backup-vaults/{VAULT}/recovery-points/{point_arn}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRecoveryPoint answered {status} {body}"
    );
    body
}

/// One row of the first table: a rule pattern and whether a rule carrying it is owed the
/// recovery point's envelope.
struct Row {
    case: &'static str,
    /// Built from the vault's and the point's own values, so the pattern names what the
    /// request itself produced; `point` is the `RecoveryPointArn` the start reported.
    pattern: fn(&str) -> Value,
    delivered: bool,
}

/// The rows: the pattern forms a client writes for this event, each with whether the
/// recovery point's own transition matches it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source-only pattern",
            pattern: |_point| json!({ "source": [EVENT_SOURCE] }),
            delivered: true,
        },
        Row {
            case: "a pattern naming this detail-type",
            pattern: |_point| json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RECOVERY_POINT_STATE_CHANGE],
            }),
            delivered: true,
        },
        Row {
            case: "a pattern scoped to the status the point moves into",
            pattern: |_point| json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RECOVERY_POINT_STATE_CHANGE],
                "detail": { "status": [COMPLETED] },
            }),
            delivered: true,
        },
        Row {
            case: "a pattern scoped to the ARN of the vault holding the point",
            pattern: |_point| json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RECOVERY_POINT_STATE_CHANGE],
                "detail": { "backupVaultArn": [vault_arn(VAULT)] },
            }),
            delivered: true,
        },
        Row {
            case: "a pattern scoped to the point's own ARN in resources",
            pattern: |point| json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RECOVERY_POINT_STATE_CHANGE],
                "resources": [point],
            }),
            delivered: true,
        },
        Row {
            case: "a pattern scoped to a status this point never takes",
            pattern: |_point| json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RECOVERY_POINT_STATE_CHANGE],
                "detail": { "status": [EXPIRED] },
            }),
            delivered: false,
        },
        Row {
            case: "a pattern scoped to another vault's ARN",
            pattern: |_point| json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RECOVERY_POINT_STATE_CHANGE],
                "detail": { "backupVaultArn": [vault_arn("another-vault")] },
            }),
            delivered: false,
        },
    ]
}

/// A recovery point's transition reaches every target of every rule whose pattern matches
/// it, carrying the point's own stored members — and reaches no target of a rule whose
/// pattern does not.
#[tokio::test]
async fn a_settled_recovery_point_reaches_the_targets_of_the_rules_that_match_it() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let (status, created) = create_vault(&router, VAULT, "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBackupVault answered {status} {created}"
        );
        // The point has to exist before the rule can be scoped to its ARN, and the
        // envelope is owed by the CLOCK rather than by this call, so the rules below are
        // installed and then waited on.
        let resource = table_arn("orders");
        let role = completing_role(&router, "state-change-row-role").await;
        let point = started_point(&router, case, &resource, &role).await;

        let (row_url, row_arn) = given_queue(&router, case, "row-target").await;
        given_rule(
            &router,
            case,
            ROW_RULE,
            &(row.pattern)(&point),
            &[row_arn],
        )
        .await;
        // The positive control: a rule on the same bus matching the source alone, whose
        // own delivery is what says the envelope really was published.
        let (control_url, control_arn) = given_queue(&router, case, "control-target").await;
        given_rule(
            &router,
            case,
            CONTROL_RULE,
            &json!({ "source": [EVENT_SOURCE] }),
            &[control_arn],
        )
        .await;

        // The control is owed BOTH envelopes the one start produces: the job's own move
        // and the move of the point it wrote.
        let control = delivered_of_type(
            &router,
            case,
            &control_url,
            RECOVERY_POINT_STATE_CHANGE,
            1,
        )
        .await;
        let control_points = of_type(&control, RECOVERY_POINT_STATE_CHANGE);
        assert_eq!(
            control_points.len(),
            1,
            "case {case:?}: expected exactly one {RECOVERY_POINT_STATE_CHANGE} envelope on \
             the control queue, got {control:?}"
        );
        assert_eq!(
            of_type(&control, BACKUP_JOB_STATE_CHANGE).len(),
            1,
            "case {case:?}: expected the job's own envelope beside the point's, got \
             {control:?}"
        );

        if !row.delivered {
            let leaked = delivered(&router, case, &row_url).await;
            assert!(
                leaked.is_empty(),
                "case {case:?}: expected a pattern that cannot match to receive nothing, \
                 got {leaked:?}"
            );
            continue;
        }

        let envelopes =
            delivered_of_type(&router, case, &row_url, RECOVERY_POINT_STATE_CHANGE, 1).await;
        let published = of_type(&envelopes, RECOVERY_POINT_STATE_CHANGE);
        let Some(envelope) = published.first() else {
            panic!(
                "case {case:?}: expected a {RECOVERY_POINT_STATE_CHANGE} envelope, got \
                 {envelopes:?}"
            );
        };
        assert_eq!(
            envelope["source"],
            json!(EVENT_SOURCE),
            "case {case:?}: expected the source AWS Backup publishes under, got {envelope}"
        );
        assert_eq!(
            envelope["resources"],
            json!([point.clone()]),
            "case {case:?}: expected the point itself in resources, got {envelope}"
        );
        // The detail IS the stored point: every member it carries is the one a read of the
        // same point reports for it.
        let read = describe_recovery_point(&router, case, &point).await;
        let detail = &envelope["detail"];
        for (member, read_member) in [
            ("recoveryPointArn", "RecoveryPointArn"),
            ("status", "Status"),
            ("backupVaultName", "BackupVaultName"),
            ("backupVaultArn", "BackupVaultArn"),
            ("resourceArn", "ResourceArn"),
            ("resourceType", "ResourceType"),
            ("iamRoleArn", "IamRoleArn"),
        ] {
            assert_eq!(
                detail[member],
                read[read_member],
                "case {case:?}: expected detail.{member} to be the point's own \
                 {read_member}, got {detail} against {read}"
            );
        }
        assert_eq!(
            detail["status"],
            json!(COMPLETED),
            "case {case:?}: expected the status the point moved into, got {detail}"
        );
    }
}

/// The SAME envelope reaches a subscriber through an SNS TOPIC target: EventBridge
/// publishes it to the topic, and the topic hands it to the queue subscribed to it, so a
/// client that fans backup notifications out through a topic reads the envelope unchanged
/// inside the notification.
#[tokio::test]
async fn a_recovery_point_envelope_reaches_a_queue_subscribed_to_a_topic_target() {
    let case = "a topic target with a subscribed queue";
    let router = emulator();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault answered {status} {created}"
    );

    // The subscriber queue, which admits the SERVICE that delivers to it: a subscription's
    // delivery is Amazon SNS's own `sqs:SendMessage`, so the grant names that principal
    // rather than the one an EventBridge target carries.
    let (status, queue) = sqs_in(
        &router,
        REGION,
        "CreateQueue",
        json!({ "QueueName": "topic-subscriber" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {queue}"
    );
    let queue_url = queue["QueueUrl"].as_str().unwrap_or_default().to_string();
    let (status, attributes) = sqs_in(
        &router,
        REGION,
        "GetQueueAttributes",
        json!({ "QueueUrl": &queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {attributes}"
    );
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let queue_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": &queue_arn,
        }],
    })
    .to_string();
    let (status, set) = sqs_in(
        &router,
        REGION,
        "SetQueueAttributes",
        json!({ "QueueUrl": &queue_url, "Attributes": { "Policy": queue_policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {set}"
    );

    // The topic the rule delivers to, which admits the service that publishes to it, with
    // the queue subscribed over the `sqs` protocol.
    let created = sns(&router, "CreateTopic", &[("Name", "backup-notifications")]).await;
    let topic_arn = element(&created, "TopicArn");
    let topic_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "SNS:Publish",
            "Resource": &topic_arn,
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
    sns(
        &router,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;

    given_rule(
        &router,
        case,
        ROW_RULE,
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [RECOVERY_POINT_STATE_CHANGE],
        }),
        &[topic_arn],
    )
    .await;

    let role = completing_role(&router, "state-change-fan-out-role").await;
    let point = started_point(&router, case, &table_arn("fanned-out"), &role).await;
    // What the subscriber receives is the NOTIFICATION the topic delivers, whose `Message`
    // is the envelope as it was published.
    let (_notification, envelope) =
        notified_of_type(&router, case, &queue_url, RECOVERY_POINT_STATE_CHANGE).await;
    assert_eq!(
        envelope["source"],
        json!(EVENT_SOURCE),
        "case {case:?}: expected the source AWS Backup publishes under, got {envelope}"
    );
    assert_eq!(
        envelope["detail"]["recoveryPointArn"],
        json!(point),
        "case {case:?}: expected the point this job wrote, got {envelope}"
    );
    assert_eq!(
        envelope["detail"]["status"],
        json!(COMPLETED),
        "case {case:?}: expected the status the point moved into, got {envelope}"
    );
}

/// Poll the queue at `url` until a notification carrying an envelope of `detail_type` is
/// there, and answer that notification together with the envelope inside it.
///
/// The body of a message a topic delivers is the NOTIFICATION, whose `Message` member is
/// the text that was published to the topic — the envelope — so it is parsed out of it.
///
/// `pub(crate)` for the sibling suite that reads a notification's own members beside the
/// envelope (`state_change_detail_member_names.rs`), so this crate has ONE notification
/// poll rather than two that can drift apart.
pub(crate) async fn notified_of_type(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    detail_type: &str,
) -> (Value, Value) {
    let deadline = ::std::time::Instant::now() + POLL_DEADLINE;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        for notification in delivered(router, case, url).await {
            let envelope = notification["Message"]
                .as_str()
                .and_then(|message| serde_json::from_str::<Value>(message).ok());
            match envelope {
                Some(envelope) if envelope["detail-type"] == json!(detail_type) => {
                    return (notification, envelope)
                }
                _ => seen.push(notification),
            }
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {case:?}: expected a {detail_type:?} envelope inside a notification on \
             {url:?} within {POLL_DEADLINE:?}, got {seen:?}"
        );
        // Hand the runtime back, so the in-process clock pump is polled between attempts.
        ::tokio::task::yield_now().await;
    }
}

/// How long the notification poll above looks for an envelope the CLOCK owes.
///
/// Generous on purpose: what it waits for falls due on the emulator's own clock, so the
/// case waits for the ENVELOPE and this deadline only bounds one that never arrives at all.
const POLL_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// The terminal state of a job whose role cannot create the recovery point is published
/// like any other move, so a rule scoped to exactly that state receives it — and a target
/// queue that grants this service only for the invoking rule is delivered to.
#[tokio::test]
async fn a_failed_jobs_state_reaches_a_rule_scoped_to_that_state() {
    let case = "a role carrying no policy at all";
    let router = emulator();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault answered {status} {created}"
    );
    // A role AWS Backup may assume that carries NO permissions policy: the backup it would
    // perform creates nothing, so the job's terminal state is FAILED.
    let role = create_role(
        &router,
        "/",
        "powerless-backup-role",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string(),
    )
    .await;

    let (url, queue_arn) = given_queue(&router, case, "failed-state-target").await;
    // The grant written for a rule's target, scoped by the ARN of the rule whose
    // deliveries it authorizes — the shape AWS documents for an EventBridge target queue.
    let rule_arn = format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/{ROW_RULE}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": &queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": &rule_arn } },
        }],
    })
    .to_string();
    let (status, set) = sqs_in(
        &router,
        REGION,
        "SetQueueAttributes",
        json!({ "QueueUrl": &url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {set}"
    );
    given_rule(
        &router,
        case,
        ROW_RULE,
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [BACKUP_JOB_STATE_CHANGE],
            "detail": { "state": [FAILED] },
        }),
        &[queue_arn],
    )
    .await;

    let point = started_point(&router, case, &table_arn("ledger"), &role).await;
    let envelopes = delivered_of_type(&router, case, &url, BACKUP_JOB_STATE_CHANGE, 1).await;
    let published = of_type(&envelopes, BACKUP_JOB_STATE_CHANGE);
    let Some(envelope) = published.first() else {
        panic!("case {case:?}: expected the FAILED envelope, got {envelopes:?}");
    };
    assert_eq!(
        envelope["detail"]["state"],
        json!(FAILED),
        "case {case:?}: expected a rule scoped to {FAILED} to receive only that state, got \
         {envelope}"
    );
    assert_eq!(
        envelope["detail"]["recoveryPointArn"],
        json!(point),
        "case {case:?}: expected the job's own recovery point ARN, got {envelope}"
    );
    assert!(
        envelope["detail"]["statusMessage"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected the failure's own status message, got {envelope}"
    );
}
