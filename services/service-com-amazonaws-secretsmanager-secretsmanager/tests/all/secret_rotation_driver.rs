// SPDX-License-Identifier: CC0-1.0

//! A rotation RUNS: `RotateSecret` drives the configured rotation function through the four
//! documented steps, and what that function writes is what the secret ends up holding —
//! end to end with Lambda mounted beside Secrets Manager on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in.
//!
//! The rules these tables assert:
//!
//! * **The invocation needs the function's own grant.** "You also need
//!   `lambda:InvokeFunction` permissions on the rotation function" (`RotateSecret`, this
//!   service's Smithy model), and this service invokes it as its own service principal — so
//!   a function whose resource-based policy does not admit `secretsmanager.amazonaws.com`
//!   cannot be invoked, the rotation cannot be started, and the call is refused with the
//!   400 `AccessDeniedException` without changing anything about the secret.
//! * **A rotation that was started finishes through the function.** Secrets Manager invokes
//!   the function once per step (`createSecret`, `setSecret`, `testSecret`, `finishSecret`),
//!   the function stages its new value under `AWSPENDING` and moves `AWSCURRENT` onto it on
//!   the last step, and `DescribeSecret` then reports `LastRotatedDate` and a
//!   `NextRotationDate` inside the configured schedule's window. Every version the rotation
//!   produced is one the function itself wrote through this service's operations.
//! * **One rotation at a time.** "If the AWSPENDING staging label is present but not
//!   attached to the same version as AWSCURRENT, then any later invocation of RotateSecret
//!   assumes that a previous rotation request is still in progress and returns an error."
//! * **A step that fails leaves the rotation unfinished.** The current value is still
//!   readable, `AWSPENDING` still names the version the rotation staked, and
//!   `LastRotatedDate` stays unset.
//!
//! The rows span what the rules generalise over rather than one recorded request: how the
//! function's grant is written (unscoped, scoped to this secret, scoped to its account,
//! scoped to another secret, granted to another service principal, absent), what the
//! `RotationLambdaARN` is (a function's ARN, a value that names no function), whether the
//! call asks to rotate immediately, how the schedule is stated, and whether the rotation
//! function completes every step.
//!
//! Every row waits for the STATE its outcome settles in rather than for a span to pass: the
//! stamp of a rotation that finishes, the attempt a function that raises records before it
//! does, and the `AWSPENDING` version a call that starts a rotation stakes while answering —
//! so a row whose rotation cannot finish concludes as soon as its outcome is settled.
//!
//! What a row COSTS is kept to what its rule needs, because every build waits for its slowest
//! test. Three things follow from that, and none of them changes what a row asserts.
//!
//! * **The rows that rotate through one function SHARE it.** An execution environment is built
//!   for one function version at one revision and then serves the invocations after it, so the
//!   rows of a table run on one function created once, and what differs per row — the
//!   function's own resource-based policy, which is no part of that environment's identity — is
//!   written onto it as the row comes up.
//! * **A row that invokes nothing creates no function.** A call refused on the
//!   `RotationLambdaARN` it carries never reaches a function, so none is made for it.
//! * **The rows are grouped into tests by the outcome they settle in**, so no test drives more
//!   than one rotation that has to run through all four of its steps.
//!
//! Nothing here drives a clock it could advance instead. A rotation's invocations run on
//! threads of their own, and the spans Lambda bounds them by are real timers around the answer
//! those threads send back, so a paused clock would be advanced to the first of those deadlines
//! the moment a test awaited an invocation — ending every run before its handler was reached.
//! What each wait below looks for is therefore the STATE the rotation reaches, and the ceiling
//! on the looking is only there so a suite cannot hang.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ABANDON_GRACE, HOST_WAIT_CEILING, INIT_PHASE_LIMIT, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{ACTIVATION_DELAY, STATE_ACTIVE};
use service_com_amazonaws_secretsmanager_secretsmanager::state::rotation::ROTATION_STEPS;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request in this suite signs for.
const REGION: &str = "us-west-2";

/// The service principal Secrets Manager invokes a rotation function as.
const SERVICE_PRINCIPAL: &str = "secretsmanager.amazonaws.com";

/// The IAM action an invocation of a function performs.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A well-formed execution role ARN for the rotation function.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/rotation-exec";

/// The handler entry point the deployment package below declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for a rotation to reach the state a row expects of it before
/// giving up on it, and how often it looks while waiting.
///
/// The ceiling is DERIVED rather than picked: a rotation finishes only once Secrets Manager
/// has invoked the rotation function once per documented step, and Lambda may spend its own
/// published bounds on each of those invocations — the wait for the capacity to run in, the
/// initialization of the execution environment, the longest the run itself is waited for and
/// the grace past that
/// (`service_com_amazonaws_lambda_awsgirapiservice::state::execution`). A ceiling below that
/// worth of work is a bet on how fast the host runs; this one is above it, and bounds the
/// suite only so it cannot hang.
const ROTATION_WAIT: Duration = PERMIT_WAIT
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(ABANDON_GRACE)
    .saturating_mul(ROTATION_STEPS.len() as u32);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How long this suite waits for a function it created to report itself invocable.
///
/// Also a ceiling rather than a span to pass: the provisioning window Lambda opens on
/// `CreateFunction` is [`ACTIVATION_DELAY`], and what a row waits for is the `State` the
/// function reports at the end of it, so this only bounds the look-again loop.
const ACTIVATION_WAIT: Duration = ACTIVATION_DELAY.saturating_mul(60);

/// The staging label the version a secret currently serves carries.
const CURRENT_STAGE: &str = "AWSCURRENT";

/// The staging label a rotation stakes on the version it is creating.
const PENDING_STAGE: &str = "AWSPENDING";

/// The tag key [`FAILING_FUNCTION`] records its attempt under, which is the literal that
/// function's own source writes — a rotation that cannot finish leaves nothing else behind,
/// so this is what tells a caller the function RAN and raised.
const ATTEMPTED_STEP_TAG: &str = "attempted-step";

/// Secrets Manager and Lambda over ONE shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Secrets Manager call (`awsJson1_1`), as its status and parsed body.
async fn sm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("authorization", authorization("secretsmanager"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same, requiring the 200 the operation answers.
async fn sm_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = sm(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The same, requiring a refusal, as its status, error code and message.
async fn sm_err(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, parsed) = sm(router, operation, body).await;
    assert_ne!(
        status, 200,
        "case {case:?}: expected {operation} to be refused, got 200 with {parsed}"
    );
    let code = parsed["__type"]
        .as_str()
        .and_then(|raw| raw.split('#').next_back())
        .unwrap_or_default()
        .to_string();
    let message = parsed["message"]
        .as_str()
        .or_else(|| parsed["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
pub(crate) fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the rotation function `name` running `source`, wait out its provisioning window,
/// and answer its ARN.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    source: &str,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} with {created}"
    );
    // "The State [...] field in the response from GetFunctionConfiguration indicate[s] when
    // the function is ready to invoke": the row waits for that CONDITION rather than for the
    // provisioning window to pass, so it goes on as soon as the function can be invoked.
    let deadline = Instant::now() + ACTIVATION_WAIT;
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFunctionConfiguration, got {status} with \
             {configuration}"
        );
        if configuration["State"].as_str() == Some(STATE_ACTIVE) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never reported {STATE_ACTIVE}, got \
             {configuration}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// The `StatementId` every grant in this suite is written under, which is also what
/// [`clear_permission`] addresses to take it away again.
const STATEMENT_ID: &str = "rotation-invoke";

/// Add one statement to `name`'s resource-based policy, as `AddPermission` writes it.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    principal: &str,
    source_arn: Option<&str>,
    source_account: Option<&str>,
) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": INVOKE_ACTION,
        "Principal": principal,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    if let Some(source_account) = source_account {
        body["SourceAccount"] = json!(source_account);
    }
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        &body,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission, got {status} with {granted}"
    );
}

/// Take that statement away again, leaving `name` holding no policy at all: "removing the last
/// statement removes the policy itself", which is the state a function nothing has granted is
/// in.
///
/// The rows of a grant table share one function, so each row clears what the row before it
/// wrote before writing its own. A function already holding no such statement is already in
/// that state, which is the 404 `RemovePermission` answers for it.
async fn clear_permission(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, answered) = lambda(
        router,
        "DELETE",
        &format!("/2015-03-31/functions/{name}/policy/{STATEMENT_ID}"),
        &Value::Null,
    )
    .await;
    assert!(
        status == 204 || status == 404,
        "case {case:?}: expected RemovePermission to remove the statement or report none held, \
         got {status} with {answered}"
    );
}

/// The rotation function every row runs: the four-step rotation AWS documents, written the
/// way the AWS rotation templates are — it reads the secret's own metadata, refuses a step
/// whose version is not staged `AWSPENDING`, stages its new value on `createSecret` and
/// moves `AWSCURRENT` on `finishSecret`, all through this service's own operations.
const ROTATION_FUNCTION: &str = r#"
import boto3

SUFFIX = "-rotated"


def lambda_handler(event, context):
    arn = event["SecretId"]
    token = event["ClientRequestToken"]
    step = event["Step"]
    client = boto3.client("secretsmanager")
    metadata = client.describe_secret(SecretId=arn)
    if not metadata.get("RotationEnabled"):
        raise ValueError("Secret %s is not enabled for rotation" % arn)
    versions = metadata["VersionIdsToStages"]
    if token not in versions:
        raise ValueError(
            "Secret version %s has no stage for rotation of secret %s." % (token, arn))
    if "AWSCURRENT" in versions[token]:
        return {"step": step, "already_current": True}
    if "AWSPENDING" not in versions[token]:
        raise ValueError(
            "Secret version %s not set as AWSPENDING for rotation of secret %s." % (token, arn))
    if step == "createSecret":
        current = client.get_secret_value(SecretId=arn, VersionStage="AWSCURRENT")
        client.put_secret_value(
            SecretId=arn,
            ClientRequestToken=token,
            SecretString=current["SecretString"] + SUFFIX,
            VersionStages=["AWSPENDING"],
        )
    elif step == "setSecret":
        pass
    elif step == "testSecret":
        client.get_secret_value(SecretId=arn, VersionId=token)
    elif step == "finishSecret":
        previous = None
        for version, stages in versions.items():
            if "AWSCURRENT" in stages:
                previous = version
        client.update_secret_version_stage(
            SecretId=arn,
            VersionStage="AWSCURRENT",
            MoveToVersionId=token,
            RemoveFromVersionId=previous,
        )
    else:
        raise ValueError("Invalid step parameter %s" % step)
    return {"step": step}
"#;

/// A rotation function that raises on the step the event names, so a rotation driven through
/// it stops there. `createSecret` is the step it fails, which is the first one.
///
/// It records the attempt before it raises — a tag under [`ATTEMPTED_STEP_TAG`] naming the
/// step it was invoked for, written through this service's own operations — so that a
/// rotation which ends unfinished is still something a caller can WAIT FOR: once the step
/// that raises has run, the rotation cannot reach `finishSecret`, and what the secret holds
/// is settled.
const FAILING_FUNCTION: &str = r#"
import boto3


def lambda_handler(event, context):
    boto3.client("secretsmanager").tag_resource(
        SecretId=event["SecretId"],
        Tags=[{"Key": "attempted-step", "Value": event["Step"]}],
    )
    raise RuntimeError("this rotation function cannot rotate %s" % event["Step"])
"#;

/// What `DescribeSecret` reports for `secret_id`.
async fn described(router: &MultiServiceRouter, case: &str, secret_id: &str) -> Value {
    sm_ok(router, case, "DescribeSecret", &json!({"SecretId": secret_id})).await
}

/// Wait until the rotation of `secret_id` has finished — `LastRotatedDate` stamped and no
/// version other than the `AWSCURRENT` one holding `AWSPENDING` — and answer what
/// `DescribeSecret` reported last.
async fn rotated(router: &MultiServiceRouter, case: &str, secret_id: &str) -> Value {
    let looks = ROTATION_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        last = described(router, case, secret_id).await;
        if last.get("LastRotatedDate").is_some() {
            return last;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    last
}

/// Wait until the rotation function has RUN for `secret_id` — its attempt recorded under
/// [`ATTEMPTED_STEP_TAG`], which [`FAILING_FUNCTION`] writes before it raises — and answer
/// the step it recorded beside what `DescribeSecret` reported last.
///
/// This waits for a CONDITION rather than for a span: a rotation whose function raises on a
/// step cannot reach the last one, so once the attempt is on the secret the rotation's
/// outcome is settled and the row can assert it at once instead of waiting a rotation's
/// worth of invocations out.
async fn attempted(router: &MultiServiceRouter, case: &str, secret_id: &str) -> (String, Value) {
    let deadline = Instant::now() + ROTATION_WAIT;
    loop {
        let described = described(router, case, secret_id).await;
        if let Some(step) = tag_value(&described, ATTEMPTED_STEP_TAG) {
            return (step, described);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rotation function was never invoked, got {described}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The value of the tag `key` in a `DescribeSecret` answer, when it carries one.
fn tag_value(described: &Value, key: &str) -> Option<String> {
    described["Tags"].as_array()?.iter().find_map(|tag| {
        (tag["Key"].as_str() == Some(key)).then(|| {
            tag["Value"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
    })
}

/// The versions a `DescribeSecret` answer reports the staging label `stage` on.
fn staged_versions(described: &Value, stage: &str) -> Vec<String> {
    described["VersionIdsToStages"]
        .as_object()
        .map(|versions| {
            versions
                .iter()
                .filter(|(_, labels)| {
                    labels
                        .as_array()
                        .is_some_and(|labels| labels.iter().any(|label| label == stage))
                })
                .map(|(version, _)| version.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The value `GetSecretValue` serves for the staging label `stage`.
async fn value_at(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    stage: &str,
) -> Option<String> {
    let (status, parsed) = sm(
        router,
        "GetSecretValue",
        &json!({"SecretId": secret_id, "VersionStage": stage}),
    )
    .await;
    (status == 200)
        .then(|| {
            parsed["SecretString"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a SecretString, got {parsed}"))
                .to_string()
        })
        .or(None)
}

/// How a row writes the rotation function's grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No statement at all: the function holds no policy.
    None,
    /// `lambda:InvokeFunction` to this service, unscoped.
    Unscoped,
    /// The same, scoped to the secret being rotated.
    ThisSecret,
    /// The same, scoped to the secret's own account.
    ThisAccount,
    /// The same, scoped to ANOTHER secret.
    AnotherSecret,
    /// `lambda:InvokeFunction` to another service principal.
    AnotherPrincipal,
}

/// One row of [`a_rotation_runs_when_the_function_grants_it`] and of
/// [`a_rotation_is_refused_when_the_function_does_not_grant_it`].
struct GrantRow {
    case: &'static str,
    grant: Grant,
    /// Whether `RotateSecret` may start the rotation at all.
    permitted: bool,
}

fn grant_rows() -> Vec<GrantRow> {
    vec![
        GrantRow {
            case: "an unscoped grant to this service",
            grant: Grant::Unscoped,
            permitted: true,
        },
        GrantRow {
            case: "a grant scoped to the secret being rotated",
            grant: Grant::ThisSecret,
            permitted: true,
        },
        GrantRow {
            case: "a grant scoped to the secret's account",
            grant: Grant::ThisAccount,
            permitted: true,
        },
        GrantRow {
            case: "a grant scoped to another secret",
            grant: Grant::AnotherSecret,
            permitted: false,
        },
        GrantRow {
            case: "a grant to another service principal",
            grant: Grant::AnotherPrincipal,
            permitted: false,
        },
        GrantRow {
            case: "no policy on the function at all",
            grant: Grant::None,
            permitted: false,
        },
    ]
}

/// The secret a grant scoped to ANOTHER secret names, which is not the one being rotated.
const ANOTHER_SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:elsewhere-AbCdEf";

/// The service principal a grant made to another service names.
const ANOTHER_PRINCIPAL: &str = "events.amazonaws.com";

/// Write the grant `grant` states onto the rotation function `name`, for a rotation of
/// `secret_arn`, clearing whatever the row before it left there.
async fn apply_grant(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    grant: Grant,
    secret_arn: &str,
) {
    clear_permission(router, case, name).await;
    match grant {
        Grant::None => {}
        Grant::Unscoped => {
            add_permission(router, case, name, SERVICE_PRINCIPAL, None, None).await;
        }
        Grant::ThisSecret => {
            add_permission(router, case, name, SERVICE_PRINCIPAL, Some(secret_arn), None).await;
        }
        Grant::ThisAccount => {
            add_permission(
                router,
                case,
                name,
                SERVICE_PRINCIPAL,
                None,
                Some(engine::emulator_account_id()),
            )
            .await;
        }
        Grant::AnotherSecret => {
            add_permission(
                router,
                case,
                name,
                SERVICE_PRINCIPAL,
                Some(ANOTHER_SECRET_ARN),
                None,
            )
            .await;
        }
        Grant::AnotherPrincipal => {
            add_permission(router, case, name, ANOTHER_PRINCIPAL, None, None).await;
        }
    }
}

/// Drive every row of [`grant_rows`] whose rotation `permitted` says may start, each on a
/// secret of its own and all of them through the one rotation function `function_arn`.
///
/// The rows share that function because what a row varies is the function's POLICY: the
/// grant is written onto it as the row comes up, and the execution environment the first
/// rotation initializes serves the invocations of the rows after it.
async fn grant_rows_permitting(permitted: bool) {
    let router = router();
    let function = "rotator";
    let shared = "the rotation function the grant rows share";
    let function_arn = create_function(&router, shared, function, ROTATION_FUNCTION).await;
    let rows = grant_rows();
    let selected: Vec<&GrantRow> = rows.iter().filter(|row| row.permitted == permitted).collect();
    assert!(
        !selected.is_empty(),
        "case {shared:?}: expected the grant table to hold a row with permitted={permitted}"
    );
    for (index, row) in selected.iter().enumerate() {
        rotation_under_grant(&router, index, row, function, &function_arn).await;
    }
}

/// One row of the grant table: the rotation runs exactly when the FUNCTION's own policy admits
/// this service, and a rotation that runs ends with the value the rotation function wrote.
async fn rotation_under_grant(
    router: &MultiServiceRouter,
    index: usize,
    row: &GrantRow,
    function: &str,
    function_arn: &str,
) {
    let case = row.case;
    let name = format!("rotating-secret-{index}");
    let created = sm_ok(
        router,
        case,
        "CreateSecret",
        &json!({
            "Name": name,
            "ClientRequestToken": format!("{:032}", index + 1),
            "SecretString": "initial-credential",
        }),
    )
    .await;
    let secret_arn = created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {created}"))
        .to_string();

    apply_grant(router, case, function, row.grant, &secret_arn).await;

    let request = json!({
        "SecretId": name,
        "RotationLambdaARN": function_arn,
        "RotationRules": {"AutomaticallyAfterDays": 7},
    });
    if !row.permitted {
        let (status, code, message) = sm_err(router, case, "RotateSecret", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {code}"
        );
        assert_eq!(
            code, "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {code} with {message:?}"
        );
        assert!(
            message.contains(SERVICE_PRINCIPAL),
            "case {case:?}: expected the refusal to name {SERVICE_PRINCIPAL}, got {message:?}"
        );
        // A refused rotation changes nothing: rotation is not enabled and the
        // current value is the one the create stored.
        let held = described(router, case, &name).await;
        assert_ne!(
            held["RotationEnabled"].as_bool(),
            Some(true),
            "case {case:?}: a refused rotation enabled rotation, got {held}"
        );
        assert_eq!(
            value_at(router, case, &name, CURRENT_STAGE).await,
            Some("initial-credential".to_string()),
            "case {case:?}: a refused rotation changed the current value"
        );
        return;
    }

    let started = sm_ok(router, case, "RotateSecret", &request).await;
    let version_id = started["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a VersionId, got {started}"))
        .to_string();

    let finished = rotated(router, case, &name).await;
    assert!(
        finished.get("LastRotatedDate").is_some(),
        "case {case:?}: expected the rotation to finish and stamp LastRotatedDate, got \
         {finished}"
    );
    // The value the rotation function wrote is the one the secret now serves, and the
    // version it was staged under is the one the call reported.
    assert_eq!(
        value_at(router, case, &name, CURRENT_STAGE).await,
        Some("initial-credential-rotated".to_string()),
        "case {case:?}: expected the rotated value, got {finished}"
    );
    let stages = finished["VersionIdsToStages"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected VersionIdsToStages, got {finished}"));
    assert_eq!(
        stages
            .get(&version_id)
            .and_then(|labels| labels.as_array())
            .map(|labels| labels.iter().any(|label| label == CURRENT_STAGE)),
        Some(true),
        "case {case:?}: expected {CURRENT_STAGE} on {version_id}, got {finished}"
    );
    assert!(
        !stages.iter().any(|(held, labels)| held != &version_id
            && labels
                .as_array()
                .is_some_and(|labels| labels.iter().any(|label| label == PENDING_STAGE))),
        "case {case:?}: expected no pending version left behind, got {finished}"
    );
    // "NextRotationDate — the next rotation is scheduled to occur on or before this
    // date", which for a schedule stated as an interval is that interval after the
    // rotation that just happened.
    let last_rotated = finished["LastRotatedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected LastRotatedDate, got {finished}"));
    let next_rotation = finished["NextRotationDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected NextRotationDate, got {finished}"));
    let window = (next_rotation - last_rotated) / 86_400.0;
    assert!(
        (6.9..=7.1).contains(&window),
        "case {case:?}: expected the next rotation about 7 days after the last, got \
         {window} days in {finished}"
    );
}

/// Every row whose grant admits this service: the rotation runs, and it ends with the value
/// the rotation function wrote.
#[tokio::test]
async fn a_rotation_runs_when_the_function_grants_it() {
    grant_rows_permitting(true).await;
}

/// Every row whose grant does not admit this service: the rotation cannot be started, so the
/// call is refused and the secret is left where it was.
#[tokio::test]
async fn a_rotation_is_refused_when_the_function_does_not_grant_it() {
    grant_rows_permitting(false).await;
}

/// What a row's `RotateSecret` comes to, which is what the row then WAITS FOR: each outcome
/// has a state of its own that settles it, so no row waits a rotation's worth of invocations
/// out to conclude that nothing more will happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settles {
    /// Every step returns, so the rotation finishes and the secret is stamped as rotated.
    Finishes,
    /// The rotation function raises on its first step, so the rotation ends there: the
    /// attempt is recorded, `AWSPENDING` still names the version the rotation staked, and
    /// nothing was stamped.
    FailsOnItsFirstStep,
    /// No rotation is started at all, so nothing is ever in flight: the call stakes no
    /// `AWSPENDING` version, which is the state a rotation in flight is seen by.
    StartsNothing,
    /// The call itself is refused, with this error code.
    Refused(&'static str),
}

impl Settles {
    /// The OUTCOME this settles in, with what a refusal's code says left out: the tests below
    /// are grouped by outcome, so every row that is refused runs in one of them whatever code
    /// it states for itself.
    fn outcome(self) -> Outcome {
        match self {
            Settles::Finishes => Outcome::Finishes,
            Settles::FailsOnItsFirstStep => Outcome::FailsOnItsFirstStep,
            Settles::StartsNothing => Outcome::StartsNothing,
            Settles::Refused(_) => Outcome::Refused,
        }
    }
}

/// What a test below selects the rows it drives by: one outcome a row can settle in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The rotation finishes.
    Finishes,
    /// The rotation function raises on its first step.
    FailsOnItsFirstStep,
    /// No rotation is started.
    StartsNothing,
    /// The call is refused.
    Refused,
}

/// One row of the configuration table, which [`rows_settling_in`] drives.
struct ConfigurationRow {
    case: &'static str,
    /// The `RotationLambdaARN` the call carries, `None` for the created function's own ARN.
    lambda_arn: Option<&'static str>,
    /// Whether the call asks for `RotateImmediately`.
    rotate_immediately: Option<bool>,
    /// The rotation function the row creates, where it creates one: a row that states a
    /// `RotationLambdaARN` of its own names something no function of this emulator answers
    /// for, so none is made for it.
    source: &'static str,
    /// What the call, and the rotation it does or does not start, comes to.
    settles: Settles,
}

fn configuration_rows() -> Vec<ConfigurationRow> {
    vec![
        ConfigurationRow {
            case: "a rotation of a function that completes every step",
            lambda_arn: None,
            rotate_immediately: None,
            source: ROTATION_FUNCTION,
            settles: Settles::Finishes,
        },
        ConfigurationRow {
            case: "a function that raises on its first step leaves the rotation unfinished",
            lambda_arn: None,
            rotate_immediately: None,
            source: FAILING_FUNCTION,
            settles: Settles::FailsOnItsFirstStep,
        },
        ConfigurationRow {
            case: "RotateImmediately false configures rotation and rotates nothing",
            lambda_arn: None,
            rotate_immediately: Some(false),
            source: ROTATION_FUNCTION,
            settles: Settles::StartsNothing,
        },
        ConfigurationRow {
            case: "a RotationLambdaARN that is not an ARN at all",
            lambda_arn: Some("RotateMySecret"),
            rotate_immediately: None,
            source: ROTATION_FUNCTION,
            settles: Settles::Refused("InvalidRequestException"),
        },
        ConfigurationRow {
            case: "a RotationLambdaARN naming another service's resource",
            lambda_arn: Some("arn:aws:sqs:us-west-2:000000000000:rotation-queue"),
            rotate_immediately: None,
            source: ROTATION_FUNCTION,
            settles: Settles::Refused("InvalidRequestException"),
        },
        ConfigurationRow {
            case: "a RotationLambdaARN naming no function of Lambda's own",
            lambda_arn: Some("arn:aws:lambda:us-west-2:000000000000:layer:rotation"),
            rotate_immediately: None,
            source: ROTATION_FUNCTION,
            settles: Settles::Refused("InvalidRequestException"),
        },
    ]
}

/// Drive every row of [`configuration_rows`] that settles in `expected`, each on a secret of
/// its own over one router.
///
/// The rows are grouped by the outcome they settle in so that no test drives more than one
/// rotation which has to run through all four of its steps: every row settles in one of the
/// four [`Outcome`]s, so each row runs in exactly one of the tests below.
async fn rows_settling_in(expected: Outcome) {
    let router = router();
    let rows = configuration_rows();
    let selected: Vec<&ConfigurationRow> = rows
        .iter()
        .filter(|row| row.settles.outcome() == expected)
        .collect();
    assert!(
        !selected.is_empty(),
        "expected the configuration table to hold a row settling in {expected:?}"
    );
    for (index, row) in selected.iter().enumerate() {
        configured_rotation(&router, index, row).await;
    }
}

/// One row of the configuration table: what the rotation configuration states decides what the
/// rotation does, and a rotation that does not finish leaves the secret where it was.
async fn configured_rotation(router: &MultiServiceRouter, index: usize, row: &ConfigurationRow) {
    let case = row.case;
    let name = format!("configured-secret-{index}");
    sm_ok(
        router,
        case,
        "CreateSecret",
        &json!({
            "Name": name,
            "ClientRequestToken": format!("{:032}", 500 + index),
            "SecretString": "initial-credential",
        }),
    )
    .await;
    // A row that states a `RotationLambdaARN` of its own is refused on that value, so it
    // never reaches a function and none is created for it.
    let lambda_arn = match row.lambda_arn {
        Some(stated) => stated.to_string(),
        None => {
            let function = format!("configured-rotator-{index}");
            let function_arn = create_function(router, case, &function, row.source).await;
            add_permission(router, case, &function, SERVICE_PRINCIPAL, None, None).await;
            function_arn
        }
    };

    let mut request = json!({"SecretId": name, "RotationLambdaARN": lambda_arn});
    if let Some(immediately) = row.rotate_immediately {
        request["RotateImmediately"] = json!(immediately);
    }
    if let Settles::Refused(expected) = row.settles {
        let (status, code, message) = sm_err(router, case, "RotateSecret", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {code}"
        );
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with {message:?}"
        );
        return;
    }

    let started = sm_ok(router, case, "RotateSecret", &request).await;
    let version_id = started["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a VersionId, got {started}"))
        .to_string();
    let settled = match row.settles {
        Settles::Finishes => {
            let settled = rotated(router, case, &name).await;
            assert!(
                settled.get("LastRotatedDate").is_some(),
                "case {case:?}: expected the rotation to finish and stamp LastRotatedDate, \
                 got {settled}"
            );
            settled
        }
        Settles::FailsOnItsFirstStep => {
            // The step the function raised on is the FIRST of the documented four, and
            // once it has run the rotation is over: there is no state left to wait for.
            let (step, settled) = attempted(router, case, &name).await;
            assert_eq!(
                step, ROTATION_STEPS[0],
                "case {case:?}: expected the rotation to be driven from its first step, \
                 got {step:?}"
            );
            // "When rotation is unsuccessful, the AWSPENDING staging label might be
            // attached to an empty secret version": the label still names the version
            // the rotation staked, that version holds no value, and nothing stamped the
            // secret as rotated.
            assert_eq!(
                staged_versions(&settled, PENDING_STAGE),
                vec![version_id.clone()],
                "case {case:?}: expected {PENDING_STAGE} still staked on {version_id}, got \
                 {settled}"
            );
            assert_eq!(
                value_at(router, case, &name, PENDING_STAGE).await,
                None,
                "case {case:?}: expected the staked version to hold no value, got {settled}"
            );
            assert!(
                settled.get("LastRotatedDate").is_none(),
                "case {case:?}: expected an unfinished rotation to leave LastRotatedDate \
                 unset, got {settled}"
            );
            settled
        }
        Settles::StartsNothing => {
            // A rotation that was STARTED is in flight from the call that answers, which
            // is the state a second RotateSecret is refused for: it stakes AWSPENDING on
            // the version the rotation will create. So a call that staked no such
            // version started no rotation, and that is readable at once.
            let settled = described(router, case, &name).await;
            assert_eq!(
                staged_versions(&settled, PENDING_STAGE),
                Vec::<String>::new(),
                "case {case:?}: expected no rotation in flight, got {settled}"
            );
            assert!(
                settled["VersionIdsToStages"].get(&version_id).is_none(),
                "case {case:?}: expected no version {version_id} to have been staked, got \
                 {settled}"
            );
            assert!(
                settled.get("LastRotatedDate").is_none(),
                "case {case:?}: expected a secret that was not rotated to leave \
                 LastRotatedDate unset, got {settled}"
            );
            settled
        }
        // The refused rows left above.
        Settles::Refused(_) => unreachable!("a refused row never reaches this point"),
    };
    // Whatever the rotation came to, the value the secret serves is the function's own
    // work: the rotated one for a rotation that finished, and the one the create stored
    // for a rotation that did not.
    let expected_value = if row.settles == Settles::Finishes {
        "initial-credential-rotated"
    } else {
        "initial-credential"
    };
    assert_eq!(
        value_at(router, case, &name, CURRENT_STAGE).await,
        Some(expected_value.to_string()),
        "case {case:?}: expected the current value {expected_value:?}, got {settled}"
    );
    // Configuring rotation enables it, whether or not a rotation was started.
    assert_eq!(
        settled["RotationEnabled"].as_bool(),
        Some(true),
        "case {case:?}: expected rotation enabled, got {settled}"
    );
}

/// Every row whose rotation function completes every step: the rotation finishes, and the
/// secret ends up holding what that function wrote.
#[tokio::test]
async fn a_configured_rotation_whose_function_completes_every_step_finishes() {
    rows_settling_in(Outcome::Finishes).await;
}

/// Every row whose rotation function raises on its first step: the rotation ends there and
/// the secret is left where it was.
#[tokio::test]
async fn a_configured_rotation_whose_function_raises_leaves_the_secret_where_it_was() {
    rows_settling_in(Outcome::FailsOnItsFirstStep).await;
}

/// Every row whose configuration starts no rotation: rotation is configured and enabled, and
/// nothing is in flight.
#[tokio::test]
async fn a_configuration_that_rotates_nothing_starts_no_rotation() {
    rows_settling_in(Outcome::StartsNothing).await;
}

/// Every row whose `RotationLambdaARN` names no rotation function: the call is refused, and
/// no function is invoked — or created — for it. Each row is checked against the error code
/// it states for itself.
#[tokio::test]
async fn a_rotation_lambda_arn_that_names_no_function_is_refused() {
    rows_settling_in(Outcome::Refused).await;
}

/// The outcomes the four tests above select the configuration table by, one per way a row can
/// settle.
const DRIVEN_OUTCOMES: [Outcome; 4] = [
    Outcome::Finishes,
    Outcome::FailsOnItsFirstStep,
    Outcome::StartsNothing,
    Outcome::Refused,
];

/// RULE: grouping the rows of a table into tests by what they settle in is TOTAL — every row
/// is driven by exactly one of those tests, so a row the table gains cannot be left undriven
/// by all of them.
///
/// This is what makes the split above safe to read as "the suite still asserts every row":
/// the four outcomes the tests select by are the four a row can settle in, and the grant table
/// is split by a flag whose both values are driven.
#[test]
fn every_row_of_both_tables_is_driven_by_exactly_one_test() {
    let rows = configuration_rows();
    for row in &rows {
        let driven = DRIVEN_OUTCOMES
            .iter()
            .filter(|outcome| row.settles.outcome() == **outcome)
            .count();
        assert_eq!(
            driven, 1,
            "case {:?}: expected exactly one test to drive this row, got {driven}",
            row.case
        );
    }
    for outcome in &DRIVEN_OUTCOMES {
        let held = rows
            .iter()
            .filter(|row| row.settles.outcome() == *outcome)
            .count();
        assert!(
            held > 0,
            "case {outcome:?}: expected the configuration table to hold a row settling this \
             way, got {held}"
        );
    }
    let grants = grant_rows();
    for permitted in [true, false] {
        let held = grants.iter().filter(|row| row.permitted == permitted).count();
        assert!(
            held > 0,
            "case permitted={permitted}: expected the grant table to hold a row whose rotation \
             starts when its grant allows it, got {held}"
        );
    }
}

/// RULE: a rotation is in flight from the call that starts it until its last step returns,
/// and "any later invocation of RotateSecret assumes that a previous rotation request is
/// still in progress and returns an error" while it is.
#[tokio::test]
async fn a_second_rotation_is_refused_while_one_is_in_flight() {
    let case = "a second RotateSecret while one is in flight";
    let router = router();
    let name = "contended-secret";
    sm_ok(
        &router,
        case,
        "CreateSecret",
        &json!({
            "Name": name,
            "ClientRequestToken": format!("{:032}", 900),
            "SecretString": "initial-credential",
        }),
    )
    .await;
    let function = "contended-rotator";
    let function_arn = create_function(&router, case, function, ROTATION_FUNCTION).await;
    add_permission(&router, case, function, SERVICE_PRINCIPAL, None, None).await;

    let request = json!({"SecretId": name, "RotationLambdaARN": function_arn});
    let started = sm_ok(&router, case, "RotateSecret", &request).await;
    let version_id = started["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a VersionId, got {started}"))
        .to_string();

    // The call answered while the rotation it started is still running, so the state a
    // caller sees straight after it is the rotation IN FLIGHT: AWSPENDING staked on the
    // version the function has not created yet.
    let inflight = described(&router, case, name).await;
    assert_eq!(
        inflight["VersionIdsToStages"][&version_id]
            .as_array()
            .map(|labels| labels.iter().any(|label| label == "AWSPENDING")),
        Some(true),
        "case {case:?}: expected AWSPENDING staked on {version_id}, got {inflight}"
    );
    let (status, code, message) = sm_err(&router, case, "RotateSecret", &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {code}"
    );
    assert_eq!(
        code, "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {code} with {message:?}"
    );
    assert_eq!(
        message, "A previous rotation isn't complete. That rotation will be reattempted.",
        "case {case:?}: expected the sentence for a rotation still in progress, got {message:?}"
    );

    // And once it has finished, a rotation may start again: the label no longer names a
    // version other than the current one.
    let finished = rotated(&router, case, name).await;
    assert!(
        finished.get("LastRotatedDate").is_some(),
        "case {case:?}: expected the first rotation to finish, got {finished}"
    );
    let again = sm_ok(&router, case, "RotateSecret", &request).await;
    assert!(
        again["VersionId"].as_str().is_some_and(|id| id != version_id),
        "case {case:?}: expected a new version id for the second rotation, got {again}"
    );
}

/// RULE: a rotation that finished stamps `LastRotatedDate` at the instant it finished, so
/// the stamp is not in the future and not before the call that started the rotation.
#[tokio::test]
async fn the_rotation_stamp_is_the_instant_the_rotation_finished() {
    let case = "the LastRotatedDate of a rotation that ran";
    let router = router();
    let name = "stamped-secret";
    sm_ok(
        &router,
        case,
        "CreateSecret",
        &json!({
            "Name": name,
            "ClientRequestToken": format!("{:032}", 901),
            "SecretString": "initial-credential",
        }),
    )
    .await;
    let function = "stamped-rotator";
    let function_arn = create_function(&router, case, function, ROTATION_FUNCTION).await;
    add_permission(&router, case, function, SERVICE_PRINCIPAL, None, None).await;

    let before = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    sm_ok(
        &router,
        case,
        "RotateSecret",
        &json!({"SecretId": name, "RotationLambdaARN": function_arn}),
    )
    .await;
    let finished = rotated(&router, case, name).await;
    let stamped = finished["LastRotatedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected LastRotatedDate, got {finished}"));
    let after = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    assert!(
        (before..=after).contains(&stamped),
        "case {case:?}: expected a stamp between {before} and {after}, got {stamped}"
    );
}

/// RULE: "if rotation fails, Secrets Manager retries the entire rotation process multiple
/// times" (`DescribeSecretResponse$NextRotationDate`, this service's Smithy model) — so a
/// rotation whose first attempt could not run the rotation function is tried again, and one
/// whose obstacle has cleared by then finishes.
///
/// The obstacle here is the one a caller hits by configuring rotation as soon as the function
/// is created: Lambda opens a provisioning window on `CreateFunction` ("During this time, you
/// can't invoke or modify the function"), so the first invocation of the rotation is refused
/// while the function is still provisioning. The window closes on its own, which is what the
/// retry finds.
#[tokio::test]
async fn a_rotation_refused_by_the_provisioning_window_is_retried() {
    let case = "a rotation started while its function is still provisioning";
    let router = router();
    let name = "promptly-rotated-secret";
    sm_ok(
        &router,
        case,
        "CreateSecret",
        &json!({
            "Name": name,
            "ClientRequestToken": format!("{:032}", 902),
            "SecretString": "initial-credential",
        }),
    )
    .await;

    // The function is NOT waited for: the rotation is configured straight after the create,
    // the way a caller that sets a secret up in one script does.
    let function = "prompt-rotator";
    let (status, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": function,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(ROTATION_FUNCTION)),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} with {created}"
    );
    let function_arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string();
    add_permission(&router, case, function, SERVICE_PRINCIPAL, None, None).await;

    let started = sm_ok(
        &router,
        case,
        "RotateSecret",
        &json!({"SecretId": name, "RotationLambdaARN": function_arn}),
    )
    .await;
    let version_id = started["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a VersionId, got {started}"))
        .to_string();

    let finished = rotated(&router, case, name).await;
    assert!(
        finished.get("LastRotatedDate").is_some(),
        "case {case:?}: expected the retried rotation to finish, got {finished}"
    );
    assert_eq!(
        staged_versions(&finished, CURRENT_STAGE),
        vec![version_id.clone()],
        "case {case:?}: expected AWSCURRENT on the version the rotation created, got {finished}"
    );
    assert_eq!(
        value_at(&router, case, name, CURRENT_STAGE).await,
        Some("initial-credential-rotated".to_string()),
        "case {case:?}: expected the value the rotation function wrote, got {finished}"
    );
}
