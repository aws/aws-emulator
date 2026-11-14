// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the X-RAY TRACE an execution publishes about itself: Step Functions
//! and AWS X-Ray on ONE engine, the trace read back where a client reads it
//! (`GetTraceSummaries` over the window the run happened in, then `BatchGetTraces` for the
//! ids it listed).
//!
//! The RULE under test: **a state machine's `tracingConfiguration` decides whether its
//! executions are recorded at X-Ray — a run of a state machine whose configuration is
//! enabled belongs to the chain the caller's trace header named (or to one started for it
//! where the caller named none) and has its segments submitted to X-Ray once the run has
//! ended, so the run's trace is retrievable there; a run of a state machine whose
//! configuration is disabled is retrievable nowhere, whatever header the start carried.**
//! Beside the configuration, two readings belong to the trace rather than to the run: the
//! SAMPLING DECISION is the caller's where its header took one (`Sampled=0` is a run that
//! happens and whose trace is not recorded) and the state machine's own where it did not,
//! and the CHAIN is the caller's `Root` where it named one in X-Ray's own form and one
//! started for the run where it named none or named text that is not a trace id.
//!
//! The rows differ along what the rule generalises over: what the caller supplied (a sampled
//! chain, a chain with no decision, an unsampled chain, a chain with a parent span of its
//! own, a root that is not a trace id, no header at all), which state machine the run was on
//! (tracing, not tracing), which WAY the run was started (`StartSyncExecution`, which ends
//! within the call, and `StartExecution`, whose outcome the clock owes) and which terminal
//! status it reached (succeeded, failed, stopped by the client).
//!
//! Each row carries a POSITIVE CONTROL: one further run of the tracing state machine that
//! must always be recorded. The assertions are made once that control's trace is
//! retrievable, so an absent trace is a trace that was never published rather than one still
//! in flight, and nothing here waits on a fixed delay for a publication.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-west-2";

/// A role ARN inside the modelled `Arn` length (1..256). No IAM store is mounted here, so it
/// is a value the create carries rather than a record this suite relies on.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// The two state machines every row runs on: identical but for their
/// `tracingConfiguration.enabled`.
const TRACED: &str = "traced-twin";
const UNTRACED: &str = "untraced-twin";

/// A definition whose two `Pass` states succeed, so a recorded run has two states to report
/// and their order is observable.
const TWO_STATES: &str = r#"{"StartAt":"First","States":{"First":{"Type":"Pass","Next":"Second"},"Second":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// A definition whose `Fail` state ends the run with its own error.
const FAILS: &str =
    r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// A definition that keeps a run going long enough for the client to stop it.
const WAITS: &str = r#"{"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":30,"Next":"Done"},"Done":{"Type":"Pass","End":true}}}"#;

/// The `origin` the segment of a Step Functions execution carries, spelled out here rather
/// than imported from the implementation, so a wrong constant in the service cannot make a
/// test pass.
const STATE_MACHINE_ORIGIN: &str = "AWS::StepFunctions::StateMachine";

/// A trace id of X-Ray's own form (`1-<8 hex>-<24 hex>`), and a span id (16 hex digits), for
/// the rows whose caller supplies a chain of its own.
const SUPPLIED_ROOT: &str = "1-5759e988-bd862e3fe1be46a994272793";
const SUPPLIED_SPAN: &str = "53995c3f42cd8ad8";

/// How long a row waits for its positive control's trace, and how often it looks. Generous:
/// the deadline is a backstop against a publication that never happens, not a measurement.
const DEADLINE: Duration = Duration::from_secs(30);
const LOOK: Duration = Duration::from_millis(50);

/// One emulator: Step Functions and X-Ray mounted on ONE shared engine.
///
/// X-Ray is mounted BESIDE Step Functions because a run's segments are submitted through the
/// in-process callback to X-Ray's own `PutTraceSegments`: with it absent there is nowhere to
/// publish, so the path under test would not be exercised at all. Each service claims only
/// the requests scoped to its own signing name.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what
    /// routes the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        )
    }

    /// One Step Functions call over its `awsJson1_0` wire, requiring a 200.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let host = format!("states.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", &host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
            .header("authorization", Self::authorization("states"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One X-Ray call over its `restJson1` wire, requiring a 200.
    async fn xray(&self, path: &str, body: &Value) -> Value {
        let host = format!("xray.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", &host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("xray"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "POST {path}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// Every segment document X-Ray holds for the window the runs of this suite happened in:
    /// `GetTraceSummaries` over that window, then `BatchGetTraces` for the ids it listed —
    /// the two reads a client finds a trace with.
    async fn recorded_documents(&self) -> Vec<Value> {
        let ids = self.listed_traces(None).await;
        if ids.is_empty() {
            return Vec::new();
        }
        let traces = self.xray("/Traces", &json!({ "TraceIds": ids })).await;
        traces
            .get("Traces")
            .and_then(Value::as_array)
            .map(|traces| {
                traces
                    .iter()
                    .flat_map(|trace| {
                        trace
                            .get("Segments")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .filter_map(|segment| {
                        segment
                            .get("Document")
                            .and_then(Value::as_str)
                            .and_then(|text| serde_json::from_str::<Value>(text).ok())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The trace ids `GetTraceSummaries` lists over the window the runs of this suite
    /// happened in, narrowed by `filter` where one is given.
    async fn listed_traces(&self, filter: Option<&str>) -> Vec<Value> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        // A window that starts an hour back and ends ten minutes ahead: well inside the
        // span and the clock skew the read admits, so what it lists is decided by the
        // traces and not by the bounds.
        let mut request = json!({ "StartTime": now - 3_600.0, "EndTime": now + 600.0 });
        if let (Some(filter), Some(object)) = (filter, request.as_object_mut()) {
            object.insert("FilterExpression".to_string(), json!(filter));
        }
        let summaries = self.xray("/TraceSummaries", &request).await;
        summaries
            .get("TraceSummaries")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("Id").cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The segment X-Ray holds for the execution `execution_arn`, out of the documents in the
    /// window, or `None` where it holds none: a run's own segment names the execution it
    /// reports in its annotations.
    async fn segment_of(&self, execution_arn: &str) -> Option<Value> {
        self.recorded_documents()
            .await
            .into_iter()
            .find(|document| annotation(document, "execution_arn").as_deref() == Some(execution_arn))
    }

    /// Wait until X-Ray holds the segment of `execution_arn` — the POSITIVE CONTROL every row
    /// carries — and answer it.
    ///
    /// The wait is for the CONDITION: a publication that has happened is observable, so this
    /// looks again until it is, and the deadline is only a backstop against one that never
    /// happens.
    async fn awaited_segment(&self, case: &str, execution_arn: &str) -> Value {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(segment) = self.segment_of(execution_arn).await {
                return segment;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: X-Ray never held a segment for the run {execution_arn:?}"
            );
            ::tokio::time::sleep(LOOK).await;
        }
    }

    /// One state machine of `sm_type` running `definition`, with tracing `enabled` or not.
    /// Answers its ARN.
    async fn given_state_machine(
        &self,
        name: &str,
        definition: &str,
        sm_type: &str,
        enabled: bool,
    ) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                    "type": sm_type,
                    "tracingConfiguration": { "enabled": enabled },
                }),
            )
            .await;
        text(&created, "stateMachineArn")
    }
}

/// The text member `name` of `body`.
fn text(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a {name:?} in {body}"))
        .to_string()
}

/// The annotation `name` a segment document carries, as text.
fn annotation(document: &Value, name: &str) -> Option<String> {
    document
        .get("annotations")
        .and_then(|annotations| annotations.get(name))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `name` of each subsegment of `document`, in the order it carries them.
fn subsegment_names(document: &Value) -> Vec<String> {
    document
        .get("subsegments")
        .and_then(Value::as_array)
        .map(|subsegments| {
            subsegments
                .iter()
                .filter_map(|subsegment| subsegment.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The boolean member `name` of `document`, which a segment carries for each outcome flag.
fn flag(document: &Value, name: &str) -> bool {
    document
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or_default()
}

/// What the caller supplied and what the trace of its run must then be.
struct Row {
    case: &'static str,
    /// The `traceHeader` the start carries, or `None` to send none at all.
    header: Option<&'static str>,
    /// Whether the run on the TRACING state machine is recorded at X-Ray.
    recorded: bool,
    /// The chain the recorded run must belong to, or `None` where it starts one of its own.
    chain: Option<&'static str>,
    /// The span the recorded run's segment must hang from, or `None` where it is the root of
    /// its trace.
    parent: Option<&'static str>,
}

/// The table every row of the two start paths below is driven over: what the caller supplied
/// is what the trace's chain, parent and recording follow from.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a sampled chain",
            header: Some("Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=1"),
            recorded: true,
            chain: Some(SUPPLIED_ROOT),
            parent: None,
        },
        Row {
            case: "a chain with no sampling decision",
            header: Some("Root=1-5759e988-bd862e3fe1be46a994272793"),
            recorded: true,
            chain: Some(SUPPLIED_ROOT),
            parent: None,
        },
        Row {
            case: "an unsampled chain",
            header: Some("Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=0"),
            recorded: false,
            chain: None,
            parent: None,
        },
        Row {
            case: "a sampled chain naming a parent span",
            header: Some(
                "Root=1-5759e988-bd862e3fe1be46a994272793;Parent=53995c3f42cd8ad8;Sampled=1",
            ),
            recorded: true,
            chain: Some(SUPPLIED_ROOT),
            parent: Some(SUPPLIED_SPAN),
        },
        Row {
            case: "a root that is not a trace id",
            header: Some("Root=not-a-trace-id;Sampled=1"),
            recorded: true,
            chain: None,
            parent: None,
        },
        Row {
            // A tracing state machine records the runs it is asked about, so a caller that
            // took no decision at all gets one.
            case: "no trace header at all",
            header: None,
            recorded: true,
            chain: None,
            parent: None,
        },
    ]
}

/// Add the row's `traceHeader` to a start request.
fn with_header(mut request: Value, header: Option<&str>) -> Value {
    if let (Some(header), Some(object)) = (header, request.as_object_mut()) {
        object.insert("traceHeader".to_string(), json!(header));
    }
    request
}

// ---------------------------------------------------------------------------
// A synchronous run, which ends inside the call that started it
// ---------------------------------------------------------------------------

/// `tracingConfiguration` decides whether a SYNCHRONOUS run is recorded at X-Ray, and the
/// caller's header decides the chain it is recorded under.
#[tokio::test]
async fn tracing_configuration_decides_whether_a_synchronous_run_is_recorded() {
    for row in rows() {
        let emulator = Emulator::new();
        let traced = emulator
            .given_state_machine(TRACED, TWO_STATES, "EXPRESS", true)
            .await;
        let untraced = emulator
            .given_state_machine(UNTRACED, TWO_STATES, "EXPRESS", false)
            .await;

        let on_traced = emulator
            .sfn(
                "StartSyncExecution",
                &with_header(
                    json!({ "stateMachineArn": traced, "input": "{}" }),
                    row.header,
                ),
            )
            .await;
        let on_untraced = emulator
            .sfn(
                "StartSyncExecution",
                &with_header(
                    json!({ "stateMachineArn": untraced, "input": "{}" }),
                    row.header,
                ),
            )
            .await;
        // The positive control: a further run of the tracing state machine, which is
        // recorded whatever the row supplied.
        let control = emulator
            .sfn(
                "StartSyncExecution",
                &json!({ "stateMachineArn": traced, "input": "{}" }),
            )
            .await;

        // "The X-Ray trace header that was passed to the execution": what the caller
        // supplied is what the response echoes, and a caller that supplied none is echoed
        // none.
        assert_eq!(
            on_traced.get("traceHeader").and_then(Value::as_str),
            row.header,
            "case {:?}: expected the supplied traceHeader echoed, got {on_traced}",
            row.case
        );

        let control_arn = text(&control, "executionArn");
        let control_segment = emulator.awaited_segment(row.case, &control_arn).await;
        assert_eq!(
            control_segment.get("origin").and_then(Value::as_str),
            Some(STATE_MACHINE_ORIGIN),
            "case {:?}: expected the control's segment to name the state machine origin, \
             got {control_segment}",
            row.case
        );
        // And the trace is found the way a client looking for one service's traces finds
        // it: a query scoped to the state machine by the name and the type its segment
        // reports.
        let scoped = emulator
            .listed_traces(Some(&format!(
                "service(id(name: \"{TRACED}\", type: \"{STATE_MACHINE_ORIGIN}\"))"
            )))
            .await;
        assert!(
            scoped.contains(&control_segment["trace_id"]),
            "case {:?}: expected a query scoped to the state machine to list the control's \
             chain {:?}, got {scoped:?}",
            row.case,
            control_segment["trace_id"]
        );

        let arn = text(&on_traced, "executionArn");
        let recorded = emulator.segment_of(&arn).await;
        assert_eq!(
            recorded.is_some(),
            row.recorded,
            "case {:?}: expected recorded={} for the run on the tracing state machine, got \
             {recorded:?}",
            row.case,
            row.recorded
        );
        if let Some(document) = recorded {
            assert_eq!(
                document.get("name").and_then(Value::as_str),
                Some(TRACED),
                "case {:?}: expected the segment to name the state machine that ran it, got \
                 {document}",
                row.case
            );
            assert_eq!(
                document.get("resource_arn").and_then(Value::as_str),
                Some(traced.as_str()),
                "case {:?}: expected the segment to name the state machine's ARN, got \
                 {document}",
                row.case
            );
            let chain = document
                .get("trace_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match row.chain {
                Some(continued) => assert_eq!(
                    chain, continued,
                    "case {:?}: expected the caller's chain continued, got {chain:?}",
                    row.case
                ),
                None => assert!(
                    is_trace_id(&chain),
                    "case {:?}: expected a chain of X-Ray's own form, got {chain:?}",
                    row.case
                ),
            }
            assert_eq!(
                document.get("parent_id").and_then(Value::as_str),
                row.parent,
                "case {:?}: expected parent {:?}, got {document}",
                row.case,
                row.parent
            );
            assert_eq!(
                subsegment_names(&document),
                vec!["First".to_string(), "Second".to_string()],
                "case {:?}: expected one subsegment per state the run entered, got {document}",
                row.case
            );
            assert!(
                !flag(&document, "fault") && !flag(&document, "error"),
                "case {:?}: expected a succeeding run to carry neither flag, got {document}",
                row.case
            );
        }

        // The disabled twin is recorded nowhere, whatever the header said.
        let untraced_arn = text(&on_untraced, "executionArn");
        let untraced_segment = emulator.segment_of(&untraced_arn).await;
        assert!(
            untraced_segment.is_none(),
            "case {:?}: expected no segment for the run on the state machine whose tracing \
             is disabled, got {untraced_segment:?}",
            row.case
        );
    }
}

/// Whether `value` is a trace id of X-Ray's own form: `1-<8 hex>-<24 hex>`. Spelled out here
/// rather than imported, so the test states the form it expects.
fn is_trace_id(value: &str) -> bool {
    let fields: Vec<&str> = value.split('-').collect();
    let hex = |field: &str, digits: usize| {
        field.len() == digits && field.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    matches!(fields.as_slice(), ["1", time, unique] if hex(time, 8) && hex(unique, 24))
}

// ---------------------------------------------------------------------------
// An asynchronous run, whose outcome the clock owes
// ---------------------------------------------------------------------------

/// The same rule for a run started with `StartExecution`: the trace is published once the
/// run has ended, which is an outcome the clock makes due after the call has answered, so the
/// trace is owed with nobody calling this service again.
#[tokio::test]
async fn an_asynchronous_run_records_its_trace_once_the_clock_has_ended_it() {
    for row in rows() {
        let emulator = Emulator::new();
        let traced = emulator
            .given_state_machine(TRACED, TWO_STATES, "STANDARD", true)
            .await;
        let untraced = emulator
            .given_state_machine(UNTRACED, TWO_STATES, "STANDARD", false)
            .await;

        let on_traced = emulator
            .sfn(
                "StartExecution",
                &with_header(
                    json!({ "stateMachineArn": traced, "name": "row", "input": "{}" }),
                    row.header,
                ),
            )
            .await;
        let on_untraced = emulator
            .sfn(
                "StartExecution",
                &with_header(
                    json!({ "stateMachineArn": untraced, "name": "row", "input": "{}" }),
                    row.header,
                ),
            )
            .await;
        let control = emulator
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": traced, "name": "control", "input": "{}" }),
            )
            .await;

        let control_arn = text(&control, "executionArn");
        emulator.awaited_segment(row.case, &control_arn).await;

        let arn = text(&on_traced, "executionArn");
        let recorded = emulator.segment_of(&arn).await;
        assert_eq!(
            recorded.is_some(),
            row.recorded,
            "case {:?}: expected recorded={} for the asynchronous run on the tracing state \
             machine, got {recorded:?}",
            row.case,
            row.recorded
        );
        if let Some(document) = recorded {
            assert_eq!(
                annotation(&document, "status").as_deref(),
                Some("SUCCEEDED"),
                "case {:?}: expected the status the run reached, got {document}",
                row.case
            );
            assert_eq!(
                subsegment_names(&document),
                vec!["First".to_string(), "Second".to_string()],
                "case {:?}: expected one subsegment per state the run entered, got {document}",
                row.case
            );
            let started = document.get("start_time").and_then(Value::as_f64);
            let ended = document.get("end_time").and_then(Value::as_f64);
            assert!(
                matches!((started, ended), (Some(from), Some(to)) if to >= from),
                "case {:?}: expected the span the run occupied, got {document}",
                row.case
            );
        }

        let untraced_arn = text(&on_untraced, "executionArn");
        let untraced_segment = emulator.segment_of(&untraced_arn).await;
        assert!(
            untraced_segment.is_none(),
            "case {:?}: expected no segment for the asynchronous run on the state machine \
             whose tracing is disabled, got {untraced_segment:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// The outcome the segment reports
// ---------------------------------------------------------------------------

/// A recorded run reports the outcome it reached: a run the service ended is a FAULT of the
/// work the segment covers, a run the client stopped is the ERROR of the request that
/// stopped it, and a run that succeeded carries neither.
#[tokio::test]
async fn the_recorded_segment_reports_the_outcome_the_run_reached() {
    // (case, definition, whether the client stops the run, the status, fault, error)
    let rows: Vec<(&str, &str, bool, &str, bool, bool)> = vec![
        ("a run that succeeded", TWO_STATES, false, "SUCCEEDED", false, false),
        ("a run the service failed", FAILS, false, "FAILED", true, false),
        ("a run the client stopped", WAITS, true, "ABORTED", false, true),
    ];
    for (case, definition, stopped, status, fault, error) in rows {
        let emulator = Emulator::new();
        let traced = emulator
            .given_state_machine(TRACED, definition, "STANDARD", true)
            .await;
        let started = emulator
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": traced,
                    "name": "outcome",
                    "input": "{}",
                    "traceHeader": format!("Root={SUPPLIED_ROOT};Sampled=1"),
                }),
            )
            .await;
        let arn = text(&started, "executionArn");
        if stopped {
            emulator
                .sfn("StopExecution", &json!({ "executionArn": arn }))
                .await;
        }
        let document = emulator.awaited_segment(case, &arn).await;
        assert_eq!(
            annotation(&document, "status").as_deref(),
            Some(status),
            "case {case:?}: expected the status {status:?}, got {document}"
        );
        assert_eq!(
            flag(&document, "fault"),
            fault,
            "case {case:?}: expected fault={fault}, got {document}"
        );
        assert_eq!(
            flag(&document, "error"),
            error,
            "case {case:?}: expected error={error}, got {document}"
        );
        assert!(
            !flag(&document, "throttle"),
            "case {case:?}: expected no throttle on a run's own segment, got {document}"
        );
        assert_eq!(
            document.get("trace_id").and_then(Value::as_str),
            Some(SUPPLIED_ROOT),
            "case {case:?}: expected the caller's chain continued, got {document}"
        );
    }
}
