// SPDX-License-Identifier: CC0-1.0

//! AWS Backup announces a job's state changes on the account's default EventBridge bus.
//!
//! The RULE under test: every state a stored job MOVES INTO after the call that started it
//! is published once onto the account's default event bus as `source` `aws.backup` with the
//! `detail-type` of the KIND of job that moved — `Backup Job State Change` for a backup
//! job, `Copy Job State Change` for a copy job — carrying that job's own members as the
//! event `detail` (its id, the state it moved into, the vaults and recovery points it names)
//! and the recovery point it wrote in `resources`. EventBridge then routes it: a rule whose
//! `EventPattern` matches delivers the envelope UNCHANGED to EVERY one of that rule's
//! targets, so one rule whose pattern lists both detail-types receives both kinds of
//! envelope on the same target, and a rule whose pattern cannot match receives nothing.
//!
//! The tests assert that rule over a TABLE of rule patterns, which differ along what the
//! rule generalises over — which clauses the pattern filters on (`source` alone, `source`
//! plus ONE detail-type, `source` plus a LIST of both detail-types) and therefore which of
//! the two envelopes that rule is owed — and, independently, over where the copy's
//! destination vault lives: this request's Region and another Region, because a copy
//! reports the destination vault's own ARN whichever Region that vault belongs to.
//!
//! Every published `detail` is checked against what `DescribeBackupJob` /
//! `DescribeCopyJob` report for the same job, so the event is the job's own stored state
//! and not a second, invented copy of it; and the state the START call stored is asserted
//! NOT to be published, because it is what that call's own response reports rather than a
//! move the bus is owed.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup`, an `events` and an `sqs` client see together. The Backup request
//! harness is the shared one in `vault_and_backup_job.rs`; EventBridge and SQS are mounted
//! beside Backup on the one shared engine, because the publication reaches EventBridge
//! through the in-process callback and a rule on that bus delivers to an SQS queue target.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{
    call, create_vault, start_job, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// How long a poll loop looks for something the CLOCK owes before failing the case.
///
/// Generous on purpose: what the loop waits for falls due on the emulator's own clock,
/// so the case waits for the CONDITION and this deadline only bounds a transition that
/// is never published at all — a loaded or a slow host still sees one that is.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// The `source` and the two `detail-type` values AWS Backup publishes its job state
/// changes under, spelled out here rather than imported, so a wrong constant in the
/// implementation cannot make a test pass. Source: the EventBridge reference for AWS
/// Backup, whose job state change events carry `"source": "aws.backup"` with
/// `"detail-type": "Backup Job State Change"` and `"detail-type": "Copy Job State Change"`.
pub(crate) const EVENT_SOURCE: &str = "aws.backup";
pub(crate) const BACKUP_JOB_STATE_CHANGE: &str = "Backup Job State Change";
const COPY_JOB_STATE_CHANGE: &str = "Copy Job State Change";

/// The terminal state both job kinds reach, as the model's `BackupJobState` and
/// `CopyJobState` spell it.
pub(crate) const COMPLETED: &str = "COMPLETED";

/// The source vault every case backs a resource up into, and the destination vault every
/// case copies into.
const SOURCE_VAULT: &str = "events-source";
const DESTINATION_VAULT: &str = "events-destination";

/// The Region the cross-Region copy cases put their destination vault in. A
/// vault is identified by its name within one Region, so the destination vault
/// of such a copy is created through a client bound to this Region.
const COPY_DESTINATION_REGION: &str = "us-east-1";

/// The name of the rule every case's pattern is installed under, and of the rule whose
/// pattern no AWS Backup event can match.
const MATCHING_RULE: &str = "backup-job-events";
const OTHER_RULE: &str = "unrelated-events";

/// One emulator: Backup, EventBridge and SQS mounted on ONE shared engine, each claiming
/// only requests scoped to its own signing name, so no Backup request is routed any
/// differently for their presence.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
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
        engine,
    ));
    router
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name`, the way
/// a client makes it: `POST /` with `X-Amz-Target: <prefix>.<Operation>` and a credential
/// scoped to that service in `region`.
async fn rpc_in(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
        })
    };
    (status, decoded)
}

/// One EventBridge operation (`awsJson1_1`, target prefix `AWSEvents`) in `region`.
async fn events_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    rpc_in(router, region, "events", "AWSEvents", "1.1", operation, body).await
}

/// One SQS operation (`awsJson1_0`, target prefix `AmazonSQS`) in `region`.
pub(crate) async fn sqs_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    rpc_in(router, region, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// An SQS queue called `name` in `region`, carrying the grant a rule's target needs —
/// EventBridge makes the target's `sqs:SendMessage` as its own service principal and relies
/// on the queue's resource policy for it, so a queue that has granted it nothing receives
/// nothing. Answers the queue's url and the ARN a rule names it by.
async fn given_queue_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
) -> (String, String) {
    let (status, created) = sqs_in(router, region, "CreateQueue", json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue {name:?} answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let (status, attributes) = sqs_in(
        router,
        region,
        "GetQueueAttributes",
        json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
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
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    let (status, set) = sqs_in(
        router,
        region,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {set}"
    );
    (url, arn)
}

/// [`given_queue_in`] in [`REGION`].
pub(crate) async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    given_queue_in(router, REGION, case, name).await
}

/// Install an ENABLED rule called `name` whose `EventPattern` is `pattern` on `region`'s
/// default bus, with every ARN in `targets` as a target of it.
async fn given_rule_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
    pattern: &Value,
    targets: &[String],
) {
    let (status, put) = events_in(
        router,
        region,
        "PutRule",
        json!({ "Name": name, "State": "ENABLED", "EventPattern": pattern.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule {name:?} answered {status} {put}"
    );
    let entries: Vec<Value> = targets
        .iter()
        .enumerate()
        .map(|(index, arn)| json!({ "Id": format!("target-{index}"), "Arn": arn }))
        .collect();
    let (status, put) = events_in(
        router,
        region,
        "PutTargets",
        json!({ "Rule": name, "Targets": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets for {name:?} answered {status} {put}"
    );
}

/// [`given_rule_in`] in [`REGION`].
pub(crate) async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    pattern: &Value,
    targets: &[String],
) {
    given_rule_in(router, REGION, case, name, pattern, targets).await
}

/// Every message waiting on the queue at `url` in `region`, as parsed JSON bodies, oldest
/// first.
///
/// Drained in several receives because one `ReceiveMessage` answers at most ten messages,
/// and a received message is invisible afterwards, so the loop sees each message once. The
/// loop is bounded: it stops on the first empty receive, and after at most
/// [`MAX_RECEIVES`] of them whatever the queue holds.
async fn delivered_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    url: &str,
) -> Vec<Value> {
    /// How many `ReceiveMessage` calls one drain makes at most.
    const MAX_RECEIVES: usize = 4;
    let mut bodies = Vec::new();
    for _ in 0..MAX_RECEIVES {
        let (status, received) = sqs_in(
            router,
            region,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ReceiveMessage on {url:?} answered {status} {received}"
        );
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if messages.is_empty() {
            break;
        }
        for message in messages {
            let raw = message["Body"].as_str().unwrap_or_default();
            bodies.push(serde_json::from_str::<Value>(raw).unwrap_or(Value::Null));
        }
    }
    bodies
}

/// [`delivered_in`] in [`REGION`].
pub(crate) async fn delivered(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    delivered_in(router, REGION, case, url).await
}

/// Poll the queue at `url` in `region` until it has handed over at least `wanted`
/// envelopes, or until the deadline passes.
///
/// Each attempt awaits, which is what lets the in-process clock pump make progress beside
/// this test, and the loop waits for the ENVELOPES rather than for a fixed span: a
/// transition the clock has not published yet is looked for again, and one that is never
/// published fails the case at [`POLL_DEADLINE`] rather than hanging it. A drain takes the
/// messages it reports off the queue, so what each attempt collects is kept.
async fn delivered_at_least_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut bodies = Vec::new();
    loop {
        bodies.extend(delivered_in(router, region, case, url).await);
        if bodies.len() >= wanted {
            return bodies;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {wanted} envelope(s) on {url:?} within \
             {POLL_DEADLINE:?}, got {bodies:?}"
        );
        // Hand the runtime back, so the in-process clock pump is polled between
        // attempts rather than only when this task happens to block.
        ::tokio::task::yield_now().await;
    }
}

/// [`delivered_at_least_in`] in [`REGION`].
async fn delivered_at_least(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    delivered_at_least_in(router, REGION, case, url, wanted).await
}

/// The backup job `job_id` once it has reached its terminal state, looked for again until
/// it has or until [`POLL_DEADLINE`] passes: the transition falls due on the emulator's
/// clock, so the case waits for the STATE rather than for a span.
async fn settled_backup_job(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let described = describe_backup_job(router, job_id).await;
        if described.get("State").and_then(Value::as_str) == Some(COMPLETED) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the backup job to settle within {POLL_DEADLINE:?}, \
             got {described}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// The copy job `copy_job_id` once it has reached its terminal state, looked for again
/// until it has or until [`POLL_DEADLINE`] passes — the same wait
/// [`settled_backup_job`] makes, for the other job kind.
async fn settled_copy_job(router: &MultiServiceRouter, case: &str, copy_job_id: &str) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let described = describe_copy_job(router, copy_job_id).await;
        if described.get("State").and_then(Value::as_str) == Some(COMPLETED) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the copy job to settle within {POLL_DEADLINE:?}, \
             got {described}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// The ARN of a backup vault of this account in `region`, as the model documents the form
/// ("for example, `arn:aws:backup:us-east-1:123456789012:backup-vault:aBackupVault`").
pub(crate) fn vault_arn(region: &str, name: &str) -> String {
    format!("arn:aws:backup:{region}:{ACCOUNT}:backup-vault:{name}")
}

/// The role a job is run with.
pub(crate) fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// One Backup request made by a client in `region`, the way the shared harness makes one in
/// [`REGION`]: the same `restJson1` route, with the credential scope naming `region` — which
/// is the Region every ARN that request mints carries.
async fn backup_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/backup/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        );
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |raw| {
        bytes::Bytes::copy_from_slice(raw.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `GET /backup-jobs/{BackupJobId}`, as the model binds `DescribeBackupJob`.
pub(crate) async fn describe_backup_job(router: &MultiServiceRouter, job_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeBackupJob must answer 200 for a stored job, got {status} {body}"
    );
    body
}

/// `GET /copy-jobs/{CopyJobId}`, as the model binds `DescribeCopyJob`; answers the `CopyJob`
/// body.
async fn describe_copy_job(router: &MultiServiceRouter, copy_job_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/copy-jobs/{copy_job_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeCopyJob must answer 200 for a stored job, got {status} {body}"
    );
    body.get("CopyJob").cloned().unwrap_or(Value::Null)
}

/// The envelopes of `detail_type` among `delivered`.
pub(crate) fn of_type(delivered: &[Value], detail_type: &str) -> Vec<Value> {
    delivered
        .iter()
        .filter(|event| event["detail-type"] == json!(detail_type))
        .cloned()
        .collect()
}

/// One row of the table: a rule pattern, the detail-types that pattern is owed, and the
/// Region the copy's destination vault ARN names.
struct Row {
    case: &'static str,
    pattern: Value,
    expected: &'static [&'static str],
    destination_region: &'static str,
}

/// The rows: the pattern forms a client writes for these events, and the two placements of
/// the destination vault a copy can have.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source-only pattern, destination vault in this Region",
            pattern: json!({ "source": [EVENT_SOURCE] }),
            expected: &[BACKUP_JOB_STATE_CHANGE, COPY_JOB_STATE_CHANGE],
            destination_region: REGION,
        },
        Row {
            case: "a pattern naming the copy-job detail-type, destination vault elsewhere",
            pattern: json!({ "source": [EVENT_SOURCE], "detail-type": [COPY_JOB_STATE_CHANGE] }),
            expected: &[COPY_JOB_STATE_CHANGE],
            destination_region: "us-east-1",
        },
        Row {
            case: "a pattern listing both job detail-types, destination vault elsewhere",
            pattern: json!({
                "source": [EVENT_SOURCE],
                "detail-type": [BACKUP_JOB_STATE_CHANGE, COPY_JOB_STATE_CHANGE],
            }),
            expected: &[BACKUP_JOB_STATE_CHANGE, COPY_JOB_STATE_CHANGE],
            destination_region: "eu-west-1",
        },
        Row {
            case: "a pattern naming the backup-job detail-type only",
            pattern: json!({ "source": [EVENT_SOURCE], "detail-type": [BACKUP_JOB_STATE_CHANGE] }),
            expected: &[BACKUP_JOB_STATE_CHANGE],
            destination_region: REGION,
        },
    ]
}

/// A job's terminal transition reaches every target of every rule whose pattern matches it,
/// carrying the job's own stored members — and reaches no target of a rule whose pattern
/// does not.
#[tokio::test]
async fn a_settled_job_reaches_every_target_of_every_rule_whose_pattern_matches() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        // A vault belongs to the Region it was created in, so each vault is
        // created through a client in its own Region: the destination vault of
        // a cross-Region copy has to exist in the Region its ARN names.
        let (status, created) = create_vault(&router, SOURCE_VAULT, "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBackupVault {SOURCE_VAULT:?} answered {status} {created}"
        );
        let (status, created) = backup_in(
            &router,
            row.destination_region,
            "PUT",
            &format!("/backup-vaults/{DESTINATION_VAULT}"),
            Some("{}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBackupVault {DESTINATION_VAULT:?} in {} answered \
             {status} {created}",
            row.destination_region
        );

        // ONE rule with TWO targets: a matched envelope is delivered to both of them.
        let (first_url, first_arn) = given_queue(&router, case, "matched-one").await;
        let (second_url, second_arn) = given_queue(&router, case, "matched-two").await;
        let (other_url, other_arn) = given_queue(&router, case, "unmatched").await;
        given_rule(
            &router,
            case,
            MATCHING_RULE,
            &row.pattern,
            &[first_arn, second_arn],
        )
        .await;
        // Another service's events: nothing AWS Backup publishes can match this.
        given_rule(
            &router,
            case,
            OTHER_RULE,
            &json!({ "source": ["aws.s3"], "detail-type": ["Object Created"] }),
            &[other_arn],
        )
        .await;

        // The backup job: started, then settled by the clock and reported COMPLETED.
        let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
        let (status, started) = start_job(
            &router,
            &json!({
                "BackupVaultName": SOURCE_VAULT,
                "ResourceArn": resource_arn,
                "IamRoleArn": role(),
            })
            .to_string(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartBackupJob answered {status} {started}"
        );
        let backup_job_id = string_at(&started, "BackupJobId");
        let recovery_point_arn = string_at(&started, "RecoveryPointArn");
        // The settle falls due on the clock, so the job is looked at again until it has
        // reached its terminal state rather than after a fixed span.
        let described = settled_backup_job(&router, case, &backup_job_id).await;
        assert_eq!(
            described.get("State").and_then(Value::as_str),
            Some(COMPLETED),
            "case {case:?}: the backup job must have settled, got {described}"
        );

        // The copy job: started from the settled recovery point into the destination
        // vault, then settled and reported COMPLETED.
        let destination_arn = vault_arn(row.destination_region, DESTINATION_VAULT);
        let (status, copy_started) = call(
            &router,
            "PUT",
            "/copy-jobs",
            Some(
                &json!({
                    "RecoveryPointArn": recovery_point_arn,
                    "SourceBackupVaultName": SOURCE_VAULT,
                    "DestinationBackupVaultArn": destination_arn,
                    "IamRoleArn": role(),
                })
                .to_string(),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartCopyJob answered {status} {copy_started}"
        );
        let copy_job_id = string_at(&copy_started, "CopyJobId");
        let copy_job = settled_copy_job(&router, case, &copy_job_id).await;
        assert_eq!(
            copy_job.get("State").and_then(Value::as_str),
            Some(COMPLETED),
            "case {case:?}: the copy job must have settled, got {copy_job}"
        );

        // What each target received. Both targets of the matching rule are handed the
        // SAME envelopes, and the rule that cannot match is handed none. Each queue is
        // looked at again until it holds the envelopes the matched transitions owe it,
        // so the drain waits for the publication rather than for a span.
        let wanted_envelopes = row.expected.len();
        let first = delivered_at_least(&router, case, &first_url, wanted_envelopes).await;
        let second = delivered_at_least(&router, case, &second_url, wanted_envelopes).await;
        let other = delivered(&router, case, &other_url).await;
        assert_eq!(
            first, second,
            "case {case:?}: every target of the rule receives the same envelopes, got \
             {first:?} and {second:?}"
        );
        assert!(
            other.is_empty(),
            "case {case:?}: a rule whose pattern cannot match receives nothing, got {other:?}"
        );

        let mut kinds: Vec<&str> = first
            .iter()
            .map(|event| event["detail-type"].as_str().unwrap_or_default())
            .collect();
        kinds.sort_unstable();
        let mut wanted: Vec<&str> = row.expected.to_vec();
        wanted.sort_unstable();
        assert_eq!(
            kinds, wanted,
            "case {case:?}: expected exactly the detail-types the pattern matches, got \
             {first:?}"
        );
        for event in &first {
            assert_eq!(
                event["source"], json!(EVENT_SOURCE),
                "case {case:?}: every envelope names AWS Backup as its source, got {event}"
            );
        }

        // The backup-job envelope, where the pattern is owed one: the job's own stored
        // members, as `DescribeBackupJob` reports them.
        if row.expected.contains(&BACKUP_JOB_STATE_CHANGE) {
            let envelopes = of_type(&first, BACKUP_JOB_STATE_CHANGE);
            assert_eq!(
                envelopes.len(),
                1,
                "case {case:?}: the settle is the job's ONE published move \u{2014} the state the \
                 start stored is not one, got {envelopes:?}"
            );
            let event = &envelopes[0];
            for (member, expected) in [
                ("backupJobId", json!(backup_job_id)),
                ("state", json!(COMPLETED)),
                ("backupVaultName", json!(SOURCE_VAULT)),
                ("backupVaultArn", json!(vault_arn(REGION, SOURCE_VAULT))),
                ("recoveryPointArn", json!(recovery_point_arn)),
                ("resourceArn", json!(resource_arn)),
                ("iamRoleArn", json!(role())),
                ("percentDone", described["PercentDone"].clone()),
                ("resourceType", described["ResourceType"].clone()),
            ] {
                assert_eq!(
                    event["detail"][member], expected,
                    "case {case:?}: expected detail.{member} {expected}, got {event}"
                );
            }
            assert_eq!(
                event["resources"],
                json!([recovery_point_arn]),
                "case {case:?}: the envelope names the recovery point the job wrote, got {event}"
            );
        }

        // The copy-job envelope: both ends of the copy, agreeing with `DescribeCopyJob`.
        if row.expected.contains(&COPY_JOB_STATE_CHANGE) {
            let envelopes = of_type(&first, COPY_JOB_STATE_CHANGE);
            assert_eq!(
                envelopes.len(),
                1,
                "case {case:?}: the settle is the copy job's ONE published move, got \
                 {envelopes:?}"
            );
            let event = &envelopes[0];
            for (member, expected) in [
                ("copyJobId", json!(copy_job_id)),
                ("state", json!(COMPLETED)),
                (
                    "sourceBackupVaultArn",
                    copy_job["SourceBackupVaultArn"].clone(),
                ),
                (
                    "sourceRecoveryPointArn",
                    copy_job["SourceRecoveryPointArn"].clone(),
                ),
                (
                    "destinationBackupVaultArn",
                    copy_job["DestinationBackupVaultArn"].clone(),
                ),
                (
                    "destinationRecoveryPointArn",
                    copy_job["DestinationRecoveryPointArn"].clone(),
                ),
                ("resourceArn", json!(resource_arn)),
                ("iamRoleArn", json!(role())),
            ] {
                assert_eq!(
                    event["detail"][member], expected,
                    "case {case:?}: expected detail.{member} {expected}, got {event}"
                );
            }
            // The destination vault's own ARN, whichever Region it belongs to.
            assert_eq!(
                event["detail"]["destinationBackupVaultArn"],
                json!(destination_arn),
                "case {case:?}: the copy reports the destination vault it was given, got {event}"
            );
            assert_ne!(
                event["detail"]["sourceBackupVaultArn"],
                event["detail"]["destinationBackupVaultArn"],
                "case {case:?}: the two ends of the copy are distinct vaults, got {event}"
            );
            assert_eq!(
                event["resources"],
                json!([copy_job["DestinationRecoveryPointArn"].as_str().unwrap_or_default()]),
                "case {case:?}: the envelope names the copy the job wrote, got {event}"
            );
        }
    }
}

/// Nothing is published for the state a start call stored, and nothing more is published
/// for a job that is read again after it has settled: a state that has not changed owes no
/// event however many times it is reported.
#[tokio::test]
async fn a_state_nothing_moved_into_publishes_nothing() {
    let case = "no move, no event";
    let router = emulator();
    for vault in [SOURCE_VAULT, DESTINATION_VAULT] {
        let (status, created) = create_vault(&router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBackupVault {vault:?} answered {status} {created}"
        );
    }
    let (url, arn) = given_queue(&router, case, "every-backup-event").await;
    given_rule(
        &router,
        case,
        MATCHING_RULE,
        &json!({ "source": [EVENT_SOURCE] }),
        &[arn],
    )
    .await;

    let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let (status, started) = start_job(
        &router,
        &json!({
            "BackupVaultName": SOURCE_VAULT,
            "ResourceArn": resource_arn,
            "IamRoleArn": role(),
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBackupJob answered {status} {started}"
    );
    let backup_job_id = string_at(&started, "BackupJobId");

    // The job is still in the state the start stored, which that call's own response
    // already reported.
    let described = describe_backup_job(&router, &backup_job_id).await;
    assert_eq!(
        described.get("State").and_then(Value::as_str),
        Some("CREATED"),
        "case {case:?}: the job has not settled yet, got {described}"
    );

    // Once the settle span has passed, the ONE move the job made is the one envelope on
    // the queue: no envelope reports the state the start stored, whichever call or the
    // clock itself materialised the move.
    std::thread::sleep(PAST_SETTLE);
    let settled = describe_backup_job(&router, &backup_job_id).await;
    assert_eq!(
        settled.get("State").and_then(Value::as_str),
        Some(COMPLETED),
        "case {case:?}: the job must have settled, got {settled}"
    );
    let after = delivered(&router, case, &url).await;
    let states: Vec<&str> = after
        .iter()
        .map(|event| event["detail"]["state"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        states,
        vec![COMPLETED],
        "case {case:?}: the settle is the job's ONE published move and the state the start \
         stored is not one, got {after:?}"
    );

    // Reading the settled job again, and listing every job, publishes no second copy of a
    // state that has not changed.
    describe_backup_job(&router, &backup_job_id).await;
    let (status, listed) = call(&router, "GET", "/backup-jobs", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListBackupJobs answered {status} {listed}"
    );
    let again = delivered(&router, case, &url).await;
    assert!(
        again.is_empty(),
        "case {case:?}: a state that has not changed owes no further event, got {again:?}"
    );
}

/// A transition reaches the bus with NO further call from anybody: the client that watches
/// a backup and a copy through EventBridge calls AWS Backup once to start each job, and
/// never reads either of them.
///
/// On the MULTI-THREADED runtime, because the clock pump is a task of its own: on a
/// current-thread runtime it could only run while this test awaits something pending, and
/// what this case asserts is that nothing of this test's doing is what publishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clock_publishes_a_transition_with_no_call_to_read_it() {
    let case = "nobody reads the job";
    let router = emulator();
    // The copy's destination ARN below names another Region, and a vault
    // belongs to the Region it was created in, so the destination vault is
    // created through a client there.
    let (status, created) = create_vault(&router, SOURCE_VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault {SOURCE_VAULT:?} answered {status} {created}"
    );
    let (status, created) = backup_in(
        &router,
        COPY_DESTINATION_REGION,
        "PUT",
        &format!("/backup-vaults/{DESTINATION_VAULT}"),
        Some("{}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault {DESTINATION_VAULT:?} in \
         {COPY_DESTINATION_REGION} answered {status} {created}"
    );
    let (url, arn) = given_queue(&router, case, "unread-jobs").await;
    given_rule(
        &router,
        case,
        MATCHING_RULE,
        &json!({ "source": [EVENT_SOURCE] }),
        &[arn],
    )
    .await;

    let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let (status, started) = start_job(
        &router,
        &json!({
            "BackupVaultName": SOURCE_VAULT,
            "ResourceArn": resource_arn,
            "IamRoleArn": role(),
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBackupJob answered {status} {started}"
    );
    let backup_job_id = string_at(&started, "BackupJobId");
    let recovery_point_arn = string_at(&started, "RecoveryPointArn");
    // Waited for by LOOKING AGAIN at the queue, never by reading the job: the drain is
    // what this case is allowed to call, and it is repeated until the envelope the clock
    // owes is there.
    let delivered = delivered_at_least(&router, case, &url, 1).await;
    let envelopes = of_type(&delivered, BACKUP_JOB_STATE_CHANGE);
    assert_eq!(
        envelopes.len(),
        1,
        "case {case:?}: the backup job's settle reaches the bus unread, got {delivered:?}"
    );
    assert_eq!(
        (
            envelopes[0]["detail"]["backupJobId"].clone(),
            envelopes[0]["detail"]["state"].clone()
        ),
        (json!(backup_job_id), json!(COMPLETED)),
        "case {case:?}: expected the job's own id and terminal state, got {:?}",
        envelopes[0]
    );

    // The same for the copy job, started from the recovery point the unread backup wrote.
    let (status, copy_started) = call(
        &router,
        "PUT",
        "/copy-jobs",
        Some(
            &json!({
                "RecoveryPointArn": recovery_point_arn,
                "SourceBackupVaultName": SOURCE_VAULT,
                "DestinationBackupVaultArn": vault_arn(COPY_DESTINATION_REGION, DESTINATION_VAULT),
                "IamRoleArn": role(),
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartCopyJob answered {status} {copy_started}"
    );
    let copy_job_id = string_at(&copy_started, "CopyJobId");
    let delivered = delivered_at_least(&router, case, &url, 1).await;
    let envelopes = of_type(&delivered, COPY_JOB_STATE_CHANGE);
    assert_eq!(
        envelopes.len(),
        1,
        "case {case:?}: the copy job's settle reaches the bus unread, got {delivered:?}"
    );
    assert_eq!(
        (
            envelopes[0]["detail"]["copyJobId"].clone(),
            envelopes[0]["detail"]["state"].clone(),
            envelopes[0]["detail"]["destinationBackupVaultArn"].clone()
        ),
        (
            json!(copy_job_id),
            json!(COMPLETED),
            json!(vault_arn(COPY_DESTINATION_REGION, DESTINATION_VAULT))
        ),
        "case {case:?}: expected the copy's own id, terminal state and destination vault, \
         got {:?}",
        envelopes[0]
    );
}

/// A job's events reach the bus of the Region the job BELONGS to — the Region of the vault
/// it was taken into — and no other.
#[tokio::test]
async fn a_jobs_events_reach_the_bus_of_the_region_its_vault_belongs_to() {
    /// A Region that is not the one every other case works in, and not one any ARN in this
    /// suite is otherwise minted for.
    const ELSEWHERE: &str = "eu-central-1";
    /// The vault created through a client in [`ELSEWHERE`], so its ARN carries that Region.
    const ELSEWHERE_VAULT: &str = "events-elsewhere";

    let case = "a vault in another Region";
    let router = emulator();
    let (status, created) = backup_in(
        &router,
        ELSEWHERE,
        "PUT",
        &format!("/backup-vaults/{ELSEWHERE_VAULT}"),
        Some("{}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault in {ELSEWHERE} answered {status} {created}"
    );
    assert_eq!(
        created.get("BackupVaultArn").and_then(Value::as_str),
        Some(vault_arn(ELSEWHERE, ELSEWHERE_VAULT).as_str()),
        "case {case:?}: the vault belongs to the Region it was created in, got {created}"
    );

    // One rule per Region, each matching EVERY AWS Backup event, each with its own queue:
    // only the Region the job belongs to is owed the envelope.
    let (home_url, home_arn) = given_queue_in(&router, REGION, case, "home-region").await;
    let (other_url, other_arn) = given_queue_in(&router, ELSEWHERE, case, "other-region").await;
    let pattern = json!({ "source": [EVENT_SOURCE] });
    given_rule_in(&router, REGION, case, MATCHING_RULE, &pattern, &[home_arn]).await;
    given_rule_in(&router, ELSEWHERE, case, MATCHING_RULE, &pattern, &[other_arn]).await;

    // The job is started by a client in ELSEWHERE, into the vault that belongs to it: a
    // vault is identified by its name within ONE Region, so a backup into it is started
    // in that Region.
    let resource_arn = format!("arn:aws:dynamodb:{ELSEWHERE}:{ACCOUNT}:table/demo");
    let (status, started) = backup_in(
        &router,
        ELSEWHERE,
        "PUT",
        "/backup-jobs",
        Some(
            &json!({
                "BackupVaultName": ELSEWHERE_VAULT,
                "ResourceArn": resource_arn,
                "IamRoleArn": role(),
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBackupJob answered {status} {started}"
    );
    let backup_job_id = string_at(&started, "BackupJobId");
    // The settle falls due on the clock, so the job is looked at again until it has
    // reached its terminal state rather than after a fixed span.
    let described = settled_backup_job(&router, case, &backup_job_id).await;
    assert_eq!(
        described.get("State").and_then(Value::as_str),
        Some(COMPLETED),
        "case {case:?}: the job must have settled, got {described}"
    );

    let elsewhere = delivered_at_least_in(&router, ELSEWHERE, case, &other_url, 1).await;
    assert_eq!(
        elsewhere.len(),
        1,
        "case {case:?}: the vault's own Region is owed the envelope, got {elsewhere:?}"
    );
    assert_eq!(
        (
            elsewhere[0]["detail-type"].clone(),
            elsewhere[0]["detail"]["backupJobId"].clone(),
            elsewhere[0]["detail"]["backupVaultArn"].clone()
        ),
        (
            json!(BACKUP_JOB_STATE_CHANGE),
            json!(backup_job_id),
            json!(vault_arn(ELSEWHERE, ELSEWHERE_VAULT))
        ),
        "case {case:?}: expected the job's own members, got {:?}",
        elsewhere[0]
    );
    let home = delivered(&router, case, &home_url).await;
    assert!(
        home.is_empty(),
        "case {case:?}: the Region the call was made in is owed nothing, got {home:?}"
    );
}
