// SPDX-License-Identifier: CC0-1.0

//! A schedule is INVOKED: end to end over the wire through the real
//! `MultiServiceRouter`, with the service the schedule's target lives in and IAM
//! mounted beside EventBridge Scheduler on ONE engine — so an invocation is
//! asserted where a client observes it, in the messages the target queue holds.
//!
//! RULE: a schedule whose `State` is `ENABLED` invokes its target at each
//! occurrence its `ScheduleExpression` names — once for `at(...)`, repeatedly for
//! `rate(...)` and `cron(...)` — inside the window `StartDate`/`EndDate` bound,
//! acting as `Target.RoleArn` and handing `Target.Input` to the API that target's
//! service is invoked by. A `DISABLED` schedule, a schedule whose window has not
//! opened, a series past its `EndDate` and a schedule that no longer exists invoke
//! nothing.
//!
//! The rows span the class rather than one recorded case: they differ along the
//! FORM of the expression (one-time instants, a rate, a cron expression), along
//! the schedule's `State`, and along which boundary of the window bounds the series.
//!
//! Every instant here is the schedule's OWN: an occurrence is performed at the
//! instant its expression names and never before it, so a row states an expression
//! whose occurrence falls seconds — or, for the minute-grained forms, a minute —
//! ahead and waits that long for it. What the occurrence computation itself names,
//! over the whole class of expressions, windows and zones, is asserted without a
//! wait in `src/state/invocation.rs`'s own tests.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::invocation;
use smithy_server_runtime::MultiServiceRouter;

// The scheduler request builder and the percent-encoder this crate's suites share
// (`tests/all/wire_harness.rs`), so this suite spells neither again.
use crate::wire_harness::{account_id, call_in, encode_label};

/// The region every request below is signed for.
pub(crate) const REGION: &str = "eu-west-2";

/// How long a row waits for an invocation it expects. The longest wait a row's
/// expression states is one minute (a `rate(1 minute)` series, a `cron` expression's
/// next minute); the rest is room for the clock's tick and the delivery.
const PATIENCE: Duration = Duration::from_secs(80);

/// How long a row watches a target it expects NOTHING to reach, and how long a row
/// watches for a SECOND message after the first: longer than the wait the row's own
/// expression states before the occurrence it does perform, so an invocation this
/// emulator would have performed has had its chance.
const QUIET_WINDOW: Duration = Duration::from_secs(6);

/// How long one poll of the target queue sleeps between reads.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The scheduler service principal a schedule's execution role must trust.
const SCHEDULER_PRINCIPAL: &str = "scheduler.amazonaws.com";

/// The emulator: EventBridge Scheduler, the service its targets live in, and IAM,
/// on ONE engine — the shape `core/router/src/lib.rs` mounts them in.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One restJson1 call against EventBridge Scheduler, through the shared harness.
pub(crate) async fn scheduler(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, method, uri, REGION, body).await
}

/// One awsJson1_0 call against Amazon SQS.
pub(crate) async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let response = sqs_response(router, operation, body, false).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, answered as the WHOLE response: a refusal is read for the headers
/// it carries as well as for its body. `query_mode` sends the header a
/// query-compatible SDK opts into the legacy error report with, which is how such an
/// SDK — the one an invocation reaches this service through — reads an error code.
pub(crate) async fn sqs_response(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    query_mode: bool,
) -> http::Response<bytes::Bytes> {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sqs/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    if query_mode {
        request = request.header("x-amzn-query-mode", "true");
    }
    let request = request
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    router.dispatch(request).await
}

/// One awsQuery call against IAM, answered as the XML text it is. The form
/// components are encoded by the harness's own percent-encoder, whose unreserved
/// set is the one an SDK encodes a query body with.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&encode_label(name));
        body.push('=');
        body.push_str(&encode_label(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/iam/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// A queue called `name`: its url and the ARN a schedule names it by.
pub(crate) async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let (status, created) = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
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
    (url, arn)
}

/// A role EventBridge Scheduler may assume, carrying the inline policy a schedule's
/// SQS target needs — the pair AWS documents for an execution role: a trust policy
/// for the scheduler service principal and `sqs:SendMessage` on the target queue.
pub(crate) async fn given_execution_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queue_arn: &str,
) -> String {
    given_execution_role_for(router, case, name, &[queue_arn]).await
}

/// The same role, granted `sqs:SendMessage` on each of `queues` — none of them for a
/// role whose permissions allow the schedule nothing at all.
pub(crate) async fn given_execution_role_for(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queues: &[&str],
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SCHEDULER_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    // The ARN IAM holds the role under is the total function of the account and the
    // name a role ARN is, so the answer is checked against it rather than read out
    // of itself.
    let arn = format!("arn:aws:iam::{}:role/{name}", account_id());
    assert!(
        created.contains(&arn),
        "case {case:?}: expected CreateRole to report the ARN {arn:?}, got {created}"
    );
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "sqs:SendMessage",
            "Resource": queues,
        }],
    })
    .to_string();
    if queues.is_empty() {
        // A role carrying no permissions at all: there is no statement to write, and
        // what such a role withholds is what the invocation is then refused by.
        return arn;
    }
    let (status, written) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "send-to-target"),
            ("PolicyDocument", &policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status}: {written}"
    );
    arn
}

/// Every message waiting on the queue at `url`, left on it: a row that waits for a
/// second message must still see the first.
pub(crate) async fn peek(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let (status, received) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "VisibilityTimeout": 0,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receive {url}: ReceiveMessage answered {status} {received}"
    );
    received["Messages"].as_array().cloned().unwrap_or_default()
}

/// The BODIES of the messages the queue at `url` holds, left on it: a suite that
/// waits for a further delivery must still see the ones before it.
pub(crate) async fn bodies(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    peek(router, url)
        .await
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The bodies of the messages the queue at `url` holds once it holds at least
/// `wanted`, or whatever it holds after `patience`.
async fn bodies_once(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    patience: Duration,
) -> Vec<String> {
    let began = Instant::now();
    loop {
        let held = bodies(router, url).await;
        if held.len() >= wanted || began.elapsed() >= patience {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every message the queue at `url` holds, REMOVED from it, so a later read reports
/// only what arrived after this one.
pub(crate) async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let held = peek(router, url).await;
    for message in &held {
        if let Some(receipt) = message["ReceiptHandle"].as_str() {
            let (status, deleted) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": receipt }),
            )
            .await;
            assert_eq!(
                status, 200,
                "draining {url}: DeleteMessage answered {status} {deleted}"
            );
        }
    }
    held
}

/// The `FlexibleTimeWindow` a case states: `MaximumWindowInMinutes` under
/// `Mode=FLEXIBLE`, or `Mode=OFF` for a schedule asking for no window at all.
pub(crate) fn flexible_time_window(window_minutes: Option<i64>) -> Value {
    match window_minutes {
        Some(minutes) => json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": minutes }),
        None => json!({ "Mode": "OFF" }),
    }
}

/// `CreateSchedule` for one case, answering what the service reported.
pub(crate) async fn given_schedule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    body: Value,
) -> Value {
    let (status, created) =
        scheduler(router, "POST", &format!("/schedules/{name}"), Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );
    created
}

/// `GetSchedule` for one schedule of `group`: whether it resolves, and the refusal it
/// answers when it does not.
pub(crate) async fn schedule_read(
    router: &MultiServiceRouter,
    group: &str,
    name: &str,
) -> (u16, Value) {
    scheduler(
        router,
        "GET",
        &format!("/schedules/{name}?groupName={group}"),
        None,
    )
    .await
}

/// `at` as an `at(...)` expression's body states it: a local date and time with no
/// offset, which with no `ScheduleExpressionTimezone` is read in UTC.
pub(crate) fn at_expression_body(at: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// A whole number of seconds from now, which is the resolution an `at(...)`
/// expression states an instant at.
pub(crate) fn seconds_from_now(seconds: u64) -> SystemTime {
    let now = SystemTime::now();
    let whole = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(whole + seconds)
}

/// What a row expects the target to be handed.
enum Expected {
    /// Exactly this many invocations, and no more inside [`QUIET_WINDOW`].
    Exactly(usize),
    /// At least this many, which is what a recurring series is asserted by.
    AtLeast(usize),
    /// Nothing at all.
    Nothing,
}

/// One row: the schedule written, and what its target is handed.
struct Row {
    case: &'static str,
    /// The `ScheduleExpression`, built when the row runs so an `at(...)` instant is
    /// ahead of the clock.
    expression: fn() -> String,
    state: &'static str,
    /// `StartDate`, as seconds from now.
    start_in: Option<u64>,
    /// `EndDate`, as seconds from now.
    end_in: Option<u64>,
    expected: Expected,
}

const ROWS: &[Row] = &[
    Row {
        case: "a one-time schedule four seconds ahead invokes its target once",
        expression: || format!("at({})", at_expression_body(seconds_from_now(4))),
        state: "ENABLED",
        start_in: None,
        end_in: None,
        expected: Expected::Exactly(1),
    },
    Row {
        case: "a one-time schedule eight seconds ahead invokes its target once",
        expression: || format!("at({})", at_expression_body(seconds_from_now(8))),
        state: "ENABLED",
        start_in: None,
        end_in: None,
        expected: Expected::Exactly(1),
    },
    Row {
        case: "a rate schedule invokes its target once per period",
        expression: || "rate(1 minute)".to_string(),
        state: "ENABLED",
        start_in: None,
        end_in: None,
        expected: Expected::AtLeast(1),
    },
    Row {
        case: "a cron schedule naming every minute invokes its target",
        expression: || "cron(* * * * ? *)".to_string(),
        state: "ENABLED",
        start_in: None,
        end_in: None,
        expected: Expected::AtLeast(1),
    },
    Row {
        case: "a DISABLED schedule invokes nothing",
        expression: || format!("at({})", at_expression_body(seconds_from_now(4))),
        state: "DISABLED",
        start_in: None,
        end_in: None,
        expected: Expected::Nothing,
    },
    Row {
        case: "a recurring schedule whose window has not opened invokes nothing",
        expression: || "rate(1 minute)".to_string(),
        state: "ENABLED",
        start_in: Some(7_200),
        end_in: None,
        expected: Expected::Nothing,
    },
    Row {
        case: "a recurring schedule is invoked only at the occurrences before its EndDate",
        expression: || "rate(1 minute)".to_string(),
        state: "ENABLED",
        start_in: None,
        // One period on is inside the window, two periods on is not.
        end_in: Some(90),
        expected: Expected::Exactly(1),
    },
];

/// Every row: the schedule is invoked at the occurrences its own expression names,
/// and what its target is handed is the `Input` the schedule states.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schedule_invokes_its_target_at_the_occurrences_it_names() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let queue = format!("target-{index}");
        let (url, queue_arn) = given_queue(&emulator, case, &queue).await;
        let role = given_execution_role(&emulator, case, &format!("role-{index}"), &queue_arn).await;
        let marker = format!("{{\"marker\":\"{index}\"}}");

        let mut body = json!({
            "ScheduleExpression": (row.expression)(),
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "State": row.state,
            "Target": { "Arn": queue_arn, "RoleArn": role, "Input": marker },
        });
        if let Some(start_in) = row.start_in {
            body["StartDate"] = json!(epoch_seconds(seconds_from_now(start_in)));
        }
        if let Some(end_in) = row.end_in {
            body["EndDate"] = json!(epoch_seconds(seconds_from_now(end_in)));
        }
        let name = format!("schedule-{index}");
        let (status, created) =
            scheduler(&emulator, "POST", &format!("/schedules/{name}"), Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );
        // The schedule is read back through the service's own reader, so what is
        // invoked below is the record a client sees.
        let (status, read) = scheduler(
            &emulator,
            "GET",
            &format!("/schedules/{name}?groupName=default"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetSchedule answered {status} {read}"
        );
        assert_eq!(
            read["State"].as_str(),
            Some(row.state),
            "case {case:?}: GetSchedule reports another State: {read}"
        );

        match row.expected {
            Expected::Nothing => {
                tokio::time::sleep(QUIET_WINDOW).await;
                let held = peek(&emulator, &url).await;
                assert!(
                    held.is_empty(),
                    "case {case:?}: expected no invocation at all, got {held:?}"
                );
            }
            Expected::AtLeast(wanted) => {
                let held = bodies_once(&emulator, &url, wanted, PATIENCE).await;
                assert!(
                    held.len() >= wanted,
                    "case {case:?}: expected at least {wanted} invocation(s), got {held:?}"
                );
                assert!(
                    held.iter().all(|body| body == &marker),
                    "case {case:?}: expected every body to be the schedule's own Input \
                     {marker:?}, got {held:?}"
                );
            }
            Expected::Exactly(wanted) => {
                let held = bodies_once(&emulator, &url, wanted, PATIENCE).await;
                assert_eq!(
                    held.len(),
                    wanted,
                    "case {case:?}: expected {wanted} invocation(s), got {held:?}"
                );
                assert!(
                    held.iter().all(|body| body == &marker),
                    "case {case:?}: expected every body to be the schedule's own Input \
                     {marker:?}, got {held:?}"
                );
                // The series states no further occurrence, so none arrives however
                // long the clock runs on.
                tokio::time::sleep(QUIET_WINDOW).await;
                let after = peek(&emulator, &url).await;
                assert_eq!(
                    after.len(),
                    wanted,
                    "case {case:?}: expected no further invocation, got {after:?}"
                );
            }
        }
    }
}

/// One instant as the seconds-since-the-epoch a restJson1 timestamp member carries.
pub(crate) fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The invocation carries the schedule's context attributes, substituted into the
/// target's own `Input`: the scheduled time is the occurrence's OWN instant — the
/// one the expression names, not the instant this emulator performed it — and the
/// execution id and the schedule's ARN are the schedule's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invocation_carries_the_schedules_context_attributes() {
    let case = "an Input carrying every context attribute";
    let emulator = emulator();
    let (url, queue_arn) = given_queue(&emulator, case, "context-target").await;
    let role = given_execution_role(&emulator, case, "context-role", &queue_arn).await;
    let occurrence = seconds_from_now(4);
    let input = json!({
        "at": "<aws.scheduler.scheduled-time>",
        "execution": "<aws.scheduler.execution-id>",
        "schedule": "<aws.scheduler.schedule-arn>",
        "attempt": "<aws.scheduler.attempt-number>",
    })
    .to_string();
    let (status, created) = scheduler(
        &emulator,
        "POST",
        "/schedules/context",
        Some(json!({
            "ScheduleExpression": format!("at({})", at_expression_body(occurrence)),
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "Target": { "Arn": queue_arn, "RoleArn": role, "Input": input },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );
    let schedule_arn = created["ScheduleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ScheduleArn in {created}"))
        .to_string();

    let held = bodies_once(&emulator, &url, 1, PATIENCE).await;
    let first = held
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the target was never invoked"));
    let delivered: Value = serde_json::from_str(first)
        .unwrap_or_else(|error| panic!("case {case:?}: the body is not JSON ({error}): {first:?}"));
    assert_eq!(
        delivered["at"].as_str(),
        Some(invocation::scheduled_time_text(occurrence).as_str()),
        "case {case:?}: expected the occurrence's own scheduled time, got {delivered}"
    );
    assert_eq!(
        delivered["schedule"].as_str(),
        Some(schedule_arn.as_str()),
        "case {case:?}: expected the schedule's own ARN, got {delivered}"
    );
    assert_eq!(
        delivered["attempt"].as_str(),
        Some("1"),
        "case {case:?}: expected the first attempt, got {delivered}"
    );
    let execution = delivered["execution"].as_str().unwrap_or_default();
    // "8-4-4-4-12": the form EventBridge Scheduler names an invocation in.
    let groups: Vec<usize> = execution.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected an execution id in the published form, got {execution:?}"
    );
    assert!(
        execution.bytes().all(|byte| byte == b'-' || byte.is_ascii_hexdigit()),
        "case {case:?}: expected a hexadecimal execution id, got {execution:?}"
    );
}

/// A schedule DELETED before its occurrence invokes nothing: the series belongs to
/// the schedule, so removing it — which is what a stack delete does to a schedule a
/// stack deployed — stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_schedule_invokes_nothing() {
    let case = "a schedule deleted before its occurrence";
    let emulator = emulator();
    let (url, queue_arn) = given_queue(&emulator, case, "deleted-target").await;
    let role = given_execution_role(&emulator, case, "deleted-role", &queue_arn).await;
    let (status, created) = scheduler(
        &emulator,
        "POST",
        "/schedules/deleted",
        Some(json!({
            "ScheduleExpression": format!("at({})", at_expression_body(seconds_from_now(4))),
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "Target": { "Arn": queue_arn, "RoleArn": role, "Input": "{\"marker\":\"gone\"}" },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );
    let (status, deleted) = scheduler(&emulator, "DELETE", "/schedules/deleted", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteSchedule answered {status} {deleted}"
    );

    tokio::time::sleep(QUIET_WINDOW).await;
    let held = peek(&emulator, &url).await;
    assert!(
        held.is_empty(),
        "case {case:?}: expected no invocation at all, got {held:?}"
    );
}
