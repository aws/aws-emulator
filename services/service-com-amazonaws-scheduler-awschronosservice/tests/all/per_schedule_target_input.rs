// SPDX-License-Identifier: CC0-1.0

//! What one occurrence hands its target is the INVOKING schedule's own
//! `Target.Input` — end to end over the wire through the real `MultiServiceRouter`,
//! with the services a schedule's target lives in and IAM mounted beside EventBridge
//! Scheduler on ONE engine, so the payload is asserted where the target sees it.
//!
//! RULE: `Target$Input` — "the text, or well-formed JSON, passed to the target" —
//! belongs to the ONE schedule record that carries it, which the store addresses by
//! its group and its name, so every occurrence hands its target the `Input` of the
//! schedule the occurrence belongs to, verbatim, however many other schedules name
//! the SAME target; and an occurrence whose `Target$RoleArn` does not allow the call
//! the target is invoked by does not invoke the target at all, so no schedule's
//! `Input` is ever observable at another schedule's target.
//!
//! The rows span the class rather than one recorded case: they differ along the FORM
//! of the `Input` (a nested document carrying arrays, mixed number types, booleans,
//! null, non-ASCII text and embedded quotes; a flat object; the empty object; a
//! top-level array; text that is not JSON at all), along the GROUP the schedule lives
//! in — including two schedules of the same NAME in different groups, which is the
//! pair the store keys a record by — and along whether the schedule's execution role
//! allows the invocation.
//!
//! The first suite below invokes a QUEUE target, where the delivered payload is the
//! message body a client reads. The second invokes a FUNCTION target, which is the
//! shape a client fans several schedules into one handler with: the payload is
//! asserted in the asynchronous invocation Lambda holds for the function.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::schedule_invocation::{
    at_expression_body, emulator, given_execution_role_for, given_queue, iam, peek, scheduler,
    seconds_from_now, POLL_INTERVAL, REGION,
};
use crate::wire_harness::account_id;

/// How long a suite waits for the payloads it expects. Every schedule below states an
/// occurrence [`OCCURRENCE_IN`] seconds ahead; the rest is room for the clock's tick
/// and the delivery.
const PATIENCE: Duration = Duration::from_secs(80);

/// How long a suite keeps LOOKING after the payloads it expected have arrived, so a
/// payload this emulator would have delivered to the wrong target — or delivered
/// twice — has had its chance to appear. Nothing is asserted about how long anything
/// took: the window is only how long the absence is watched for.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// How far ahead of the clock every schedule below states its one occurrence. Each
/// schedule of a suite states the SAME instant, so the occurrences fall due together
/// — which is what puts the binding of a payload to a schedule under test.
const OCCURRENCE_IN: u64 = 6;

/// The scheduler service principal a schedule's execution role must trust.
const SCHEDULER_PRINCIPAL: &str = "scheduler.amazonaws.com";

/// The IAM action an invocation of a FUNCTION target is authorized by, as
/// `Target$RoleArn`'s own documentation names the grant a Lambda target needs.
pub(crate) const INVOKE_FUNCTION: &str = "lambda:InvokeFunction";

/// Where Lambda's `CreateFunction` is bound.
pub(crate) const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed Lambda execution role ARN, satisfying the model's `RoleArn` pattern.
/// The function's OWN role plays no part here: what is under test is the payload the
/// invocation carries.
const FUNCTION_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime a function this target creates runs on.
pub(crate) const RUNTIME: &str = "python3.12";

/// The `Handler` a function whose package carries no module at all names: nothing loads
/// it, which is why only a suite reading the payload an invocation carried uses it.
const EMPTY_HANDLER: &str = "index.handler";

/// An empty `.zip` deployment package, base64 as `FunctionCode$ZipFile` carries one.
/// Nothing here runs a handler — the payload is read from the invocation Lambda queued
/// — so the archive's contents do not matter.
const EMPTY_ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// A nested document: an array of mixed number types, a boolean pair, a null, an
/// empty object, a deeper object, non-ASCII text, an embedded quote and an integer
/// past 32 bits. `Target$Input` is "the text, or well-formed JSON, passed to the
/// target", and the whole of it travels verbatim.
const NESTED_INPUT: &str = r#"{"marker":"nested","nested":{"list":[1,2.5,-3,true,false,null],"empty":{},"deep":{"k":["v"]}},"unicode":"café ✓ 日本","quote":"she said \"hi\"","big":12345678901234}"#;

/// A flat object, the shape a client parameterises one handler per schedule with.
const FLAT_INPUT: &str = r#"{"marker":"flat","tenant":"b"}"#;

/// The empty JSON object: well-formed JSON carrying nothing.
const EMPTY_OBJECT_INPUT: &str = "{}";

/// A top-level ARRAY, which is well-formed JSON as much as an object is.
const ARRAY_INPUT: &str = r#"[1,"two",{"three":3}]"#;

/// TEXT that is not JSON at all, which `Target$Input` admits ("the text, or
/// well-formed JSON").
const TEXT_INPUT: &str = "a line of plain text, not JSON";

/// The `Input` of the schedule that shares its NAME with another group's schedule.
const OTHER_GROUP_INPUT: &str = r#"{"marker":"the other group's own payload"}"#;

/// The `Input` of the schedule whose execution role allows the invocation nothing:
/// this text must never be observable at the target.
const WITHHELD_INPUT: &str = r#"{"marker":"withheld"}"#;

/// The group a row states when it states one of its own, created by the suite.
const OTHER_GROUP: &str = "fan-in";

/// One row: where the schedule lives, what it hands its target, and whether its
/// execution role allows the invocation.
struct Row {
    case: &'static str,
    /// `GroupName`, or `None` for the account's implicit `default` group.
    group: Option<&'static str>,
    name: &'static str,
    input: &'static str,
    /// Whether `Target.RoleArn` allows the call the target is invoked by.
    authorized: bool,
}

/// The rows of the QUEUE-target suite.
const QUEUE_ROWS: &[Row] = &[
    Row {
        case: "a nested document in the default group",
        group: None,
        name: "nested",
        input: NESTED_INPUT,
        authorized: true,
    },
    Row {
        case: "a flat object in the default group",
        group: None,
        name: "flat",
        input: FLAT_INPUT,
        authorized: true,
    },
    Row {
        case: "the empty JSON object",
        group: None,
        name: "empty-object",
        input: EMPTY_OBJECT_INPUT,
        authorized: true,
    },
    Row {
        case: "a top-level array",
        group: None,
        name: "array",
        input: ARRAY_INPUT,
        authorized: true,
    },
    Row {
        case: "text that is not JSON",
        group: None,
        name: "text",
        input: TEXT_INPUT,
        authorized: true,
    },
    Row {
        case: "a schedule of the same name in another group",
        group: Some(OTHER_GROUP),
        name: "nested",
        input: OTHER_GROUP_INPUT,
        authorized: true,
    },
    Row {
        case: "a schedule whose execution role allows the invocation nothing",
        group: None,
        name: "withheld",
        input: WITHHELD_INPUT,
        authorized: false,
    },
];

/// The rows of the FUNCTION-target suite: the fan-in shape the issue describes — one
/// function, several schedules, each with its own parameters — with one of them
/// denied the call.
const FUNCTION_ROWS: &[Row] = &[
    Row {
        case: "a nested document handed to the shared function",
        group: None,
        name: "fn-nested",
        input: NESTED_INPUT,
        authorized: true,
    },
    Row {
        case: "a flat object handed to the same function",
        group: None,
        name: "fn-flat",
        input: FLAT_INPUT,
        authorized: true,
    },
    Row {
        case: "a schedule of the same name in another group, on the same function",
        group: Some(OTHER_GROUP),
        name: "fn-nested",
        input: OTHER_GROUP_INPUT,
        authorized: true,
    },
    Row {
        case: "a schedule whose execution role may not invoke the function",
        group: None,
        name: "fn-withheld",
        input: WITHHELD_INPUT,
        authorized: false,
    },
];

/// Several schedules naming ONE queue target each hand it their own `Input`, and the
/// schedule whose role allows nothing hands it nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_schedule_hands_one_shared_queue_target_its_own_input() {
    let emulator = emulator();
    let suite = "schedules sharing one queue target";
    let (url, queue_arn) = given_queue(&emulator, suite, "shared-target").await;
    let allowed = given_execution_role_for(&emulator, suite, "allowed-role", &[&queue_arn]).await;
    // A role that allows this invocation nothing at all: it exists and EventBridge
    // Scheduler may assume it, so what withholds the call is its permissions.
    let withholding = given_execution_role_for(&emulator, suite, "withholding-role", &[]).await;
    given_group(&emulator, suite, OTHER_GROUP).await;

    let occurrence = seconds_from_now(OCCURRENCE_IN);
    for row in QUEUE_ROWS {
        let role = if row.authorized { &allowed } else { &withholding };
        create_schedule(&emulator, row, occurrence, &queue_arn, role).await;
    }

    let expected = expected_payloads(QUEUE_ROWS);
    let held = payloads_once(&emulator, &url, expected.len()).await;
    assert_eq!(
        held, expected,
        "case {suite:?}: expected each schedule's own Input delivered once, got {held:?}"
    );
    // Nothing further arrives: no schedule's Input reaches the target twice, and the
    // withheld one never reaches it at all.
    let deadline = Instant::now() + QUIET_WINDOW;
    while Instant::now() < deadline {
        let again = queue_payloads(&emulator, &url).await;
        assert_eq!(
            again, expected,
            "case {suite:?}: expected the target to hold exactly each schedule's own Input, got \
             {again:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Several schedules naming ONE function target each invoke it with their own
/// `Input`, and the schedule whose role may not invoke the function does not invoke
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_schedule_invokes_one_shared_function_target_with_its_own_input() {
    let suite = "schedules sharing one function target";
    let (emulator, engine) = emulator_with_lambda();
    let function = "shared-handler";
    let function_arn = given_function(&emulator, suite, function).await;
    let allowed = given_invoking_role(
        &emulator,
        suite,
        "fn-allowed-role",
        &[(INVOKE_FUNCTION, &function_arn)],
    )
    .await;
    // A role EventBridge Scheduler may assume whose policies name another action
    // entirely, so nothing in them allows the invocation of the function.
    let withholding = given_invoking_role(
        &emulator,
        suite,
        "fn-withholding-role",
        &[(
            "sqs:SendMessage",
            &format!("arn:aws:sqs:{REGION}:{}:somewhere-else", account_id()),
        )],
    )
    .await;
    given_group(&emulator, suite, OTHER_GROUP).await;

    let occurrence = seconds_from_now(OCCURRENCE_IN);
    for row in FUNCTION_ROWS {
        let role = if row.authorized { &allowed } else { &withholding };
        create_schedule(&emulator, row, occurrence, &function_arn, role).await;
    }

    let expected = expected_payloads(FUNCTION_ROWS);
    let held = invoked_payloads_once(&engine, function, expected.len()).await;
    assert_eq!(
        held, expected,
        "case {suite:?}: expected the function invoked with each schedule's own Input, got \
         {held:?}"
    );
    let deadline = Instant::now() + QUIET_WINDOW;
    while Instant::now() < deadline {
        let again = invoked_payloads(&engine, function);
        assert_eq!(
            again, expected,
            "case {suite:?}: expected the function to have been invoked with exactly each \
             schedule's own Input, got {again:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The payloads the rows that ARE invoked hand their target, as a set: the delivery
/// order of occurrences falling due together is nothing AWS promises.
fn expected_payloads(rows: &[Row]) -> BTreeSet<String> {
    rows.iter()
        .filter(|row| row.authorized)
        .map(|row| row.input.to_string())
        .collect()
}

/// Write one row's schedule: its one occurrence at `occurrence`, its own `Input`, and
/// the target every row of its suite shares.
async fn create_schedule(
    router: &MultiServiceRouter,
    row: &Row,
    occurrence: std::time::SystemTime,
    target_arn: &str,
    role_arn: &str,
) {
    let case = row.case;
    let mut body = json!({
        "ScheduleExpression": format!("at({})", at_expression_body(occurrence)),
        "ScheduleExpressionTimezone": "UTC",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "State": "ENABLED",
        "Target": {
            "Arn": target_arn,
            "RoleArn": role_arn,
            "Input": row.input,
        },
    });
    if let Some(group) = row.group {
        body["GroupName"] = json!(group);
    }
    let (status, created) = scheduler(
        router,
        "POST",
        &format!("/schedules/{}", row.name),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );
    // The record the occurrence is performed FROM is the one a client reads, so the
    // `Input` under test is read back through the service's own reader first.
    let group = row.group.unwrap_or("default");
    let (status, read) = scheduler(
        router,
        "GET",
        &format!("/schedules/{}?groupName={group}", row.name),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSchedule answered {status} {read}"
    );
    assert_eq!(
        read["Target"]["Input"].as_str(),
        Some(row.input),
        "case {case:?}: expected GetSchedule to echo the schedule's own Input {:?}, got {read}",
        row.input
    );
}

/// A schedule group of `name`, which a row that states one needs before its schedule
/// can live there.
pub(crate) async fn given_group(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, created) = scheduler(
        router,
        "POST",
        &format!("/schedule-groups/{name}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateScheduleGroup answered {status} {created}"
    );
}

/// The bodies the queue at `url` holds, as a set.
async fn queue_payloads(router: &MultiServiceRouter, url: &str) -> BTreeSet<String> {
    peek(router, url)
        .await
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The bodies the queue at `url` holds once it holds at least `wanted` of them, or
/// whatever it holds once [`PATIENCE`] has passed.
async fn payloads_once(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
) -> BTreeSet<String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let held = queue_payloads(router, url).await;
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// EventBridge Scheduler, Amazon SQS, IAM and Lambda on ONE engine — the shape
/// `core/router/src/lib.rs` mounts them in — with the engine itself, so a suite can
/// read the invocation Lambda holds for a function.
pub(crate) fn emulator_with_lambda() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
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
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A function of `name`, and the ARN a schedule names it by.
///
/// Its package carries no handler module, which is all a suite that reads the PAYLOAD an
/// invocation carried needs; one whose function must RUN states its own
/// ([`given_function_with_code`]).
pub(crate) async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    given_function_with_code(router, case, name, EMPTY_HANDLER, EMPTY_ZIP_FILE).await
}

/// The same create, with the `Handler` and the deployment package stated: the ONE
/// `CreateFunction` call this test target makes, so a suite differs from another only in
/// the code it uploads.
pub(crate) async fn given_function_with_code(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    handler: &str,
    zip_file: &str,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        Some(json!({
            "FunctionName": name,
            "Role": FUNCTION_ROLE,
            "Runtime": RUNTIME,
            "Handler": handler,
            "Code": { "ZipFile": zip_file },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction answered {status} {created}"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {created}"))
        .to_string()
}

/// One call against Lambda's restJson1 front door, which is how this test target reaches
/// the service a function target lives in.
pub(crate) async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |body| {
        bytes::Bytes::from(body.to_string())
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/lambda/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = response.into_body();
    let parsed = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A role EventBridge Scheduler may assume whose inline policy allows each
/// `(action, resource)` pair and nothing else.
pub(crate) async fn given_invoking_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    granted: &[(&str, &str)],
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
    let statements: Vec<Value> = granted
        .iter()
        .map(|(action, resource)| {
            json!({ "Effect": "Allow", "Action": action, "Resource": resource })
        })
        .collect();
    let policy = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
    let (status, written) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "invocation"),
            ("PolicyDocument", &policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status}: {written}"
    );
    format!("arn:aws:iam::{}:role/{name}", account_id())
}

/// The payloads Lambda holds asynchronous invocations of `function` for, as a set.
pub(crate) fn invoked_payloads(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
) -> BTreeSet<String> {
    invoked_payload_counts(engine, function)
        .into_keys()
        .collect()
}

/// The same invocations, COUNTED by the payload each one carries: Lambda holds one entry
/// per call that was delivered to the function — a run the function itself attempts again
/// is the same entry — so a payload's count is how many deliveries carried it.
pub(crate) fn invoked_payload_counts(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
) -> BTreeMap<String, usize> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let mut held: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(record) = store.functions.get(function) {
        for entry in &record.async_invocations {
            *held
                .entry(String::from_utf8_lossy(&entry.payload).to_string())
                .or_default() += 1;
        }
    }
    held
}

/// Those payloads once Lambda holds at least `wanted` of them, or whatever it holds
/// once [`PATIENCE`] has passed.
pub(crate) async fn invoked_payloads_once(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    wanted: usize,
) -> BTreeSet<String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let held = invoked_payloads(engine, function);
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
