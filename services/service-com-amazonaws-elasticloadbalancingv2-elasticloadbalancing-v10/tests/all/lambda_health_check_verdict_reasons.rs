// SPDX-License-Identifier: CC0-1.0

//! WHY a `lambda` target is reported unhealthy: the `TargetHealth$Reason` and
//! `TargetHealth$Description` `DescribeTargetHealth` serves for a target group whose health
//! check is an invocation of the registered function.
//!
//! The rule under test: **a health check that was ANSWERED is judged by the status the answer
//! carried, so a target whose answer the group's `Matcher` rejects is reported with the model's
//! own reason for that — "`Target.ResponseCodeMismatch` - The health checks did not return an
//! expected HTTP code" (`TargetHealth$Reason`) — described by the codes those checks really got
//! back, while a check that came back with an ERROR or with an answer that is no response at all
//! has no code to mismatch and is reported with "`Target.FailedHealthChecks` - The load balancer
//! received an error while establishing a connection to the target or the target response was
//! malformed". A target whose answer the matcher accepts is `healthy`, which carries no reason
//! and no description at all ("if the target state is healthy, a description is not
//! provided").**
//!
//! Those two failures are different operational faults with different fixes, so one must not be
//! served as the other: a client reads the reason to tell a function that answered the wrong
//! status from one that faulted or never answered.
//!
//! `TargetGroup$TargetType` gives `lambda` as "Register a single Lambda function as a target",
//! `HealthCheckPath` is "[HTTP/HTTPS health checks] The destination for health checks on the
//! targets" and `Matcher` "[HTTP/HTTPS health checks] The HTTP or gRPC codes to use when
//! checking for a successful response from a target", so the load balancer invokes the function
//! on that destination and judges the status it returns against those codes.
//!
//! The rows differ along what the rule generalises over: the status the function answers (four
//! statuses outside the group's codes), the shape of the codes the matcher names (one code,
//! several, a range), and the class of answer that carries no status at all (a run that raised,
//! an object naming no status, a payload that is no object). Two rows then CHANGE the codes
//! their answer is judged against, so the same answer the group first rejected becomes a
//! success: the verdict follows the comparison rather than the status itself. Every row begins
//! at a verdict only failing checks can reach, and the accepted side is reached from that
//! verdict, so no row is satisfied by the health a registration carries before its checks have
//! decided anything.
//!
//! Nothing here is observed from a record this suite invented: the function is a real deployment
//! package created through Lambda's OWN API and RUN by the Lambda emulator mounted on this
//! suite's engine, which decides from the destination the check requested what it answers, and
//! every verdict is read back through `DescribeTargetHealth`.
//!
//! Nothing is timed by the wall clock either. The spans the clock owes — the function's
//! activation, the load balancer's move to `active`, each registration's settling and the
//! instant its next check is owed at — are PLACED in the store rather than slept for, and every
//! wait is a look repeated until the condition it is about holds inside a budget generous enough
//! that a loaded host cannot decide the outcome. The two members that time a check are left as
//! long as the model admits for the same reason: how fast this host runs the function must not
//! decide whether its answer arrives in time.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{create_load_balancer, settle};
use crate::alb_lambda_target_health_check::make_checks_due;
use crate::alb_lambda_target_published_address::{activate_functions, lambda, HANDLER, RUNTIME};
use crate::provisioning_loop::{action, call, first_text, health_block, members, p};

/// The one function every row registers: it answers what the destination the check requested
/// names, so one package serves every class of answer the rule generalises over.
///
/// `pub(crate)` with the three items below ([`HANDLER_SOURCE`], [`package`], [`emulator`] and
/// [`create_function`]): every suite of this crate whose rows turn on what a health check's
/// invocation ANSWERS needs exactly this function and exactly this mounting
/// (`all/lambda_target_health_state_sequence.rs` among them), so they share one of each rather
/// than drifting on what is mounted or on what the handler answers.
pub(crate) const FUNCTION: &str = "elbv2-hc-verdict-target";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is mounted
/// here, so nothing is known about the role and nothing the function does needs one.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The cadence and the wait every checked group is created with: the LONGEST
/// `HealthCheckIntervalSeconds` admits ("the range is 5-300") and the longest
/// `HealthCheckTimeoutSeconds` admits ("the range is 2-120 seconds"), so no row's verdict depends
/// on how fast this host runs the function. No row waits the cadence out: the instant a
/// registration's next check is owed at is PLACED in the store ([`make_checks_due`]).
const INTERVAL_SECONDS: &str = "300";
const TIMEOUT_SECONDS: &str = "120";

/// How many consecutive checks each verdict takes: the FEWEST either threshold member admits
/// ("the range is 2-10"), because the rule under test is what a verdict is WORDED as and not how
/// many checks it takes, and every one of those checks is a real run of the function.
const THRESHOLD: &str = "2";

/// How long a row waits for the verdict its checks reach before it is a failure, and how long
/// between two looks. Generous on purpose: everything waited for happens in this process, so a
/// loaded host does not decide the outcome.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The `TargetHealthStateEnum` values this suite reads back.
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// The `TargetHealthReasonEnum` values the two failure classes are reported under, as
/// `TargetHealth$Reason` words each: "`Target.ResponseCodeMismatch` - The health checks did not
/// return an expected HTTP code" and "`Target.FailedHealthChecks` - The load balancer received an
/// error while establishing a connection to the target or the target response was malformed".
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";
const FAILED_HEALTH_CHECKS: &str = "Target.FailedHealthChecks";

/// The description the failed-checks reason carries.
const FAILED_HEALTH_CHECKS_DESCRIPTION: &str = "Health checks failed";

/// The handler every row runs. It answers what the LAST SEGMENT of the destination the check
/// requested names: a run that raises for `raise`, an object naming no status for `nostatus`, a
/// payload that is no object for `notanobject`, else the status that segment spells.
pub(crate) const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    choice = (event.get("path") or "/").rsplit("/", 1)[-1]
    if choice == "raise":
        raise RuntimeError("the handler cannot answer this health check")
    if choice == "nostatus":
        return {"statusDescription": "200 OK", "body": "no status at all"}
    if choice == "notanobject":
        return "not a response object at all"
    return {
        "statusCode": int(choice),
        "headers": {"content-type": "text/plain"},
        "body": "OK",
    }
"#;

/// A .zip deployment package holding [`HANDLER_SOURCE`] as `lambda_function.py`, written in
/// memory the way a caller uploads one.
pub(crate) fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// This service and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them: the
/// function a health check invokes is RUN by that Lambda, so what each row's verdict is decided
/// from is a real run rather than a record this suite wrote.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Create the one function through Lambda's own API and answer its ARN.
pub(crate) async fn create_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Code": { "ZipFile": aws_smithy_types::base64::encode(package()) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected 201, got {status} {created}"
    );
    // The activation span Lambda owes a freshly created function, placed rather than slept for.
    activate_functions(engine);
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateFunction: no FunctionArn in {created}"))
        .to_string()
}

/// What `DescribeTargetHealth` reports for one registration: the state, the reason and the
/// description, each read as empty where the answer carries none.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    state: String,
    reason: String,
    description: String,
}

/// The health a target whose answer the matcher ACCEPTS is reported with: the state alone,
/// because "if the target state is healthy, a reason code is not provided" and no description is
/// provided either.
fn healthy() -> Reported {
    Reported {
        state: HEALTHY.to_string(),
        reason: String::new(),
        description: String::new(),
    }
}

/// The health a target that ANSWERED `status` outside its group's codes is reported with: the
/// model's own reason for an answer those codes reject, described by the status the checks got
/// back.
fn mismatched(status: u16) -> Reported {
    Reported {
        state: UNHEALTHY.to_string(),
        reason: RESPONSE_CODE_MISMATCH.to_string(),
        description: format!("Health checks failed with these codes: [{status}]"),
    }
}

/// The health a target whose checks came back with an error or with an answer that is no response
/// is reported with: there is no code to report a mismatch of.
fn failed_checks() -> Reported {
    Reported {
        state: UNHEALTHY.to_string(),
        reason: FAILED_HEALTH_CHECKS.to_string(),
        description: FAILED_HEALTH_CHECKS_DESCRIPTION.to_string(),
    }
}

/// Every registration the target group `arn` holds, as it is reported.
async fn described(router: &MultiServiceRouter, arn: &str) -> Vec<Reported> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "DescribeTargetHealth must answer 200, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            // The health's own element, because the override beside it carries members of
            // the same names ([`health_block`]).
            let health = health_block(description);
            Reported {
                state: first_text(&health, "State").unwrap_or_default(),
                reason: first_text(&health, "Reason").unwrap_or_default(),
                description: first_text(&health, "Description").unwrap_or_default(),
            }
        })
        .collect()
}

/// Look at the target group until its one registration is reported `wanted`, and assert that it
/// is — the whole triple, so a state reached with another reason or another description fails the
/// case.
///
/// The wait is for the CONDITION: every look first places the instant the next check of each
/// registration is owed at ([`make_checks_due`]) and the verdict arrives once as many of those
/// checks have been made as the group's threshold takes.
async fn health_reaching(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    arn: &str,
    wanted: &Reported,
) {
    let deadline = Instant::now() + BUDGET;
    loop {
        make_checks_due(engine);
        let reported = described(router, arn).await;
        let settled = reported.len() == 1 && &reported[0] == wanted;
        if settled || Instant::now() >= deadline {
            assert_eq!(
                reported,
                vec![wanted.clone()],
                "case {case:?}: expected the registration reported {wanted:?}, got {reported:?}"
            );
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the destination the group's health check requests, the codes its matcher names, the
/// health its checks must then reach, and — for a row that goes on to change those codes — the
/// codes it changes them to and the health the checks judged against THOSE must reach.
struct Row {
    case: &'static str,
    group_name: &'static str,
    listener_port: &'static str,
    path: &'static str,
    matcher: &'static str,
    expected: Reported,
    /// `None` for a row that stays as it was created.
    then: Option<(&'static str, Reported)>,
}

/// The rows, spanning what the rule generalises over.
///
/// Every row begins at a verdict only failing checks can reach, so no row is satisfied by the
/// health a registration carries before its checks have decided anything: an `unhealthy` is
/// `UnhealthyThresholdCount` consecutive failures away. The two rows that go on to assert the
/// ACCEPTED side of the rule reach `healthy` from that unhealthy verdict, which is
/// `HealthyThresholdCount` consecutive passing checks away — so the same answer the group first
/// rejected is seen to pass once the group's codes cover it, and the verdict follows the
/// COMPARISON rather than the status itself.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a function answering 500 against the one code the matcher names",
            group_name: "hc-verdict-500",
            listener_port: "80",
            path: "/hc/500",
            matcher: "200",
            expected: mismatched(500),
            then: None,
        },
        Row {
            case: "a function answering 503 against the one code the matcher names",
            group_name: "hc-verdict-503",
            listener_port: "81",
            path: "/hc/503",
            matcher: "200",
            expected: mismatched(503),
            then: None,
        },
        Row {
            case: "a function answering 404, against several codes and then against a range \
                   that covers it",
            group_name: "hc-verdict-404",
            listener_port: "82",
            path: "/hc/404",
            matcher: "200,201",
            expected: mismatched(404),
            then: Some(("400-499", healthy())),
        },
        Row {
            case: "a function answering the top of a range, against one code and then against \
                   that range",
            group_name: "hc-verdict-299",
            listener_port: "83",
            path: "/hc/299",
            matcher: "200",
            expected: mismatched(299),
            then: Some(("200-299", healthy())),
        },
        Row {
            case: "a handler that raises, so the run returned no response at all",
            group_name: "hc-verdict-raise",
            listener_port: "84",
            path: "/hc/raise",
            matcher: "200",
            expected: failed_checks(),
            then: None,
        },
        Row {
            case: "an answer that is an object naming no status",
            group_name: "hc-verdict-nostatus",
            listener_port: "85",
            path: "/hc/nostatus",
            matcher: "200",
            expected: failed_checks(),
            then: None,
        },
        Row {
            case: "an answer that is no response object at all",
            group_name: "hc-verdict-notanobject",
            listener_port: "86",
            path: "/hc/notanobject",
            matcher: "200",
            expected: failed_checks(),
            then: None,
        },
    ]
}

/// Store `matcher` as the codes the group `arn` counts as success, through `ModifyTargetGroup` —
/// the call a client changes a health check with. Nothing else of the group is touched.
async fn modify_matcher(router: &MultiServiceRouter, case: &str, arn: &str, matcher: &str) {
    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Matcher.HttpCode", matcher));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ModifyTargetGroup must answer 200, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.text("HttpCode").as_deref(),
        Some(matcher),
        "case {case:?}: expected the stored matcher {matcher:?}, got {}",
        reply.body
    );
}

/// Create the row's `lambda` target group, grant the load balancer's invocation on it, register
/// the function and route a listener to it — each through the owning service's own API. Answers
/// the group's ARN.
async fn routed_group(
    router: &MultiServiceRouter,
    row: &Row,
    lb_arn: &str,
    function_arn: &str,
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.group_name));
    form.push(p("TargetType", "lambda"));
    form.push(p("HealthCheckEnabled", "true"));
    form.push(p("HealthCheckPath", row.path));
    form.push(p("Matcher.HttpCode", row.matcher));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
    form.push(p("HealthCheckTimeoutSeconds", TIMEOUT_SECONDS));
    form.push(p("HealthyThresholdCount", THRESHOLD));
    form.push(p("UnhealthyThresholdCount", THRESHOLD));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: CreateTargetGroup must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );
    let tg_arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body));

    // The grant that admits the invocation: the function's own resource-based policy, narrowed by
    // `aws:SourceArn` to this target group, written through Lambda's own `AddPermission`.
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        json!({
            "StatementId": format!("elbv2-hc-{}", row.group_name),
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": tg_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: AddPermission must answer 201, got {status} {granted}",
        row.case
    );

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", function_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: RegisterTargets must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );

    // The listener that forwards to the group, which is what starts its health checking.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", row.listener_port));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: CreateListener must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );
    tg_arn
}

/// RULE: a `lambda` target's unhealthy verdict is reported with the condition its checks found —
/// the response-code mismatch an answer the group's codes reject is, described by the codes it
/// really answered, and the failed checks an error or an answer that is no response is — while a
/// target whose answer those codes accept is healthy with no reason and no description.
#[tokio::test]
async fn a_lambda_targets_verdict_names_the_condition_its_health_checks_found() {
    let (router, engine) = emulator();
    let (lb_arn, _dns_name) = create_load_balancer(&router, "lambda-hc-verdict-alb").await;
    let function_arn = create_function(&router, &engine).await;

    // Every row's group is created, granted, registered and routed before anything waits, so the
    // checks of all of them run on their own cadence at the same time.
    let rows = rows();
    let mut arns: Vec<String> = Vec::new();
    for row in &rows {
        arns.push(routed_group(&router, row, &lb_arn, &function_arn).await);
    }
    // The settling spans the clock owes the load balancer and every registration, placed rather
    // than slept for: a reader takes its own `now`, which is later than this call, so neither
    // instant can be read as future.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    for (row, arn) in rows.iter().zip(&arns) {
        health_reaching(&router, &engine, row.case, arn, &row.expected).await;
    }
    // Then the rows that CHANGE the codes their answer is judged against: the same answer the
    // group rejected is a success once its codes cover it, which the checks reach from the
    // unhealthy verdict they had.
    for (row, arn) in rows.iter().zip(&arns) {
        let Some((matcher, expected)) = row.then.as_ref() else {
            continue;
        };
        modify_matcher(&router, row.case, arn, matcher).await;
        health_reaching(&router, &engine, row.case, arn, expected).await;
    }
}
