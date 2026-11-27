// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH execution environment serves a RESPONSE-STREAMING invocation:
//! `PutProvisionedConcurrencyConfig`
//! (`PUT /2019-09-30/functions/{FunctionName}/provisioned-concurrency`),
//! `GetProvisionedConcurrencyConfig` polled until the allocation reports itself ready, and
//! then `InvokeWithResponseStream`
//! (`POST /2021-11-15/functions/{FunctionName}/response-streaming-invocations`) of the
//! qualifier the allocation was made on.
//!
//! The rule under test: provisioned concurrency "initializes a requested number of execution
//! environments so that they are prepared to respond immediately to your function's
//! invocations", allocated per alias or version, so once the allocation of the qualifier a
//! call addressed is READY the invocations that fit inside it are the ones those initialised
//! environments serve and the handler reads
//! `AWS_LAMBDA_INITIALIZATION_TYPE=provisioned-concurrency`; every other invocation — one
//! past the allocation, one of a qualifier the allocation is not on, and one made while
//! Lambda is still making the allocation — is served on demand and reads `on-demand`. The
//! rule is the INVOCATION's, not one entry point's, so the response-streaming entry point
//! reports the same split `Invoke` does.
//!
//! The rows differ along what the rule generalises over: how much is allocated (one
//! environment, two, three), how many of those environments are already serving an
//! invocation when the row invokes (none, some, all of them), which kind of qualifier holds
//! the allocation (a published version, an alias), which qualifier the invocation addresses
//! (the one holding it, the version a provisioned alias resolves to, the unpublished version),
//! and whether the allocation is ready at all.
//!
//! What is NOT asserted here, because it is asserted elsewhere:
//! `crate::provisioned_concurrency_warmup_convergence` pins that an accepted allocation
//! converges on `READY` with everything it asked for allocated,
//! `crate::provisioned_concurrency_invocation_series` what the split is metered as,
//! `crate::provisioned_concurrency` that the unpublished version holds no allocation at all, and
//! `crate::response_stream_trailer` the framing of the event stream itself.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_smithy_eventstream::frame::{DecodedFrame, MessageFrameDecoder};
use aws_smithy_types::event_stream::HeaderValue;
use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// Where `CreateFunction`, `PublishVersion` and `CreateAlias` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `State` a function settles in, from the model's `State` enum — the state a client waits
/// for before it configures or invokes the function.
const STATE_ACTIVE: &str = "Active";

/// The `ProvisionedConcurrencyStatusEnum` values an allocation moves through
/// (`smithy_query(query="shape", shape="ProvisionedConcurrencyStatusEnum")`):
/// `IN_PROGRESS | READY | FAILED`.
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_IN_PROGRESS`].
const STATUS_READY: &str = "READY";

/// What `AWS_LAMBDA_INITIALIZATION_TYPE` states for an invocation served by one of the
/// environments provisioned concurrency keeps initialised, and for one served by an environment
/// Lambda started on demand (the AWS Lambda runtime environment-variable reference).
const PROVISIONED_CONCURRENCY: &str = "provisioned-concurrency";
/// See [`PROVISIONED_CONCURRENCY`].
const ON_DEMAND: &str = "on-demand";

/// How long a poll may look for its condition before the case gives up — generous, so a loaded
/// host cannot turn a condition that holds into a failure. Every poll here waits for the
/// CONDITION and never for a fixed span, and nothing asserts how long one took.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How far ahead the warm-up of the row whose allocation is STILL BEING MADE is held: the move
/// to ready is owed by the clock, and a due instant this far out has not come however loaded
/// the host is. Nothing waits for it.
const WARM_UP_HELD_OPEN: Duration = Duration::from_secs(3600);

/// The alias every row creates, pointing at the one published version.
const ALIAS: &str = "live";

/// The published version that alias names.
const VERSION: &str = "1";

/// One decoded event-stream frame: the union member it carries and its payload.
#[derive(Debug)]
struct Frame {
    event_type: Option<String>,
    payload: Vec<u8>,
}

impl Frame {
    /// The frame's payload as JSON, when it is JSON at all.
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.payload).ok()
    }
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn number(&self, member: &str) -> Option<i64> {
        self.json().get(member).and_then(Value::as_i64)
    }

    /// The frames of the event stream this answer's body is.
    fn frames(&self, case: &str) -> Vec<Frame> {
        let mut decoder = MessageFrameDecoder::new();
        let mut remaining: &[u8] = &self.body;
        let mut frames = Vec::new();
        while !remaining.is_empty() {
            match decoder.decode_frame(&mut remaining) {
                Ok(DecodedFrame::Complete(message)) => {
                    let event_type = message
                        .headers()
                        .iter()
                        .find(|header| header.name().as_str() == ":event-type")
                        .and_then(|header| match header.value() {
                            HeaderValue::String(value) => Some(value.as_str().to_string()),
                            _ => None,
                        });
                    frames.push(Frame {
                        event_type,
                        payload: message.payload().to_vec(),
                    });
                }
                Ok(DecodedFrame::Incomplete) => {
                    panic!("case {case:?}: the event stream ends mid-frame")
                }
                Err(error) => panic!(
                    "case {case:?}: the event stream is not decodable: {error}: {:?}",
                    self.text()
                ),
            }
        }
        frames
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// A .zip deployment package holding one streaming handler that reports the initialization type
/// of the environment serving it, which is the value its own runtime was told.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file("index.js", options).expect("the entry");
        writer
            .write_all(
                b"exports.handler = awslambda.streamifyResponse(\n\
                  \x20   async (event, responseStream) => {\n\
                  \x20       responseStream.write(JSON.stringify({\n\
                  \x20           initType: process.env.AWS_LAMBDA_INITIALIZATION_TYPE,\n\
                  \x20       }));\n\
                  \x20       responseStream.end();\n\
                  \x20   },\n\
                  );\n",
            )
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Where a function's provisioned concurrency operations are bound.
fn concurrency_uri(function: &str, qualifier: &str) -> String {
    format!("/2019-09-30/functions/{function}/provisioned-concurrency?Qualifier={qualifier}")
}

/// Create the streaming function `name`, WAIT for it by polling its configuration until it
/// reports itself `Active`, then publish [`VERSION`] and point [`ALIAS`] at it — so an
/// allocation can be made on either kind of qualifier and the version behind the alias
/// addressed on its own.
async fn create_active_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "nodejs20.x",
            "Handler": "index.handler",
            "MemorySize": 256,
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    // "During this time, you can't invoke or modify the function": the move to `Active` is owed
    // by the clock, so this looks again until the function reports it.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let read = call(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        if read.string("State").as_deref() == Some(STATE_ACTIVE) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: function {name:?} never reported {STATE_ACTIVE}, last {}",
            read.text()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let published = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/versions"),
        &json!({}),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "case {case:?}: PublishVersion must succeed, got {} {}",
        published.status,
        published.text()
    );
    assert_eq!(
        published.string("Version").as_deref(),
        Some(VERSION),
        "case {case:?}: the first published version is {VERSION}, got {}",
        published.text()
    );
    let alias = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &json!({ "Name": ALIAS, "FunctionVersion": VERSION }),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "case {case:?}: CreateAlias must succeed, got {} {}",
        alias.status,
        alias.text()
    );
}

/// Ask for `requested` provisioned environments of `qualifier`, and then POLL the reader until
/// the allocation reports itself [`STATUS_READY`] with all of it allocated — the state a client
/// waits for before it cuts traffic over. Nothing but the clock moves it.
async fn allocate_until_ready(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    qualifier: &str,
    requested: i64,
) {
    let accepted = call(
        router,
        "PUT",
        &concurrency_uri(function, qualifier),
        &json!({ "ProvisionedConcurrentExecutions": requested }),
    )
    .await;
    assert_eq!(
        accepted.status,
        202,
        "case {case:?}: PutProvisionedConcurrencyConfig({requested}) must be accepted, got {} {}",
        accepted.status,
        accepted.text()
    );
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let read = call(
            router,
            "GET",
            &concurrency_uri(function, qualifier),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetProvisionedConcurrencyConfig must answer 200, got {} {}",
            read.status,
            read.text()
        );
        if read.string("Status").as_deref() == Some(STATUS_READY) {
            assert_eq!(
                read.number("AllocatedProvisionedConcurrentExecutions"),
                Some(requested),
                "case {case:?}: a ready allocation reports all {requested} of it allocated, got {}",
                read.text()
            );
            assert_eq!(
                read.number("AvailableProvisionedConcurrentExecutions"),
                Some(requested),
                "case {case:?}: a ready allocation reports all {requested} of it available, got {}",
                read.text()
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the allocation never became {STATUS_READY}, last {}",
            read.text()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Ask for `requested` provisioned environments of `qualifier` and leave the allocation STILL
/// BEING MADE: the move to ready is owed by the clock, so holding the due instant
/// [`WARM_UP_HELD_OPEN`] out is a warm-up that has not finished — which is what an invocation
/// arriving inside the window meets, without this suite waiting for anything.
async fn allocate_still_in_progress(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    function: &str,
    qualifier: &str,
    requested: i64,
) {
    let accepted = call(
        router,
        "PUT",
        &concurrency_uri(function, qualifier),
        &json!({ "ProvisionedConcurrentExecutions": requested }),
    )
    .await;
    assert_eq!(
        accepted.status,
        202,
        "case {case:?}: PutProvisionedConcurrencyConfig({requested}) must be accepted, got {} {}",
        accepted.status,
        accepted.text()
    );
    {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let record = store
            .functions
            .get_mut(function)
            .expect("the function this row created");
        let config = record
            .provisioned_concurrency_configs
            .get_mut(qualifier)
            .expect("the allocation this row asked for");
        config.ready_due = Some(SystemTime::now() + WARM_UP_HELD_OPEN);
    }
    let read = call(
        router,
        "GET",
        &concurrency_uri(function, qualifier),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read.string("Status").as_deref(),
        Some(STATUS_IN_PROGRESS),
        "case {case:?}: the allocation must still be being made, got {}",
        read.text()
    );
    assert_eq!(
        read.number("AllocatedProvisionedConcurrentExecutions"),
        Some(0),
        "case {case:?}: nothing is allocated while the allocation is being made, got {}",
        read.text()
    );
}

/// Leave `count` of `qualifier`'s provisioned environments SERVING an invocation, which is what
/// a concurrent caller of the same qualifier leaves behind: the per-qualifier in-flight count is
/// what the admission reads to decide whether the next invocation fits inside the allocation.
///
/// The count is SET rather than added to, and each row runs on an engine of its own with no
/// invocation of its own in flight yet, so what the admission reads is exactly the row's.
fn occupy_provisioned_environments(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    qualifier: &str,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let record = store
        .functions
        .get_mut(function)
        .expect("the function this row created");
    record
        .in_flight_by_qualifier
        .insert(qualifier.to_string(), count);
    record.concurrent_executions = record.concurrent_executions.saturating_add(count);
}

/// One `InvokeWithResponseStream` of `function`, addressed at `qualifier` when the row names
/// one.
async fn invoke_with_response_stream(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    call(
        router,
        "POST",
        &format!("/2021-11-15/functions/{function}/response-streaming-invocations{query}"),
        &json!({}),
    )
    .await
}

/// What the handler of one streaming invocation reported as its own initialization type, taken
/// out of the chunk it wrote — so the value asserted is the one the RUNTIME was told and not one
/// the control plane reported.
fn reported_initialization_type(answer: &Answer, case: &str) -> String {
    let frames = answer.frames(case);
    assert_eq!(
        frames
            .last()
            .and_then(|frame| frame.event_type.clone())
            .as_deref(),
        Some("InvokeComplete"),
        "case {case:?}: the stream ends with the completion event, got {frames:?}"
    );
    let reported: Vec<String> = frames
        .iter()
        .filter(|frame| frame.event_type.as_deref() == Some("PayloadChunk"))
        .filter_map(Frame::json)
        .filter_map(|chunk| {
            chunk
                .get("initType")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the handler writes exactly one chunk naming its own environment, got \
         {frames:?}"
    );
    reported[0].clone()
}

/// Which allocation a row makes, if any.
#[derive(Clone, Copy)]
enum Allocation {
    /// `requested` environments of `qualifier`, polled until the allocation is ready.
    Ready {
        qualifier: &'static str,
        requested: i64,
    },
    /// `requested` environments of `qualifier`, with the warm-up still under way.
    StillBeingMade {
        qualifier: &'static str,
        requested: i64,
    },
    /// Nothing provisioned at all.
    None,
}

/// One row: what is allocated, how much of it is already busy, which qualifier the streaming
/// invocation addresses, and the initialization type its handler must then read.
struct Row {
    case: &'static str,
    allocation: Allocation,
    /// How many of the allocation's environments are already serving an invocation.
    occupied: i32,
    /// The qualifier the invocation names, or `None` for an unqualified invocation.
    invoked: Option<&'static str>,
    expected: &'static str,
}

/// The rows. Each expectation is written out rather than computed, so a row that reports the
/// split and one that reports a single value everywhere cannot both pass.
const ROWS: &[Row] = &[
    Row {
        case: "one environment on the published version, none of it busy",
        allocation: Allocation::Ready {
            qualifier: VERSION,
            requested: 1,
        },
        occupied: 0,
        invoked: Some(VERSION),
        expected: PROVISIONED_CONCURRENCY,
    },
    Row {
        case: "three environments on the published version, two of them busy",
        allocation: Allocation::Ready {
            qualifier: VERSION,
            requested: 3,
        },
        occupied: 2,
        invoked: Some(VERSION),
        expected: PROVISIONED_CONCURRENCY,
    },
    Row {
        case: "an alias holding the allocation",
        allocation: Allocation::Ready {
            qualifier: ALIAS,
            requested: 1,
        },
        occupied: 0,
        invoked: Some(ALIAS),
        expected: PROVISIONED_CONCURRENCY,
    },
    Row {
        case: "the one environment of the allocation is busy, so the invocation spills over",
        allocation: Allocation::Ready {
            qualifier: VERSION,
            requested: 1,
        },
        occupied: 1,
        invoked: Some(VERSION),
        expected: ON_DEMAND,
    },
    Row {
        case: "both environments of the alias's allocation are busy, so it spills over",
        allocation: Allocation::Ready {
            qualifier: ALIAS,
            requested: 2,
        },
        occupied: 2,
        invoked: Some(ALIAS),
        expected: ON_DEMAND,
    },
    Row {
        case: "the version a provisioned alias resolves to, which holds no allocation",
        allocation: Allocation::Ready {
            qualifier: ALIAS,
            requested: 2,
        },
        occupied: 0,
        invoked: Some(VERSION),
        expected: ON_DEMAND,
    },
    Row {
        case: "the unpublished version, which no allocation can be added to",
        allocation: Allocation::Ready {
            qualifier: VERSION,
            requested: 1,
        },
        occupied: 0,
        invoked: None,
        expected: ON_DEMAND,
    },
    Row {
        case: "an allocation Lambda is still making",
        allocation: Allocation::StillBeingMade {
            qualifier: VERSION,
            requested: 1,
        },
        occupied: 0,
        invoked: Some(VERSION),
        expected: ON_DEMAND,
    },
    Row {
        case: "no provisioned concurrency at all",
        allocation: Allocation::None,
        occupied: 0,
        invoked: Some(VERSION),
        expected: ON_DEMAND,
    },
];

/// RULE: a response-streaming invocation that fits inside the READY provisioned-concurrency
/// allocation of the qualifier it addressed is served by one of the environments that allocation
/// keeps initialised and reads `provisioned-concurrency`; every other one is served on demand.
#[tokio::test]
async fn a_streaming_invocation_reports_the_kind_of_environment_serving_it() {
    let _stream = one_invocation_stream_at_a_time().await;
    for (index, row) in ROWS.iter().enumerate() {
        // One account per row, so the only allocation and the only invocations in it are this
        // row's.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let case = row.case;
        let function = &format!("stream-provisioned-{index}");
        create_active_function(&router, case, function).await;
        match row.allocation {
            Allocation::Ready {
                qualifier,
                requested,
            } => {
                allocate_until_ready(&router, case, function, qualifier, requested).await;
                occupy_provisioned_environments(&engine, function, qualifier, row.occupied);
            }
            Allocation::StillBeingMade {
                qualifier,
                requested,
            } => {
                allocate_still_in_progress(&router, &engine, case, function, qualifier, requested)
                    .await;
                occupy_provisioned_environments(&engine, function, qualifier, row.occupied);
            }
            Allocation::None => {}
        }

        let answered = invoke_with_response_stream(&router, function, row.invoked).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: a streaming invocation is answered 200, got {} {}",
            answered.status,
            answered.text()
        );
        let reported = reported_initialization_type(&answered, case);
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected the handler to read {:?}, got {reported:?}",
            row.expected
        );

        // Reading the allocation back after the invocation reports the same settled state: the
        // invocation it served consumed none of it. Only the rows that left no environment
        // occupied take this leg, so what it reads back is the state the API itself produced.
        if let (
            Allocation::Ready {
                qualifier,
                requested,
            },
            0,
        ) = (row.allocation, row.occupied)
        {
            let read = call(
                &router,
                "GET",
                &concurrency_uri(function, qualifier),
                &Value::Null,
            )
            .await;
            assert_eq!(
                (
                    read.string("Status"),
                    read.number("AllocatedProvisionedConcurrentExecutions"),
                    read.number("AvailableProvisionedConcurrentExecutions"),
                ),
                (
                    Some(STATUS_READY.to_string()),
                    Some(requested),
                    Some(requested)
                ),
                "case {case:?}: the allocation still reports all {requested} of it ready after \
                 the invocation, got {}",
                read.text()
            );
        }
    }
}
