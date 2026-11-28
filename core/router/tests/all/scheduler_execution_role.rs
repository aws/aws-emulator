// SPDX-License-Identifier: CC0-1.0

//! EventBridge Scheduler resolves a schedule's execution role against IAM.
//!
//! The rule under test: `Target$RoleArn` is "the IAM role that EventBridge Scheduler will
//! use for this target when the schedule is invoked", so the reference is resolved where it
//! is TAKEN UP — when the schedule is CREATED, and again when it fires — and a
//! `CreateSchedule` is refused with HTTP 400 `ValidationException`, persisting nothing,
//! unless that role exists in this account and its trust policy lets the Scheduler service
//! principal assume it. `UpdateSchedule` does not resolve it: an update states the
//! schedule's whole configuration and is applied with the role it carries, assumable or
//! not, which is what leaves a client the one recovery it has for a schedule whose role was
//! deleted — re-pointing it at a live one.
//!
//! This suite needs TWO services on one engine — the scheduler reaches IAM
//! through the in-process callback, so a single-service router has no IAM to ask
//! — which is why it lives here rather than in either service's own crate. The
//! rows span what the rule generalises over: the trust policy's shape (a single
//! `Action`/`Service` string, each as a list, a `Deny` statement, a policy
//! naming only another service), where the role lives (the default path and a
//! role under a path), which account the ARN names, whether the role was ever
//! created or has since been deleted, and both operations that take a `Target`
//! (`CreateSchedule`, which resolves the role, and `UpdateSchedule`, which does not).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built
//! — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The refusal AWS answers for an execution role Scheduler cannot assume
/// (recorded message; its code and status are what a client branches on).
const ROLE_REFUSAL: &str =
    "The execution role you provide must allow AWS EventBridge Scheduler to assume the role.";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
/// The signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// A trust policy with one statement, built from the pieces that vary between
/// rows: the effect, the action(s) and the service principal(s).
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an
/// `awsQuery` form body as written.
fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// A trust policy with TWO statements about the same principal and action: one
/// `Allow` and one explicit `Deny`. IAM's explicit deny wins, so a role carrying
/// this policy cannot be assumed.
fn allow_then_deny_policy(service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Principal\":{{\"Service\":{service}}},\
         \"Action\":\"sts:AssumeRole\"}},\
         {{\"Effect\":\"Deny\",\"Principal\":{{\"Service\":{service}}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// One IAM role the table needs, as `CreateRole` parameters.
struct RoleFixture {
    role_name: &'static str,
    path: Option<&'static str>,
    policy: String,
}

/// The roles the rows below name. Each trust policy differs in the shape AWS
/// accepts for `Action` and `Principal.Service` (one string, or a list), in its
/// effect, or in the principal it names.
fn role_fixtures() -> Vec<RoleFixture> {
    vec![
        RoleFixture {
            role_name: "scheduler-role",
            path: None,
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"scheduler.amazonaws.com\"",
            ),
        },
        RoleFixture {
            role_name: "scheduler-role-lists",
            path: None,
            policy: trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"events.amazonaws.com\",\"scheduler.amazonaws.com\"]",
            ),
        },
        RoleFixture {
            role_name: "scheduler-role-wildcard",
            path: None,
            policy: trust_policy("Allow", "\"sts:*\"", "\"scheduler.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "path-scoped-role",
            path: Some("/service-role/"),
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"scheduler.amazonaws.com\"",
            ),
        },
        RoleFixture {
            role_name: "lambda-only-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "denied-role",
            path: None,
            policy: trust_policy("Deny", "\"sts:AssumeRole\"", "\"scheduler.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "allow-then-deny-role",
            path: None,
            policy: allow_then_deny_policy("\"scheduler.amazonaws.com\""),
        },
    ]
}

/// One row: the execution role a `CreateSchedule` names, and whether the
/// schedule must be created.
struct RoleCase {
    case: &'static str,
    /// The schedule's name, so each row writes its own resource.
    schedule: &'static str,
    /// The `Target.RoleArn` the request carries.
    role_arn: String,
    /// Whether the schedule must be persisted (`true`) or refused (`false`).
    accepted: bool,
}

/// The role ARN of a role this account holds, under the default path.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

fn role_cases() -> Vec<RoleCase> {
    vec![
        RoleCase {
            case: "a role that exists and trusts Scheduler",
            schedule: "trusted",
            role_arn: role_arn("scheduler-role"),
            accepted: true,
        },
        RoleCase {
            case: "a trust policy whose Action and Service are lists",
            schedule: "trusted-lists",
            role_arn: role_arn("scheduler-role-lists"),
            accepted: true,
        },
        RoleCase {
            case: "a trust policy allowing the whole sts namespace",
            schedule: "trusted-wildcard",
            role_arn: role_arn("scheduler-role-wildcard"),
            accepted: true,
        },
        RoleCase {
            case: "a role under a path",
            schedule: "path-scoped",
            role_arn: format!(
                "arn:aws:iam::{}:role/service-role/path-scoped-role",
                engine::emulator_account_id()
            ),
            accepted: true,
        },
        RoleCase {
            case: "a role that was never created",
            schedule: "absent-role",
            role_arn: role_arn("never-created-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role that trusts another service only",
            schedule: "lambda-role",
            role_arn: role_arn("lambda-only-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role whose statement denies the assume",
            schedule: "denied-role",
            role_arn: role_arn("denied-role"),
            accepted: false,
        },
        RoleCase {
            // IAM's explicit deny wins over an Allow for the same principal.
            case: "a role that both allows and explicitly denies the assume",
            schedule: "allow-then-deny",
            role_arn: role_arn("allow-then-deny-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role of another account",
            schedule: "foreign-role",
            role_arn: "arn:aws:iam::123456789012:role/scheduler-role".to_string(),
            accepted: false,
        },
    ]
}

/// A `CreateSchedule`/`UpdateSchedule` body naming `role_arn`.
fn schedule_body(role_arn: &str) -> serde_json::Value {
    serde_json::json!({
        "ScheduleExpression": "rate(1 day)",
        "State": "DISABLED",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "Target": {
            "Arn": format!("arn:aws:sqs:{REGION}:{}:placeholder", engine::emulator_account_id()),
            "RoleArn": role_arn,
        }
    })
}

/// Send a request and return the status with the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action: `POST /` with the parameters in the form
/// body. Every value here is free of `&`, `=`, `+` and `%`, so it needs no
/// escaping.
fn iam(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    send(
        router,
        runtime,
        "iam",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
}

/// Drive one scheduler request, returning the status and the parsed JSON body.
fn scheduler(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    let (status, text) = send(
        router,
        runtime,
        "scheduler",
        method,
        uri,
        "application/json",
        payload,
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The restJson1 error code, without its namespace.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(body: &serde_json::Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[test]
fn a_schedule_is_written_only_for_an_execution_role_iam_holds_and_scheduler_may_assume() {
    std::thread::Builder::new()
        .name("scheduler-execution-role".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The time driver, because the emulator's services run CLOCKS: a service
                // whose state owes a transition spawns a background task that waits on a
                // timer, and a timer on a runtime built without this panics the task — and,
                // on a current-thread runtime, that panic unwinds out of whichever
                // `block_on` happened to poll it. Enabling it is what lets those clocks run
                // here, and it changes nothing this suite asserts.
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            for fixture in role_fixtures() {
                let mut parameters = vec![
                    ("RoleName", fixture.role_name),
                    ("AssumeRolePolicyDocument", fixture.policy.as_str()),
                ];
                if let Some(path) = fixture.path {
                    parameters.push(("Path", path));
                }
                let (status, body) = iam(&router, &runtime, "CreateRole", &parameters);
                assert_eq!(
                    status, 200,
                    "setup: CreateRole {}: got {status} {body}",
                    fixture.role_name
                );
            }

            for case in role_cases() {
                let (status, body) = scheduler(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/schedules/{}", case.schedule),
                    Some(schedule_body(&case.role_arn)),
                );
                let read = scheduler(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/schedules/{}", case.schedule),
                    None,
                );
                if case.accepted {
                    assert_eq!(
                        status, 200,
                        "case {:?}: CreateSchedule must succeed, got {status} {body}",
                        case.case
                    );
                    assert_eq!(
                        read.0, 200,
                        "case {:?}: the schedule must be persisted, got {} {}",
                        case.case, read.0, read.1
                    );
                    assert_eq!(
                        read.1
                            .get("Target")
                            .and_then(|target| target.get("RoleArn"))
                            .and_then(serde_json::Value::as_str),
                        Some(case.role_arn.as_str()),
                        "case {:?}: the stored target must carry the role it was given, got {}",
                        case.case,
                        read.1
                    );
                } else {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "ValidationException"),
                        "case {:?}: an unassumable execution role must be refused 400 \
                         ValidationException, got {status} {body}",
                        case.case
                    );
                    assert_eq!(
                        error_message(&body),
                        ROLE_REFUSAL,
                        "case {:?}: expected the execution-role refusal, got {body}",
                        case.case
                    );
                    assert_eq!(
                        (read.0, error_code(&read.1).as_str()),
                        (404, "ResourceNotFoundException"),
                        "case {:?}: a refused create must persist nothing, got {} {}",
                        case.case,
                        read.0,
                        read.1
                    );
                }
            }

            // `UpdateSchedule` does NOT apply that gate: the role is resolved where it is
            // taken up — on the create above and at each occurrence — so an update is served
            // and APPLIED with the role it states. Every row is a role the create path above
            // refused, one of them the schedule's OWN role deleted out of band, and each is
            // applied by the update while a create naming it is still refused, so the
            // asymmetry belongs to the role reference rather than to one request.
            let update_cases: [(&str, String); 3] = [
                // The role the schedule was created with, DELETED out of band and
                // then re-affirmed by the update.
                ("the deleted role, re-affirmed", role_arn("scheduler-role")),
                ("a role that was never created", role_arn("never-created-role")),
                (
                    "a role of another account",
                    "arn:aws:iam::123456789012:role/scheduler-role".to_string(),
                ),
            ];
            let (status, body) = iam(
                &router,
                &runtime,
                "DeleteRole",
                &[("RoleName", "scheduler-role")],
            );
            assert_eq!(
                status, 200,
                "setup: DeleteRole scheduler-role: got {status} {body}"
            );

            for (index, (case, role_arn)) in update_cases.into_iter().enumerate() {
                let (status, body) = scheduler(
                    &router,
                    &runtime,
                    "PUT",
                    "/schedules/trusted",
                    Some(schedule_body(&role_arn)),
                );
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateSchedule must apply a role it does not resolve, \
                     got {status} {body}"
                );
                let (status, body) =
                    scheduler(&router, &runtime, "GET", "/schedules/trusted", None);
                assert_eq!(
                    (
                        status,
                        body.get("Target")
                            .and_then(|target| target.get("RoleArn"))
                            .and_then(serde_json::Value::as_str)
                    ),
                    (200, Some(role_arn.as_str())),
                    "case {case:?}: an applied update must store the role it stated, \
                     got {status} {body}"
                );

                // The CREATE path refuses that same role, so what the update applied is a
                // reference this account cannot assume — the gate is the create's.
                let (status, body) = scheduler(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/schedules/created-with-row-{index}"),
                    Some(schedule_body(&role_arn)),
                );
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: CreateSchedule must still refuse an unassumable role, \
                     got {status} {body}"
                );
                assert_eq!(
                    error_message(&body),
                    ROLE_REFUSAL,
                    "case {case:?}: expected the execution-role refusal, got {body}"
                );
            }

            // The recovery an update being served is what leaves open: a schedule standing
            // on a role nothing holds is re-pointed at one this account does hold, and the
            // read reports it.
            let assumable = role_arn("scheduler-role-lists");
            let (status, body) = scheduler(
                &router,
                &runtime,
                "PUT",
                "/schedules/trusted",
                Some(schedule_body(&assumable)),
            );
            assert_eq!(
                status, 200,
                "case \"an assumable role\": UpdateSchedule must be served, got {status} \
                 {body}"
            );
            let (status, body) = scheduler(&router, &runtime, "GET", "/schedules/trusted", None);
            assert_eq!(
                (
                    status,
                    body.get("Target")
                        .and_then(|target| target.get("RoleArn"))
                        .and_then(serde_json::Value::as_str)
                ),
                (200, Some(assumable.as_str())),
                "case \"an assumable role\": the update must have applied, got {status} {body}"
            );
        })
        .expect("spawn the execution-role thread")
        .join()
        .expect("every execution role must be resolved against IAM");
}
