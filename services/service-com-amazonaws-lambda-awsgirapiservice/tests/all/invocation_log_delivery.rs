// SPDX-License-Identifier: CC0-1.0

//! Where an invocation's LOG goes: the function's CloudWatch Logs group, read back through
//! CloudWatch Logs' own API.
//!
//! RULE: every invocation Lambda runs delivers its log to the function's log group — the one
//! its `LoggingConfig$LogGroup` names, else the default `/aws/lambda/<function name>` — into
//! a stream of that invocation named for the date, the version that executed and the
//! invocation itself, one event per line of the log: the `START` line, everything the handler
//! wrote, and the `END` and `REPORT` lines. Lambda makes those writes ACTING AS the function's
//! execution role, so a role the policies in force on it do not allow to create the group or
//! to put events leaves correspondingly less behind, and a withheld write is never an
//! invocation failure.
//!
//! Every row drives real HTTP through the production `MultiServiceRouter` with Lambda,
//! CloudWatch Logs and IAM mounted on ONE engine — exactly what `core/router` assembles — so
//! what is asserted is what a client reads: `DescribeLogGroups`, `DescribeLogStreams` and
//! `GetLogEvents` on the group, never a handler's Rust.
//!
//! The rows differ along what the rule generalises over: what the handler wrote (nothing, one
//! line, several lines, an exception), which group the function logs to (the default, and one
//! its configuration names), how many invocations were made (so two runs take two streams),
//! which RUNTIME ran (the rule is the host's, so the rows that do not assert a runtime's own
//! output run on the cheaper engine and the one that does names python), which of the
//! three log writes the execution role allows (all of them, none, everything but the put, and
//! a put denied beside a wildcard grant), and which INVOCATION SURFACE ran the function (the
//! synchronous `Invoke`, the asynchronous `Event` one whose run happens off the response
//! path, and the dry run, which runs nothing and so logs nothing).
//!
//! Every test here RUNS handlers, so each takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process, and a suite that
//! keeps several of those slots busy starves every other suite's runs.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    run_budget, run_heap_bytes, ABANDON_GRACE, HOST_WAIT_CEILING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{allow, given_role};
use crate::event_source_mappings::{await_function_active, call, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN for the rows that mount no IAM at all, where nothing is
/// known about the role and so no write is withheld.
const UNKNOWN_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The IAM action namespace CloudWatch Logs' calls are named in.
pub(crate) const LOGS: &str = "logs";

/// The three calls the runtime makes to deliver one invocation's log. Source: the actions
/// AWS's own managed execution-role policy for a function (`AWSLambdaBasicExecutionRole`)
/// grants.
pub(crate) const LOG_WRITES: [&str; 3] = ["CreateLogGroup", "CreateLogStream", "PutLogEvents"];

/// Lambda, CloudWatch Logs and IAM over ONE shared engine.
///
/// `pub(crate)` so the sibling suite that reads a delivered log through the group-wide search
/// ([`crate::invocation_log_group_search`]) mounts the same three services rather than a
/// second copy of this harness.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`), as its parsed body.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) async fn logs(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    logs_answer(router, case, operation, body).await.1
}

/// The SAME call, with the STATUS it answered beside its body — for a caller that asserts the
/// refusal itself rather than the content.
pub(crate) async fn logs_answer(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/logs/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert!(
        status < 500,
        "case {case:?}: CloudWatch Logs {operation} answered {status}: {parsed}"
    );
    (status, parsed)
}

/// The names of the log groups whose name starts with `prefix`.
///
/// `pub(crate)` for the reason [`emulator`] is: the sibling suite that walks HOW the logs
/// grant reaches the execution role ([`crate::invocation_log_delivery_grant_carrier`]) reads
/// the delivery back through these same three readers rather than a second copy of them.
pub(crate) async fn groups(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let answered = logs(
        router,
        case,
        "DescribeLogGroups",
        &json!({ "logGroupNamePrefix": prefix }),
    )
    .await;
    answered["logGroups"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The names of the streams the group `group` holds.
///
/// `pub(crate)` for the reason [`groups`] is.
pub(crate) async fn streams(router: &MultiServiceRouter, case: &str, group: &str) -> Vec<String> {
    let answered = logs(
        router,
        case,
        "DescribeLogStreams",
        &json!({ "logGroupName": group }),
    )
    .await;
    answered["logStreams"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The events of one stream, as `(timestamp, message)` pairs in the order they were written.
///
/// `pub(crate)` for the reason [`groups`] is.
pub(crate) async fn events(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    stream: &str,
) -> Vec<(i64, String)> {
    let answered = logs(
        router,
        case,
        "GetLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "startFromHead": true,
        }),
    )
    .await;
    answered["events"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .map(|event| {
                    (
                        event["timestamp"].as_i64().unwrap_or_default(),
                        event["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The text of ONE delivered record: the message the group holds with the line break that
/// closed the write it was made in taken off.
///
/// Every record of a run reaches CloudWatch Logs closed by that break — a run's log is a stream
/// of newline-terminated records, and Lambda delivers each as it was written — so the break is
/// part of the record rather than of its text. The suite about the delivery itself asserts it is
/// there ([`an_invocation_delivers_its_log_to_the_functions_log_group`]); every suite reading a
/// record for what it SAYS goes through here, so the terminator is stated in one place.
///
/// `pub(crate)` for the reason [`groups`] is.
#[must_use]
pub(crate) fn record_text(message: &str) -> &str {
    message.strip_suffix('\n').unwrap_or(message)
}

/// ONE delivered line as the JSON OBJECT it is, or `None` for a line that is not one — which
/// is how a client that selected the structured format reads what it was delivered.
///
/// `pub(crate)` for the reason [`record_text`] is: every suite that reads a STRUCTURED record
/// ([`crate::invocation_log_record_format`], [`crate::invocation_log_phases_and_level_filters`],
/// [`crate::python_json_error_records`]) parses a delivered line through here, so the reading is
/// stated once.
#[must_use]
pub(crate) fn structured_record(line: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// Every message the group `group` holds, whichever stream carries it, as `FilterLogEvents`
/// reports them — the read a client that has just invoked the function makes, knowing the group
/// but not the stream.
///
/// `pub(crate)` for the reason [`groups`] is: the suites that read what a run wrote without
/// naming its stream ([`crate::node_caught_error_log`],
/// [`crate::invoke_handler_reported_status`]) share this reader rather than keeping a copy each.
pub(crate) async fn searched(router: &MultiServiceRouter, case: &str, group: &str) -> Vec<String> {
    let answered = logs(
        router,
        case,
        "FilterLogEvents",
        &json!({ "logGroupName": group }),
    )
    .await;
    answered["events"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .map(|event| event["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The header a run that FAILED is signalled on (`Invoke$FunctionError`, `httpHeader:
/// X-Amz-Function-Error`), which a handler that returned never sets.
///
/// `pub(crate)` for the reason [`groups`] is.
pub(crate) const FUNCTION_ERROR_HEADER: &str = "x-amz-function-error";

/// The whole log of one stream as its text, each line closed by its break — which is the form
/// the invocation itself reports its log tail in.
///
/// An event's own message is the LINE as the runtime wrote it, the break that closed it
/// included, so the lines are reported here without it and the text is the messages as they
/// were held.
///
/// `pub(crate)` for the reason [`groups`] is.
pub(crate) async fn delivered(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    stream: &str,
) -> (Vec<String>, String) {
    let held = events(router, case, group, stream).await;
    // The text is the records as they were delivered, concatenated: each already carries the
    // break that closed it, which is what makes the whole stream the run's log again.
    let text = held.iter().map(|(_, record)| record.clone()).collect();
    let lines: Vec<String> = held
        .iter()
        .map(|(_, record)| record_text(record).to_string())
        .collect();
    (lines, text)
}

/// The log of the INVOCATIONS a stream holds: its records, each closed by its break, less the
/// record that states the initialization of the execution environment — which is the
/// ENVIRONMENT's own record, written to the group ahead of the first invocation it serves and
/// no part of what any invocation reports as its own log.
///
/// This is the form an invocation reports its log tail in, which is what makes the two
/// readings of one run comparable.
///
/// `pub(crate)` for the reason [`groups`] is.
pub(crate) fn invocations_log(records: &[String]) -> String {
    records
        .iter()
        .filter(|record| !states_initialization(record))
        .map(|record| format!("{record}\n"))
        .collect()
}

/// Whether `record` is the one that states the initialization of an execution environment, in
/// either of the two forms a function's `LogFormat` renders it in.
///
/// `pub(crate)` for the reason [`groups`] is.
pub(crate) fn states_initialization(record: &str) -> bool {
    record.starts_with("INIT_START Runtime Version: ")
        || record.contains("\"type\":\"platform.initStart\"")
}

/// A function the suite creates: its deployment package, its runtime and where it logs.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) struct Created<'a> {
    pub(crate) case: &'a str,
    pub(crate) name: &'a str,
    /// The handler's source.
    pub(crate) source: &'a str,
    /// The runtime that runs it, which decides the package's one entry and the handler name.
    pub(crate) runtime: &'a str,
    /// The execution role it runs as.
    pub(crate) role: &'a str,
    /// The group its configuration names, or `None` to take the default.
    pub(crate) log_group: Option<&'a str>,
}

/// The runtime most rows use. The delivery is the HOST's, so the cheaper engine runs the rows
/// that do not assert a runtime's own output; the python rows below name python themselves.
pub(crate) const NODE_RUNTIME: &str = "nodejs20.x";

/// The runtime whose own error report one row asserts.
pub(crate) const PYTHON_RUNTIME: &str = "python3.12";

/// The `Timeout` every function this fixture creates carries, in the seconds the member is
/// stated in.
///
/// No suite over the delivery of a log asserts anything about this bound, so it is given all
/// the room there is to give rather than a value picked by hand: a run is waited for at most
/// `state::execution::HOST_WAIT_CEILING`, of which `state::execution::ABANDON_GRACE` is the
/// span a caller still gives a run past its own budget, so this is the longest budget whose
/// run the caller waits out in full. A tighter one would turn the HOST's speed into an
/// invocation FAILURE — a test binary runs its suites together, so a run's wall clock covers
/// the other suites' work as well as its own, and a handler cut off on that bound is a failed
/// attempt rather than a slow one.
pub(crate) fn function_timeout_seconds() -> i32 {
    let seconds = HOST_WAIT_CEILING.saturating_sub(ABANDON_GRACE).as_secs();
    i32::try_from(seconds).unwrap_or(i32::MAX).max(1)
}

/// The `MemorySize` every function this fixture creates carries, in the MB the member is
/// stated in: the smallest value that gives a run the whole per-run ceiling this emulator
/// allows (`state::execution::run_heap_bytes`, which clamps every larger `MemorySize` to it).
///
/// For the reason [`function_timeout_seconds`] is generous, and against a bound that is read
/// from the whole PROCESS as well as from the run: a run is stopped for memory once this
/// process is past its own resident ceiling AND has grown, since the run began, by more than
/// the run's own ceiling allows it to add — and in a test binary running many suites at once
/// that growth is the other suites' and none of the run's own. A function given the per-run
/// ceiling is the furthest such growth can be put out of reach of a run that allocates
/// almost nothing, as every row of these suites does.
pub(crate) fn function_memory_size_mb() -> i32 {
    const BYTES_PER_MB: usize = 1024 * 1024;
    let megabytes = run_heap_bytes(i32::MAX) / BYTES_PER_MB;
    i32::try_from(megabytes).unwrap_or(i32::MAX).max(1)
}

/// Create `function` and wait until it reports `Active`.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) async fn create_function(router: &MultiServiceRouter, function: &Created<'_>) {
    create_function_logging(router, function, None).await;
}

/// The SAME create, with the whole `LoggingConfig` the function carries — for a suite that
/// configures more of it than the group ([`crate::invocation_log_record_format`], whose
/// functions carry a `LogFormat` and a level). The configuration REPLACES
/// [`Created::log_group`] where one is given, so one create carries one configuration and
/// there is one create path in this target.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) async fn create_function_logging(
    router: &MultiServiceRouter,
    function: &Created<'_>,
    logging_config: Option<&Value>,
) {
    let python = function.runtime.starts_with("python");
    let (handler, package) = if python {
        (
            "lambda_function.lambda_handler",
            crate::synchronous_invoke_run::package(function.source),
        )
    } else {
        (
            "index.handler",
            crate::node_invoke_run::package(&[("index.js", function.source)]),
        )
    };
    let mut body = json!({
        "FunctionName": function.name,
        "Role": function.role,
        "Runtime": function.runtime,
        "Handler": handler,
        "Timeout": function_timeout_seconds(),
        "MemorySize": function_memory_size_mb(),
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(package) },
    });
    match (logging_config, function.log_group) {
        (Some(logging), _) => body["LoggingConfig"] = logging.clone(),
        (None, Some(group)) => body["LoggingConfig"] = json!({ "LogGroup": group }),
        (None, None) => {}
    }
    let created = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        created.status,
        201,
        "case {:?}: expected CreateFunction {:?} to answer 201, got {} {}",
        function.case,
        function.name,
        created.status,
        created.text()
    );
    await_function_active(router, function.case, function.name).await;
}

/// `Invoke` the function `name` synchronously, asking for the log tail, and answer the log the
/// run wrote as the invocation itself reported it, together with the payload it answered.
///
/// `X-Amz-Log-Result` is "the last 4 KB of the execution log, which is base64-encoded", so for
/// the small logs below it IS the whole log — which makes it the independent statement of what
/// the delivery has to have put into the group.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) async fn invoke(router: &MultiServiceRouter, case: &str, name: &str) -> (String, Value) {
    let request = invocation_request(name, &[("x-amz-log-type", "Tail")]);
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    assert_eq!(
        parts.status.as_u16(),
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        parts.status,
        String::from_utf8_lossy(&body)
    );
    let encoded = parts
        .headers
        .get("x-amz-log-result")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a log tail on the answer"));
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the log tail is base64: {error}"));
    let log = String::from_utf8(decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the log tail is text: {error}"));
    let payload: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (log, payload)
}

/// ONE `Invoke` request on the function `name`, signed for this suite's Region and carrying
/// `headers` on top of the ones every invocation of it carries. The signing and host details
/// live here, so every invocation this suite makes is the same request but for those headers.
///
/// `pub(crate)` for the reason [`emulator`] is: the sibling suite that reads the answer's own
/// HEADERS beside the log ([`crate::node_caught_error_log`], which asserts that a caught error
/// is no function error) sends this same request rather than a second copy of it.
pub(crate) fn invocation_request(
    name: &str,
    headers: &[(&str, &str)],
) -> http::Request<bytes::Bytes> {
    invocation_request_carrying(name, headers, b"{}")
}

/// The SAME request carrying `event` as the invocation's PAYLOAD — for a suite whose handler
/// branches on what it was handed ([`crate::invoke_handler_reported_status`], whose rows are
/// the values one handler answers different events with), so this target keeps one `Invoke`
/// request path rather than a second copy of its signing.
pub(crate) fn invocation_request_carrying(
    name: &str,
    headers: &[(&str, &str)],
    event: &[u8],
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("{FUNCTIONS_URI}/{name}/invocations"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/lambda/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", "application/json");
    for (header, value) in headers {
        builder = builder.header(*header, *value);
    }
    builder
        .body(bytes::Bytes::copy_from_slice(event))
        .expect("the test request is well formed")
}

/// One row of the delivery table: what the handler writes, and how many of its own lines that
/// leaves in the log.
struct Row {
    case: &'static str,
    /// The body of the handler, which decides what the run writes.
    source: &'static str,
    /// The runtime that runs it.
    runtime: &'static str,
    /// The lines the HANDLER itself must have contributed, each of which has to appear as an
    /// event of its own.
    written: &'static [&'static str],
    /// The group the function's configuration names, or `None` to take the default.
    log_group: Option<&'static str>,
    /// Whether the handler reports the destination the runtime told it about, which must be
    /// the group and the stream the delivery used.
    reports_destination: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a handler that writes nothing: only the framing is logged",
            source: "exports.handler = async () => ({ ok: true });",
            runtime: NODE_RUNTIME,
            written: &[],
            log_group: None,
            reports_destination: false,
        },
        Row {
            case: "a handler that writes one line",
            source: "exports.handler = async () => {\n    console.log('one line from the \
                     handler');\n    return { ok: true };\n};",
            runtime: NODE_RUNTIME,
            written: &["one line from the handler"],
            log_group: None,
            reports_destination: false,
        },
        Row {
            case: "a handler that writes several lines",
            source: "exports.handler = async () => {\n    for (let index = 0; index < 3; index \
                     += 1) {\n        console.log('line ' + index);\n    }\n    return { ok: \
                     true };\n};",
            runtime: NODE_RUNTIME,
            written: &["line 0", "line 1", "line 2"],
            log_group: None,
            reports_destination: false,
        },
        Row {
            case: "a function whose configuration names another group",
            source: "exports.handler = async () => {\n    console.log('into the named \
                     group');\n    return {\n        group: \
                     process.env.AWS_LAMBDA_LOG_GROUP_NAME,\n        stream: \
                     process.env.AWS_LAMBDA_LOG_STREAM_NAME,\n    };\n};",
            runtime: NODE_RUNTIME,
            written: &["into the named group"],
            log_group: Some("/aws/lambda/named-elsewhere"),
            reports_destination: true,
        },
    ]
}

/// The log of every run reaches the group the function logs to, as the events of a stream of
/// its own.
#[tokio::test]
async fn an_invocation_delivers_its_log_to_the_functions_log_group() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let name = format!("log-probe-{index}");
        let group = row
            .log_group
            .map_or_else(|| format!("/aws/lambda/{name}"), str::to_string);
        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source: row.source,
                runtime: row.runtime,
                role: UNKNOWN_ROLE,
                log_group: row.log_group,
            },
        )
        .await;

        // Nothing is logged before the function is invoked: the group is created by the
        // first invocation, not by the create.
        let before = groups(&router, case, &group).await;
        assert!(
            before.is_empty(),
            "case {case:?}: expected no log group before the first invocation, got {before:?}"
        );

        let (reported, payload) = invoke(&router, case, &name).await;
        let found = groups(&router, case, &group).await;
        assert_eq!(
            found,
            vec![group.clone()],
            "case {case:?}: expected the invocation to create {group:?}, got {found:?}"
        );

        let held = streams(&router, case, &group).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected one stream for one invocation, got {held:?}"
        );
        let stream = held[0].clone();

        // The whole log the invocation reported, record for record, is what the group holds —
        // beside the one record the ENVIRONMENT wrote for its own initialization.
        let (lines, _rebuilt) = delivered(&router, case, &group, &stream).await;
        let rebuilt = invocations_log(&lines);
        assert_eq!(
            rebuilt, reported,
            "case {case:?}: expected the group to hold the log the invocation reported, got \
             {rebuilt:?} against {reported:?}"
        );

        // Each event is ONE record of the run, carried as the write produced it: the line break
        // that closed the write is part of the message a client reads, and there is exactly one
        // of them per record.
        for (_, message) in events(&router, case, &group, &stream).await {
            assert!(
                message.ends_with('\n') && !record_text(&message).ends_with('\n'),
                "case {case:?}: expected every delivered record closed by exactly one line \
                 break, got {message:?}"
            );
        }

        // The framing, and the handler's own lines between it. The log opens on the
        // initialization of the execution environment the run built, which precedes the
        // invocation's own START line.
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with("INIT_START Runtime Version: ")),
            "case {case:?}: expected the log to open on the initialization of the environment, \
             got {lines:?}"
        );
        assert!(
            lines
                .get(1)
                .is_some_and(|line| line.starts_with("START RequestId: ")),
            "case {case:?}: expected the invocation's START line after it, got {lines:?}"
        );
        assert_eq!(
            lines.len(),
            row.written.len() + 4,
            "case {case:?}: expected 4 framing lines around {} of the handler's own, got \
             {lines:?}",
            row.written.len()
        );
        for (position, written) in row.written.iter().enumerate() {
            let actual = lines.get(position + 2).cloned().unwrap_or_default();
            // A node runtime delivers a console line FRAMED — the instant, the invocation's
            // id, the level, then the text — so what the handler wrote is the last field.
            let text = actual.rsplit('\t').next().unwrap_or_default();
            assert_eq!(
                text, *written,
                "case {case:?}: expected the handler's line {position} to be {written:?}, got \
                 {actual:?}"
            );
        }
        let closing: Vec<&String> = lines.iter().rev().take(2).collect();
        assert!(
            closing[1].starts_with("END RequestId: ")
                && closing[0].starts_with("REPORT RequestId: "),
            "case {case:?}: expected the log to close on END and REPORT, got {closing:?}"
        );

        // The stream names the invocation: the date it ran, the version that executed, and
        // the invocation's own id, which is the one the START line carries — the environment
        // was built for this invocation, so the stream it opened is named after it.
        let request_id = lines[1]
            .trim_start_matches("START RequestId: ")
            .split(' ')
            .next()
            .unwrap_or_default()
            .replace('-', "");
        assert!(
            stream.ends_with(&format!("/[$LATEST]{request_id}")),
            "case {case:?}: expected the stream to name the version and the invocation, got \
             {stream:?}"
        );
        let dated: Vec<&str> = stream.split('/').collect();
        assert!(
            dated.len() == 4
                && dated[0].len() == 4
                && dated[1].len() == 2
                && dated[2].len() == 2
                && dated[0].chars().all(char::is_numeric),
            "case {case:?}: expected the stream to open on a YYYY/MM/DD date, got {stream:?}"
        );

        // Every event carries an instant of the run, and the batch is ordered by it.
        let stamps: Vec<i64> = events(&router, case, &group, &stream)
            .await
            .iter()
            .map(|(at, _)| *at)
            .collect();
        assert!(
            stamps.iter().all(|at| *at > 0) && stamps.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: expected ordered instants of the run, got {stamps:?}"
        );

        // The destination the RUNTIME told the handler about is the destination the delivery
        // used, so a handler writing to its own log group addresses the group it logs to.
        if row.reports_destination {
            assert_eq!(
                payload["group"],
                json!(group.clone()),
                "case {case:?}: expected the runtime to report the group it logs to, got \
                 {payload}"
            );
            assert_eq!(
                payload["stream"],
                json!(stream.clone()),
                "case {case:?}: expected the runtime to report the stream it wrote, got \
                 {payload}"
            );
        }
    }
}

/// Two invocations of one function are two runs, so each writes its own log — and both write
/// it to the stream of the EXECUTION ENVIRONMENT that served them: an environment is kept
/// after an invocation and serves the next one, and it writes to ONE stream for its whole life,
/// so the two runs' logs sit in that one stream in the order they ran. That is what makes one
/// invocation's output readable apart from another's inside it: each run's records are framed
/// by its own `START`/`END`/`REPORT` lines naming its own request id.
#[tokio::test]
async fn the_invocations_one_environment_serves_write_their_logs_to_its_one_stream() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "two invocations of one function";
    let router = emulator();
    create_function(
        &router,
        &Created {
            case,
            name: "log-twice",
            source: "exports.handler = async () => {\n    console.log('a run');\n    return { \
                     ok: true };\n};",
            runtime: NODE_RUNTIME,
            role: UNKNOWN_ROLE,
            log_group: None,
        },
    )
    .await;
    let group = "/aws/lambda/log-twice";
    let (first, _) = invoke(&router, case, "log-twice").await;
    let (second, _) = invoke(&router, case, "log-twice").await;
    assert_ne!(
        first, second,
        "case {case:?}: two runs are two invocations, so their logs name different ids"
    );
    let held = streams(&router, case, group).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: the second invocation is served by the environment the first one left, \
         which writes to one stream, got {held:?}"
    );
    let (records, _) = delivered(&router, case, group, &held[0]).await;
    let rebuilt = invocations_log(&records);
    assert_eq!(
        rebuilt,
        format!("{first}{second}"),
        "case {case:?}: expected the stream to hold both runs' logs whole, in the order they \
         ran, got {rebuilt:?}"
    );
}

/// One row of the permission table: the logs grant the execution role carries, and what that
/// leaves in CloudWatch Logs.
struct Grant {
    case: &'static str,
    /// The statements of the role's one inline policy.
    statements: Value,
    /// Whether the group has to exist after the invocation.
    group: bool,
    /// Whether a stream of it has to exist.
    stream: bool,
    /// Whether that stream has to hold the run's events.
    events: bool,
}

fn grant_rows() -> Vec<Grant> {
    vec![
        Grant {
            case: "a role granted every log write",
            statements: json!([allow(LOGS, &LOG_WRITES, "*")]),
            group: true,
            stream: true,
            events: true,
        },
        Grant {
            case: "a role granted the writes on its own log group only",
            statements: json!([allow(
                LOGS,
                &LOG_WRITES,
                "arn:aws:logs:us-east-1:000000000000:log-group:/aws/lambda/*"
            )]),
            group: true,
            stream: true,
            events: true,
        },
        Grant {
            case: "a role granted no log write at all",
            statements: json!([allow("s3", &["GetObject"], "*")]),
            group: false,
            stream: false,
            events: false,
        },
        Grant {
            case: "a role granted everything but the put",
            statements: json!([allow(LOGS, &["CreateLogGroup", "CreateLogStream"], "*")]),
            group: true,
            stream: true,
            events: false,
        },
        Grant {
            case: "a role whose policy denies the put beside a wildcard grant",
            statements: json!([
                allow(LOGS, &LOG_WRITES, "*"),
                json!({
                    "Effect": "Deny",
                    "Action": "logs:PutLogEvents",
                    "Resource": "*",
                }),
            ]),
            group: true,
            stream: true,
            events: false,
        },
    ]
}

/// What the execution role may do decides what an invocation leaves behind, and a withheld
/// write is never an invocation failure: the run still answers.
#[tokio::test]
async fn the_execution_role_decides_what_an_invocation_leaves_in_the_log_group() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    for (index, row) in grant_rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let role = given_role(
            &router,
            case,
            &format!("log-writer-{index}"),
            Some(row.statements.clone()),
        )
        .await;
        let name = format!("log-grant-{index}");
        let group = format!("/aws/lambda/{name}");
        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source: "exports.handler = async () => {\n    console.log('a guarded run');\n  \
                         return { ok: true };\n};",
                runtime: NODE_RUNTIME,
                role: &role,
                log_group: None,
            },
        )
        .await;
        // The invocation itself is answered whatever became of its log.
        let (reported, _) = invoke(&router, case, &name).await;
        assert!(
            reported.contains("a guarded run"),
            "case {case:?}: expected the run's own log on the answer, got {reported:?}"
        );

        let found = groups(&router, case, &group).await;
        assert_eq!(
            !found.is_empty(),
            row.group,
            "case {case:?}: expected the group to exist: {}, got {found:?}",
            row.group
        );
        let held = if row.group {
            streams(&router, case, &group).await
        } else {
            Vec::new()
        };
        assert_eq!(
            !held.is_empty(),
            row.stream,
            "case {case:?}: expected a stream to exist: {}, got {held:?}",
            row.stream
        );
        let carried = match held.first() {
            Some(stream) => events(&router, case, &group, stream).await,
            None => Vec::new(),
        };
        assert_eq!(
            !carried.is_empty(),
            row.events,
            "case {case:?}: expected the run's events to be held: {}, got {} event(s)",
            row.events,
            carried.len()
        );
    }
}

/// A handler that RAISES still logs: the error report its own runtime writes — the error's
/// class and message, then the handler's traceback — is in the group beside the framing, which
/// is how a client reads what went wrong with an invocation that answered an error document.
#[tokio::test]
async fn a_handler_that_raises_logs_its_runtimes_error_report() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a python handler that raises";
    let router = emulator();
    create_function(
        &router,
        &Created {
            case,
            name: "log-raiser",
            source: "def lambda_handler(event, context):\n    raise ValueError('the handler \
                     gave up')\n",
            runtime: PYTHON_RUNTIME,
            role: UNKNOWN_ROLE,
            log_group: None,
        },
    )
    .await;
    let group = "/aws/lambda/log-raiser";
    let (reported, _) = invoke(&router, case, "log-raiser").await;
    let held = streams(&router, case, group).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected one stream for the invocation, got {held:?}"
    );
    let (lines, _text) = delivered(&router, case, group, &held[0]).await;
    let rebuilt = invocations_log(&lines);
    assert_eq!(
        rebuilt, reported,
        "case {case:?}: expected the group to hold the log the invocation reported, got \
         {rebuilt:?} against {reported:?}"
    );
    // The report is ONE record of the group — a failure is one thing that happened, so the
    // `[ERROR]` line, the traceback header and the frames are one event and not one per line.
    let report: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("[ERROR] "))
        .collect();
    assert_eq!(
        report.len(),
        1,
        "case {case:?}: expected the runtime's error report as one record, got {lines:?}"
    );
    for expected in [
        "[ERROR] ValueError: the handler gave up",
        "Traceback (most recent call last):",
    ] {
        assert!(
            report[0].lines().any(|line| line == expected),
            "case {case:?}: expected the line {expected:?} in the report, got {report:?}"
        );
    }
    assert!(
        report[0].contains("lambda_function.py"),
        "case {case:?}: expected the handler's own frame in the traceback, got {report:?}"
    );
}

/// The request header `Invoke$InvocationType` is bound to, and the two values the invocation
/// surfaces below are driven with (`Event | RequestResponse | DryRun` is the model's enum).
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";
/// See [`INVOCATION_TYPE_HEADER`]: "Invoke the function asynchronously […] The API response
/// only includes a status code", so the run happens off the response path.
const EVENT_TYPE: &str = "Event";
/// See [`INVOCATION_TYPE_HEADER`]: "Validate parameter values and verify that the user or
/// role has permission to invoke the function", which runs no function code at all.
const DRY_RUN_TYPE: &str = "DryRun";

/// How long the asynchronous case waits for the queued event to be delivered and run before
/// giving up, and how often it looks. The delivery is owed by the CLOCK a short span after the
/// accepting call, so the test POLLS for it rather than sleeping out a fixed span: it reads as
/// soon as the run has landed and fails only once the deadline has passed.
const ASYNC_DEADLINE: Duration = Duration::from_secs(10);
/// See [`ASYNC_DEADLINE`].
const ASYNC_POLL: Duration = Duration::from_millis(100);

/// `Invoke` the function `name` with the `InvocationType` `invocation_type`, answering the
/// status it was accepted with — which the caller states its expectation of. Neither type
/// carries a log tail — an asynchronous invocation is answered before its run and the dry run
/// runs nothing — so what the group holds is the only statement of what was logged.
async fn invoke_as(router: &MultiServiceRouter, name: &str, invocation_type: &str) -> u16 {
    let response = router
        .dispatch(invocation_request(
            name,
            &[(INVOCATION_TYPE_HEADER, invocation_type)],
        ))
        .await;
    response.status().as_u16()
}

/// The group's streams and the LINES the one of them holds, once the delivery has put them
/// there, looked for until [`ASYNC_DEADLINE`] passes.
///
/// A delivery made off the response path has not necessarily happened when the accepting call
/// is answered, and it opens the stream before it writes to it, so this reads until the
/// EVENTS are there rather than until the stream is — which is what a client polling for a
/// run's output does.
async fn delivery_to(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let deadline = std::time::Instant::now() + ASYNC_DEADLINE;
    loop {
        let found = groups(router, case, group).await;
        if !found.is_empty() {
            let held = streams(router, case, group).await;
            if let Some(stream) = held.first() {
                let (lines, _) = delivered(router, case, group, stream).await;
                if !lines.is_empty() {
                    return (found, held, lines);
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the queued invocation's log never reached {group:?} within \
             {ASYNC_DEADLINE:?}"
        );
        tokio::time::sleep(ASYNC_POLL).await;
    }
}

/// An ASYNCHRONOUS invocation's run is logged like any other: the accepting call says nothing
/// about the run, so the group is where a client reads what the handler wrote — and a DRY RUN,
/// which runs no function code, logs nothing at all.
///
/// Both surfaces are driven against ONE function, so the two answers differ only in what the
/// surface did: one ran the handler and one did not.
#[tokio::test]
async fn an_asynchronous_run_is_logged_and_a_dry_run_is_not() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "an asynchronous invocation beside a dry run";
    let router = emulator();
    create_function(
        &router,
        &Created {
            case,
            name: "log-queued",
            source: "exports.handler = async () => {\n    console.log('a queued run');\n    \
                     return { ok: true };\n};",
            runtime: NODE_RUNTIME,
            role: UNKNOWN_ROLE,
            log_group: None,
        },
    )
    .await;
    let group = "/aws/lambda/log-queued";

    // The dry run FIRST: it "validate[s] parameter values" and invokes nothing, so it leaves
    // the group that does not exist yet still not existing.
    let dry = invoke_as(&router, "log-queued", DRY_RUN_TYPE).await;
    assert_eq!(
        dry, 204,
        "case {case:?}: the dry run answers 204, got {dry}"
    );
    let after_dry_run = groups(&router, case, group).await;
    assert!(
        after_dry_run.is_empty(),
        "case {case:?}: a dry run invokes nothing, so it logs nothing, got {after_dry_run:?}"
    );

    // And the asynchronous one, whose run happens after the call was answered.
    let accepted = invoke_as(&router, "log-queued", EVENT_TYPE).await;
    assert_eq!(
        accepted, 202,
        "case {case:?}: an asynchronous invocation is accepted with 202, got {accepted}"
    );

    let (found, held, lines) = delivery_to(&router, case, group).await;
    assert_eq!(
        found,
        vec![group.to_string()],
        "case {case:?}: expected the delivered run to create {group:?}, got {found:?}"
    );
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected one stream for the one run, got {held:?}"
    );
    // The whole log of the run, in order: the environment's initialization, then the framing
    // around the one line the handler wrote.
    let expected_lines = 5;
    assert_eq!(
        lines.len(),
        expected_lines,
        "case {case:?}: expected the framing around the handler's one line, got {lines:?}"
    );
    assert!(
        lines[0].starts_with("INIT_START Runtime Version: "),
        "case {case:?}: expected the log to open on the environment's initialization, got \
         {lines:?}"
    );
    assert!(
        lines[1].starts_with("START RequestId: "),
        "case {case:?}: expected the invocation's START line after it, got {lines:?}"
    );
    assert_eq!(
        lines[2].rsplit('\t').next().unwrap_or_default(),
        "a queued run",
        "case {case:?}: expected the handler's own line, got {lines:?}"
    );
    assert!(
        lines[3].starts_with("END RequestId: ") && lines[4].starts_with("REPORT RequestId: "),
        "case {case:?}: expected the log to close on END and then REPORT, got {lines:?}"
    );
}

/// Every function this fixture creates carries the run bounds of [`function_timeout_seconds`]
/// and [`function_memory_size_mb`], read back through `GetFunctionConfiguration` — the one
/// surface a client reads them from — so neither bound can be tightened back into a limit the
/// HOST's own load decides.
///
/// No row of these suites is about either bound: what they assert is where a log goes, and a
/// run of a handler that writes a line or two reaches neither. A run cut off on a tight one is
/// a FAILED invocation — reported as a run that timed out or whose runtime was killed — so a
/// suite that runs handlers beside the other suites of its test binary would pass or fail by
/// how much wall clock and how much of this process's memory those other suites happened to
/// be taking.
///
/// The rows are the two shapes of function the fixture creates — the default group and one the
/// configuration names — and each runtime it uploads a package for, because the bounds are the
/// function's and belong to neither.
#[tokio::test]
async fn every_function_of_these_suites_is_created_with_room_for_the_hosts_own_load() {
    const PYTHON_SOURCE: &str = "def lambda_handler(event, context):\n    return {}\n";
    const NODE_SOURCE: &str = "exports.handler = async () => ({ ok: true });";
    let rows: [(&str, &str, &str, Option<&str>); 3] = [
        ("the default group, python", PYTHON_RUNTIME, PYTHON_SOURCE, None),
        ("the default group, node", NODE_RUNTIME, NODE_SOURCE, None),
        (
            "a group the configuration names",
            NODE_RUNTIME,
            NODE_SOURCE,
            Some("/aws/lambda/bounds-named-elsewhere"),
        ),
    ];
    for (index, (case, runtime, source, log_group)) in rows.iter().enumerate() {
        let router = emulator();
        let name = format!("log-bounds-{index}");
        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source,
                runtime,
                role: UNKNOWN_ROLE,
                log_group: *log_group,
            },
        )
        .await;
        let read = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetFunctionConfiguration answered {} {}",
            read.status,
            read.text()
        );
        let configuration = read.json();
        let memory_size = configuration["MemorySize"]
            .as_i64()
            .and_then(|size| i32::try_from(size).ok())
            .unwrap_or_default();
        let timeout = configuration["Timeout"]
            .as_i64()
            .and_then(|seconds| i32::try_from(seconds).ok())
            .unwrap_or_default();

        // What the create asked for is what the service reports: the bounds a run is judged
        // against are the ones a client reads off the function, not ones the create quietly
        // clamped or dropped.
        assert_eq!(
            (memory_size, timeout),
            (function_memory_size_mb(), function_timeout_seconds()),
            "case {case:?}: GetFunctionConfiguration must report the bounds {name:?} was created \
             with, got {}",
            read.text()
        );

        // The memory bound: the run holds the whole per-run ceiling, which is what no larger
        // `MemorySize` can add to.
        assert_eq!(
            run_heap_bytes(memory_size),
            run_heap_bytes(i32::MAX),
            "case {case:?}: a run of {name:?} must hold the per-run ceiling, so this process's \
             own growth cannot reach it; MemorySize {memory_size} gives it {} bytes against the \
             ceiling's {}",
            run_heap_bytes(memory_size),
            run_heap_bytes(i32::MAX)
        );

        // And the wall clock: every second the host waits for a run but the grace it gives one
        // past its budget.
        let budget = run_budget(
            timeout,
            memory_size,
            service_com_amazonaws_lambda_awsgirapiservice::state::DEFAULT_EPHEMERAL_STORAGE_MB,
            false,
        );
        let least = HOST_WAIT_CEILING.saturating_sub(ABANDON_GRACE);
        assert!(
            budget.wall_clock >= least,
            "case {case:?}: a run of {name:?} must be given at least {least:?} of wall clock, \
             got {:?} from Timeout {timeout}",
            budget.wall_clock
        );
    }
}
