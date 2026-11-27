// SPDX-License-Identifier: CC0-1.0

//! The EventBridge Scheduler calls a RUNNING handler makes with the AWS SDK its runtime
//! supplies, end to end over the wire: Lambda, EventBridge Scheduler, IAM and the token service
//! on ONE engine, a `python3.x` function whose code builds a client at module scope, and every
//! group and schedule it wrote read back where a client reads them — through that service's own
//! reads.
//!
//! The RULE pinned here has two halves:
//!
//! * a managed runtime ships the AWS SDK for the handler's language with the service model of
//!   every service the runtime's call seam binds, so a handler builds a client for one with no
//!   dependency of its own and what the call answers is THAT service's own answer — the
//!   operation's output members and the state the service then reports, or that service's own
//!   modelled refusal raised as `botocore.exceptions.ClientError` carrying its error code;
//! * a service decides authorization at the IAM layer ahead of its own code, so a call the
//!   execution role's identity-based policies do not allow is refused with HTTP 403
//!   `AccessDeniedException`, worded as the principal, the `<namespace>:<OperationName>` action
//!   and the resource the request addressed — and it performs nothing.
//!
//! The rows differ along what the rule generalises over: the OPERATION the handler calls
//! (a group create, a schedule create, a read, a listing, an update, a delete and a group
//! read), the SHAPE of the input it carries (a required label, a nested structure, an enum, an
//! integer inside a structure, a list of tag structures, an optional group that defaults), the
//! shape of the output (a minted ARN, the empty output of a delete, a document carrying a list,
//! a timestamp the service decides), whether the call is served or refused, and — for a refusal
//! — WHICH fault it is: a group that does not exist, a schedule that does not exist, a
//! cross-member rule of the input, a reserved name, and a grant the role does not hold, for an
//! operation that addresses one schedule and for one that addresses none.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]), and it makes THREE
//! invocations in all — the served calls, the refused calls, and the calls made once the grant
//! is withdrawn — because an invocation of its own costs this target a share of the run budget
//! every other invocation suite draws on.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The IAM call every suite in this target writes a role with, and the one .zip writer and
// function helpers a running handler is packaged and awaited with — owned by the suites that
// first needed them rather than copied.
use crate::event_source_mapping_role_access::iam as iam_call;
use crate::event_source_mappings::{await_function_active, call, router_on};
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for, which is the Region the function runs in.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The account every ARN below names, which is the account this emulator serves.
const ACCOUNT: &str = "000000000000";

/// The execution role the function runs as — the principal every call it makes is judged on —
/// the function's own name, and the inline identity policy the role carries the grants in.
const CALLER_ROLE: &str = "lambda-schedule-author";
const FUNCTION: &str = "schedule-author";
const INLINE_POLICY: &str = "inline";

/// The role the handler hands EventBridge Scheduler as a schedule's execution role: Scheduler
/// resolves `Target.RoleArn` when a schedule is created, so this one trusts the Scheduler
/// service principal.
const TARGET_ROLE: &str = "scheduler-target-runner";

/// The IAM service principal a schedule's execution role has to trust.
const SCHEDULER_PRINCIPAL: &str = "scheduler.amazonaws.com";

/// The AWS-managed policy a Lambda execution role is created with: it grants the log writes a
/// function makes and nothing else, so a role carrying only it has no grant on EventBridge
/// Scheduler.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The schedule group the handler creates, the account's built-in group, and a group name no
/// group answers to.
const GROUP: &str = "orders-schedules";
const DEFAULT_GROUP: &str = "default";
const ABSENT_GROUP: &str = "no-such-group";

/// The schedules the handler writes: a recurring one in the group it created, and a one-time
/// one in the built-in group. Plus a name no schedule answers to.
const RECURRING: &str = "orders.nightly-1";
const ONE_OFF: &str = "orders-once";
const ABSENT_SCHEDULE: &str = "no-such-schedule";

/// The expressions the schedules run on: the `rate` form the create carries, and the `cron`
/// form the update moves it to (`CreateScheduleInput$ScheduleExpression` documents `at`, `rate`
/// and `cron`). The one-time schedule names an instant far enough ahead that no occurrence of
/// it falls due while this suite runs.
const RATE_EXPRESSION: &str = "rate(1 hour)";
const CRON_EXPRESSION: &str = "cron(0 8 1 * ? *)";
const AT_EXPRESSION: &str = "at(2030-01-01T00:00:00)";

/// The zone the one-time schedule's expression is read in, and its description.
const TIMEZONE: &str = "Europe/London";
const DESCRIPTION: &str = "the one-time run a handler deferred";

/// The flexible window the one-time schedule carries, in minutes
/// (`FlexibleTimeWindow$MaximumWindowInMinutes` is modelled `@range(min: 1, max: 1440)`).
const WINDOW_MINUTES: u64 = 15;

/// The queue every schedule's target names, and the tag the group create carries.
const TARGET_QUEUE: &str = "arn:aws:sqs:us-east-1:000000000000:orders-due";
const TAG_KEY: &str = "team";
const TAG_VALUE: &str = "orders";

/// The error codes EventBridge Scheduler refuses with: a name it does not hold, an input its
/// own rules reject (both modelled by this service), and the AWS-common refusal of a call the
/// caller's policies do not permit.
const NOT_FOUND: &str = "ResourceNotFoundException";
const VALIDATION: &str = "ValidationException";
const ACCESS_DENIED: &str = "AccessDeniedException";

/// Lambda, EventBridge Scheduler, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the schedule it writes is the schedule this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. Nothing here is signed: the suite's own calls present the account's own published
/// credential.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One `restJson1` call to EventBridge Scheduler, as the status it answered and the document it
/// carried — the way a client reads this service, which is where the handler's writes are
/// asserted.
async fn scheduler_call(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("scheduler.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("scheduler"))
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "setup: expected a JSON document from EventBridge Scheduler's {uri}, got \
                 {error}: {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The same, for a call that must be served.
async fn scheduler(router: &MultiServiceRouter, method: &str, uri: &str) -> Value {
    let (status, parsed) = scheduler_call(router, method, uri).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from EventBridge Scheduler's {uri}, got {status} with {parsed}"
    );
    parsed
}

/// One IAM call that must be served.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) {
    let (status, text) = iam_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} / {text}"
    );
}

/// The handler every invocation runs: it makes the calls the event names, in order, and reports
/// what each one answered — so the invocation's payload IS what the SDK gave the guest.
///
/// The client is built at MODULE scope, which is where a real function builds one: a runtime
/// whose SDK carries no model for the service fails there, before any handler code runs.
const HANDLER_SOURCE: &str = r#"
import datetime

import boto3
import botocore.exceptions

SCHEDULER = boto3.client("scheduler")


def _reported(value):
    """`value` as JSON, which is what an invocation's payload has to be.

    An SDK hands a member the called service's model types as a timestamp over as a `datetime`,
    so a handler reporting the whole answer renders each instant in it as its ISO-8601 text.
    """
    if isinstance(value, datetime.datetime):
        return value.isoformat()
    if isinstance(value, dict):
        return {name: _reported(held) for name, held in value.items()}
    if isinstance(value, list):
        return [_reported(held) for held in value]
    return value


def lambda_handler(event, context):
    results = []
    for asked in event["calls"]:
        try:
            answered = getattr(SCHEDULER, asked["method"])(**asked["parameters"])
        except botocore.exceptions.UnknownServiceError as error:
            results.append({"ok": False, "unknown_service": str(error)})
            continue
        except botocore.exceptions.ClientError as error:
            results.append(
                {
                    "ok": False,
                    "code": error.response["Error"]["Code"],
                    "message": error.response["Error"].get("Message"),
                    "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
                }
            )
            continue
        except botocore.exceptions.SdkCallNotMade as error:
            results.append({"ok": False, "not_made": str(error)})
            continue
        results.append(
            {
                "ok": True,
                "status": answered["ResponseMetadata"]["HTTPStatusCode"],
                "answered": _reported({
                    name: value
                    for name, value in answered.items()
                    if name != "ResponseMetadata"
                }),
            }
        )
    return {"results": results}
"#;

/// One served row: the client method the handler calls, the input it carries, the output members
/// whose value is known, and the ones the service itself decides.
struct Row {
    case: &'static str,
    method: &'static str,
    parameters: Value,
    /// Each known output member: where it sits in the answered document, and its value.
    answered: Vec<(&'static [&'static str], Value)>,
    /// Each output member the service itself fills in, which must be there and carry a value.
    decided: Vec<&'static [&'static str]>,
}

/// One refused row: the call, the error code the service refuses it with, the HTTP status that
/// goes with it, and the text its sentence has to carry.
struct Refused {
    case: &'static str,
    method: &'static str,
    parameters: Value,
    code: &'static str,
    status: u16,
    names: Vec<String>,
}

/// The ARN of a schedule of this account in this Region, as the model's `ScheduleArn` pattern
/// spells one.
fn schedule_arn(group: &str, name: &str) -> String {
    format!("arn:aws:scheduler:{REGION}:{ACCOUNT}:schedule/{group}/{name}")
}

/// The ARN of a schedule group, as the model's `ScheduleGroupArn` pattern spells one.
fn group_arn(name: &str) -> String {
    format!("arn:aws:scheduler:{REGION}:{ACCOUNT}:schedule-group/{name}")
}

/// The ARN of an IAM role of this account, as IAM records it.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The target every schedule carries: the queue it would deliver to, and the execution role
/// EventBridge Scheduler would act as.
fn target() -> Value {
    json!({ "Arn": TARGET_QUEUE, "RoleArn": role_arn(TARGET_ROLE) })
}

/// The identity policy that grants the caller role every action of this service plus the pass
/// of the schedules' execution role — the two permissions a schedule-writing handler needs.
fn grant_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "scheduler:*", "Resource": "*" },
            { "Effect": "Allow", "Action": "iam:PassRole", "Resource": role_arn(TARGET_ROLE) },
        ],
    })
    .to_string()
}

/// The trust policy a role assumed by `principal` carries.
fn trust_document(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": principal},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The served rows, in the order one invocation makes them: each later row stands on what the
/// earlier ones did, which is what puts the schedule the reads report into the group the create
/// made.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a group create carrying a required label and a list of tag structures",
            method: "create_schedule_group",
            parameters: json!({
                "Name": GROUP,
                "Tags": [{ "Key": TAG_KEY, "Value": TAG_VALUE }],
            }),
            answered: vec![(&["ScheduleGroupArn"], json!(group_arn(GROUP)))],
            decided: Vec::new(),
        },
        Row {
            case: "a schedule create in that group, carrying a rate expression",
            method: "create_schedule",
            parameters: json!({
                "Name": RECURRING,
                "GroupName": GROUP,
                "ScheduleExpression": RATE_EXPRESSION,
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target(),
            }),
            answered: vec![(
                &["ScheduleArn"],
                json!(schedule_arn(GROUP, RECURRING)),
            )],
            decided: Vec::new(),
        },
        Row {
            case: "a schedule create that names no group, carrying an at expression, a zone, a \
                   state and a flexible window",
            method: "create_schedule",
            parameters: json!({
                "Name": ONE_OFF,
                "ScheduleExpression": AT_EXPRESSION,
                "ScheduleExpressionTimezone": TIMEZONE,
                "Description": DESCRIPTION,
                "State": "DISABLED",
                "FlexibleTimeWindow": {
                    "Mode": "FLEXIBLE",
                    "MaximumWindowInMinutes": WINDOW_MINUTES,
                },
                "Target": target(),
            }),
            // A create that names no group puts the schedule in the account's built-in one.
            answered: vec![(
                &["ScheduleArn"],
                json!(schedule_arn(DEFAULT_GROUP, ONE_OFF)),
            )],
            decided: Vec::new(),
        },
        Row {
            case: "a read of the schedule the create wrote",
            method: "get_schedule",
            parameters: json!({ "Name": RECURRING, "GroupName": GROUP }),
            answered: vec![
                (&["Arn"], json!(schedule_arn(GROUP, RECURRING))),
                (&["Name"], json!(RECURRING)),
                (&["GroupName"], json!(GROUP)),
                (&["ScheduleExpression"], json!(RATE_EXPRESSION)),
                // The create named no `State`, and a schedule takes `ENABLED` then.
                (&["State"], json!("ENABLED")),
                (&["Target", "Arn"], json!(TARGET_QUEUE)),
                (&["FlexibleTimeWindow", "Mode"], json!("OFF")),
            ],
            decided: vec![&["CreationDate"], &["LastModificationDate"]],
        },
        Row {
            case: "a listing of that group's schedules",
            method: "list_schedules",
            parameters: json!({ "GroupName": GROUP }),
            answered: vec![
                (&["Schedules", "0", "Name"], json!(RECURRING)),
                (&["Schedules", "0", "GroupName"], json!(GROUP)),
                (
                    &["Schedules", "0", "Arn"],
                    json!(schedule_arn(GROUP, RECURRING)),
                ),
            ],
            decided: vec![&["Schedules"]],
        },
        Row {
            case: "an update moving that schedule onto a cron expression and disabling it",
            method: "update_schedule",
            parameters: json!({
                "Name": RECURRING,
                "GroupName": GROUP,
                "ScheduleExpression": CRON_EXPRESSION,
                "State": "DISABLED",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target(),
            }),
            answered: vec![(
                &["ScheduleArn"],
                json!(schedule_arn(GROUP, RECURRING)),
            )],
            decided: Vec::new(),
        },
        Row {
            case: "a read of the updated schedule",
            method: "get_schedule",
            parameters: json!({ "Name": RECURRING, "GroupName": GROUP }),
            answered: vec![
                (&["ScheduleExpression"], json!(CRON_EXPRESSION)),
                (&["State"], json!("DISABLED")),
            ],
            decided: vec![&["LastModificationDate"]],
        },
        Row {
            case: "a delete of the schedule in the built-in group",
            method: "delete_schedule",
            parameters: json!({ "Name": ONE_OFF }),
            // `DeleteScheduleOutput` declares no member at all, so a served call answers the
            // empty document beside the SDK's own response metadata.
            answered: vec![(&[], json!({}))],
            decided: Vec::new(),
        },
        Row {
            case: "a read of the group the create minted",
            method: "get_schedule_group",
            parameters: json!({ "Name": GROUP }),
            answered: vec![
                (&["Arn"], json!(group_arn(GROUP))),
                (&["Name"], json!(GROUP)),
                (&["State"], json!("ACTIVE")),
            ],
            decided: vec![&["CreationDate"]],
        },
    ]
}

/// The refused rows, which one further invocation makes: each is a fault of the REQUEST, so each
/// is the service's own modelled refusal.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a create naming a schedule group the account does not hold",
            method: "create_schedule",
            parameters: json!({
                "Name": RECURRING,
                "GroupName": ABSENT_GROUP,
                "ScheduleExpression": RATE_EXPRESSION,
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target(),
            }),
            code: NOT_FOUND,
            status: 404,
            names: vec![format!("Schedule group {ABSENT_GROUP} does not exist.")],
        },
        Refused {
            case: "a read naming a schedule the group does not hold",
            method: "get_schedule",
            parameters: json!({ "Name": ABSENT_SCHEDULE, "GroupName": GROUP }),
            code: NOT_FOUND,
            status: 404,
            names: vec![format!("Schedule {ABSENT_SCHEDULE} does not exist.")],
        },
        Refused {
            case: "a delete of the schedule the served calls already deleted",
            method: "delete_schedule",
            parameters: json!({ "Name": ONE_OFF }),
            code: NOT_FOUND,
            status: 404,
            names: vec![format!("Schedule {ONE_OFF} does not exist.")],
        },
        Refused {
            case: "a create whose window members contradict each other",
            method: "create_schedule",
            parameters: json!({
                "Name": "windowed",
                "GroupName": GROUP,
                "ScheduleExpression": RATE_EXPRESSION,
                "FlexibleTimeWindow": {
                    "Mode": "OFF",
                    "MaximumWindowInMinutes": WINDOW_MINUTES,
                },
                "Target": target(),
            }),
            code: VALIDATION,
            status: 400,
            names: vec!["MaximumWindowInMinutes".to_string()],
        },
        Refused {
            case: "a group create naming the group the account already owns",
            method: "create_schedule_group",
            parameters: json!({ "Name": DEFAULT_GROUP }),
            code: VALIDATION,
            status: 400,
            names: vec![format!(
                "This operation cannot be performed on schedule group {DEFAULT_GROUP}."
            )],
        },
    ]
}

/// The rows a third invocation makes once the role's grant is withdrawn: the action each one
/// performs, and the resource IAM judges it on — one addressing a schedule, one addressing a
/// whole group's resource, and one addressing nothing of its own, which is where IAM writes `*`.
fn withheld_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a create the execution role is no longer granted",
            method: "create_schedule",
            parameters: json!({
                "Name": "withheld",
                "GroupName": GROUP,
                "ScheduleExpression": RATE_EXPRESSION,
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target(),
            }),
            code: ACCESS_DENIED,
            status: 403,
            names: vec![denial_sentence(
                "CreateSchedule",
                &schedule_arn(GROUP, "withheld"),
            )],
        },
        Refused {
            case: "a listing the execution role is no longer granted",
            method: "list_schedules",
            parameters: json!({ "GroupName": GROUP }),
            code: ACCESS_DENIED,
            status: 403,
            names: vec![denial_sentence("ListSchedules", "*")],
        },
        Refused {
            case: "a read the execution role is no longer granted",
            method: "get_schedule",
            parameters: json!({ "Name": RECURRING, "GroupName": GROUP }),
            code: ACCESS_DENIED,
            status: 403,
            names: vec![denial_sentence(
                "GetSchedule",
                &schedule_arn(GROUP, RECURRING),
            )],
        },
        Refused {
            case: "a group read the execution role is no longer granted",
            method: "get_schedule_group",
            parameters: json!({ "Name": GROUP }),
            code: ACCESS_DENIED,
            status: 403,
            names: vec![denial_sentence("GetScheduleGroup", &group_arn(GROUP))],
        },
    ]
}

/// What AWS's authorization layer refuses a call no identity-based policy allows with, from the
/// action the call performs and the resource it addressed — the part of the sentence that does
/// not depend on which session of the role the runtime minted.
fn denial_sentence(operation: &str, resource: &str) -> String {
    format!(
        "is not authorized to perform: scheduler:{operation} on resource: {resource} because no \
         identity-based policy allows the scheduler:{operation} action"
    )
}

/// The value at `place` of `document`, where a numeric step indexes an array.
fn at<'a>(document: &'a Value, place: &[&str]) -> &'a Value {
    let mut read = document;
    for step in place {
        read = match step.parse::<usize>() {
            Ok(index) => &read[index],
            Err(_not_an_index) => &read[*step],
        };
    }
    read
}

/// Invoke the function with `event` and answer the payload, asserting the invocation itself
/// answered 200 with no function error — the handler reports a refusal in its own payload, so a
/// function error here is the crash the runtime must not produce.
async fn invoked(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        event,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} ({})",
        answered.status,
        answered.text()
    );
    let signal = answered.header("x-amz-function-error");
    assert_eq!(
        signal, None,
        "case {case:?}: expected no FunctionError, got {signal:?} with {}",
        answered.text()
    );
    answered.json()
}

/// The event one invocation carries for `calls`.
fn event(calls: &[Value]) -> Value {
    json!({ "calls": calls })
}

/// The results of one invocation of `asked`, one per call, in the order they were made.
async fn results(router: &MultiServiceRouter, case: &str, asked: &[Value]) -> Vec<Value> {
    let payload = invoked(router, case, &event(asked)).await;
    let results = payload["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        asked.len(),
        "case {case:?}: expected one result per call, got {payload}"
    );
    results
}

/// The refused rows of `rows`, asserted against what the handler was told.
fn assert_refused(rows: &[Refused], results: &[Value]) {
    for (row, result) in rows.iter().zip(results) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(false),
            "case {case:?}: expected the call to be refused, got {result}"
        );
        assert_eq!(
            result["code"],
            json!(row.code),
            "case {case:?}: expected the error code {:?}, got {result}",
            row.code
        );
        assert_eq!(
            result["status"],
            json!(row.status),
            "case {case:?}: expected HTTP {} beside the code, got {result}",
            row.status
        );
        let message = result["message"].as_str().unwrap_or_default();
        for named in &row.names {
            assert!(
                message.contains(named.as_str()),
                "case {case:?}: expected the refusal's sentence to carry {named:?}, got \
                 {message:?}"
            );
        }
    }
}

/// RULE: a handler reaches EventBridge Scheduler through the SDK its runtime supplies, so what
/// its call answers is that service's own answer — and a call the execution role's policies do
/// not allow is refused with that service's authorization refusal and performs nothing.
#[tokio::test]
async fn a_running_handler_writes_and_reads_schedules_through_its_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    // The role the function runs as, written with IAM's own operations, so the session the
    // runtime mints speaks for the record `iam:CreateRole` wrote.
    iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", CALLER_ROLE),
            (
                "AssumeRolePolicyDocument",
                &trust_document("lambda.amazonaws.com"),
            ),
        ],
    )
    .await;
    iam(
        &router,
        "AttachRolePolicy",
        &[
            ("RoleName", CALLER_ROLE),
            ("PolicyArn", BASIC_EXECUTION_POLICY),
        ],
    )
    .await;
    iam(
        &router,
        "PutRolePolicy",
        &[
            ("RoleName", CALLER_ROLE),
            ("PolicyName", INLINE_POLICY),
            ("PolicyDocument", &grant_document()),
        ],
    )
    .await;
    // The role the SCHEDULES run as: EventBridge Scheduler resolves it when a schedule is
    // created, so it has to exist and trust the Scheduler service principal.
    iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", TARGET_ROLE),
            (
                "AssumeRolePolicyDocument",
                &trust_document(SCHEDULER_PRINCIPAL),
            ),
        ],
    )
    .await;

    // The function itself, running as that role.
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": role_arn(CALLER_ROLE),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected HTTP 201 from CreateFunction, got {} ({})",
        created.status,
        created.text()
    );
    await_function_active(&router, "setup", FUNCTION).await;

    // ONE invocation makes every served call, in order.
    let rows = rows();
    let asked: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "method": row.method, "parameters": row.parameters }))
        .collect();
    let served = results(&router, "the served calls", &asked).await;

    for (row, result) in rows.iter().zip(&served) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(true),
            "case {case:?}: expected the handler's call to be served, got {result}"
        );
        assert_eq!(
            result["status"],
            json!(200),
            "case {case:?}: expected the SDK to report HTTP 200, got {result}"
        );
        for (place, expected) in &row.answered {
            let read = at(&result["answered"], place);
            assert_eq!(
                read, expected,
                "case {case:?}: expected the output member {place:?} to be {expected}, got \
                 {result}"
            );
        }
        for place in &row.decided {
            let read = at(&result["answered"], place);
            assert!(
                !read.is_null(),
                "case {case:?}: expected the service to fill in the output member {place:?}, got \
                 {result}"
            );
        }
    }

    // The listing answered the group's schedules and nothing else: the group holds exactly the
    // one schedule the create put in it, so a listing that leaked the built-in group's schedule
    // would carry two.
    let listed = served[4]["answered"]["Schedules"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case \"a listing of that group's schedules\": expected the one schedule that group \
         holds, got {listed:?}"
    );

    // The other half of each served row: what EventBridge Scheduler itself holds, read where a
    // client reads it.
    let read = scheduler(
        &router,
        "GET",
        &format!("/schedules/{RECURRING}?groupName={GROUP}"),
    )
    .await;
    assert_eq!(
        read["ScheduleExpression"],
        json!(CRON_EXPRESSION),
        "case \"an update moving that schedule onto a cron expression and disabling it\": \
         expected the service to report the expression the update carried, got {read}"
    );
    assert_eq!(
        read["State"],
        json!("DISABLED"),
        "case \"an update moving that schedule onto a cron expression and disabling it\": \
         expected the service to report the state the update carried, got {read}"
    );
    assert_eq!(
        read["Target"]["RoleArn"],
        json!(role_arn(TARGET_ROLE)),
        "case \"a schedule create in that group, carrying a rate expression\": expected the \
         service to report the execution role the handler passed, got {read}"
    );
    let group = scheduler(&router, "GET", &format!("/schedule-groups/{GROUP}")).await;
    assert_eq!(
        group["Arn"],
        json!(group_arn(GROUP)),
        "case \"a group create carrying a required label and a list of tag structures\": \
         expected the service to report the group the handler created, got {group}"
    );
    let tags = scheduler(
        &router,
        "GET",
        &format!("/tags/{}", group_arn(GROUP).replace(':', "%3A").replace('/', "%2F")),
    )
    .await;
    assert_eq!(
        tags["Tags"][0]["Key"],
        json!(TAG_KEY),
        "case \"a group create carrying a required label and a list of tag structures\": \
         expected the tag the create carried, got {tags}"
    );
    // The delete really removed the schedule in the built-in group: the service answers 404 for
    // it, which is also what the refused rows below assert the handler is told.
    let (status, gone) = scheduler_call(&router, "GET", &format!("/schedules/{ONE_OFF}")).await;
    assert_eq!(
        status, 404,
        "case \"a delete of the schedule in the built-in group\": expected the service to hold \
         no such schedule, got {status} with {gone}"
    );

    // A second invocation makes the calls the service itself refuses.
    let refused = refused_rows();
    let asked: Vec<Value> = refused
        .iter()
        .map(|row| json!({ "method": row.method, "parameters": row.parameters }))
        .collect();
    let answered = results(&router, "the refused calls", &asked).await;
    assert_refused(&refused, &answered);

    // A third invocation runs with the grant WITHDRAWN from the role: the call a handler makes
    // is the role's, so the authorization layer refuses it in AWS's own words.
    iam(
        &router,
        "DeleteRolePolicy",
        &[("RoleName", CALLER_ROLE), ("PolicyName", INLINE_POLICY)],
    )
    .await;
    let withheld = withheld_rows();
    let asked: Vec<Value> = withheld
        .iter()
        .map(|row| json!({ "method": row.method, "parameters": row.parameters }))
        .collect();
    let answered = results(&router, "the withheld calls", &asked).await;
    assert_refused(&withheld, &answered);
    // The sentence names the PRINCIPAL the call was made by: the session the runtime minted for
    // the function's execution role.
    for (row, result) in withheld.iter().zip(&answered) {
        let message = result["message"].as_str().unwrap_or_default();
        let expected = format!("User: arn:aws:sts::{ACCOUNT}:assumed-role/{CALLER_ROLE}/");
        assert!(
            message.starts_with(expected.as_str()),
            "case {:?}: expected the refusal to name the role's session, got {message:?}",
            row.case
        );
    }

    // And nothing the refused or withheld calls asked for was performed: the group holds exactly
    // the schedule the granted create wrote.
    let after = scheduler(&router, "GET", &format!("/schedules?ScheduleGroup={GROUP}")).await;
    let held = after["Schedules"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        held.len(),
        1,
        "case \"a create the execution role is no longer granted\": expected the refused calls \
         to leave the group as it was, got {after}"
    );
    assert_eq!(
        held[0]["Name"],
        json!(RECURRING),
        "case \"a create the execution role is no longer granted\": expected the one schedule \
         the granted create wrote, got {after}"
    );
}
