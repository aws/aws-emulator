// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an event source mapping: `CreateEventSourceMapping` "creates a
//! mapping between an event source and an Lambda Event Function",
//! `GetEventSourceMapping` "returns details about an event source mapping" and
//! `ListEventSourceMappings` "lists event source mappings. Specify an EventSourceArn to
//! show only event source mappings for a single event source."
//!
//! The rule under test: a created mapping is ONE record this emulator holds under a UUID
//! and an `event-source-mapping` ARN minted once — carrying the event source, the
//! function it resolves and every polling, filtering, destination and logging setting the
//! request stated, with each absent setting at the default its own member documents for
//! THAT event source — and `ListEventSourceMappings` reports those stored records, in one
//! stable order, narrowed to the event source or the function the request names, at most
//! 100 per page, continuing after the marker of the previous page so every mapping is
//! reported exactly once; `GetEventSourceMapping` reports the same record.
//!
//! Every row drives real HTTP requests through the router and reads the mapping back
//! through `GetEventSourceMapping` and `ListEventSourceMappings`, so a mapping that was
//! answered but never stored fails here. The rows differ along what the rule generalises
//! over: WHICH event source the mapping polls (a queue, each of the two shard sources,
//! an MSK cluster, a self-managed Kafka cluster, an Amazon MQ broker and a DocumentDB
//! change stream — the sources the `EventSourceArn` member enumerates), how the function
//! is addressed (bare name, ARN, partial ARN, qualified by a version), which optional
//! settings the request states (none, and each at a published bound) and whether the
//! mapping is enabled.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    claim_poll_passes, poll_passes_once, PollSession,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateEventSourceMapping` and `ListEventSourceMappings` are bound, from
/// `smithy_query(query="operation", operation="CreateEventSourceMapping")`
/// (`POST /2015-03-31/event-source-mappings`).
pub(crate) const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// Where `ListTags` is bound (`GET /2017-03-31/tags/{Resource}`), which is how a
/// mapping's tags are read back.
const TAGS_URI: &str = "/2017-03-31/tags";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `State` a freshly created mapping carries, and the two it settles into, from
/// `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
pub(crate) const STATE_CREATING: &str = "Creating";
/// See [`STATE_CREATING`].
pub(crate) const STATE_ENABLED: &str = "Enabled";
/// See [`STATE_CREATING`].
pub(crate) const STATE_DISABLED: &str = "Disabled";

/// The `State` a create answers for a request carrying `extras`: `Creating` while the
/// poller of a mapping asked to be active is stood up, and `Disabled` for one asked for
/// with polling paused, which has no poller to stand up and so is created in the terminal
/// state its setting asks for ("When false, Lambda pauses polling and invocation").
///
/// Stated here from the REQUEST, independently of the store's own resolution of the same
/// rule, so that these assertions check the emulator rather than restate it: keep the two
/// apart rather than collapsing this into the production helper.
pub(crate) fn created_state_for(extras: &Value) -> &'static str {
    if extras.get("Enabled").and_then(Value::as_bool) == Some(false) {
        STATE_DISABLED
    } else {
        STATE_CREATING
    }
}

/// `StateTransitionReason` for a change a caller asked for.
pub(crate) const USER_INITIATED: &str = "USER_INITIATED";

/// The bounds the model puts on `EventSourceMappingArn` (`@length(min: 85, max: 120)`),
/// from `smithy_query(query="shape", shape="EventSourceMappingArn")`.
const MAPPING_ARN_MIN_LEN: usize = 85;
/// See [`MAPPING_ARN_MIN_LEN`].
const MAPPING_ARN_MAX_LEN: usize = 120;

/// The length the model puts on `UUIDString` (`@length(min: 36, max: 36)`).
pub(crate) const UUID_LEN: usize = 36;

/// The published bounds of `MaximumBatchingWindowInSeconds` (`@range(min: 0, max: 300)`),
/// `ParallelizationFactor` (`@range(min: 1, max: 10)`), `BatchSize`
/// (`@range(min: 1, max: 10000)`), `MaximumRecordAgeInSeconds`
/// (`@range(min: -1, max: 604800)`), `MaximumRetryAttempts` (`@range(min: -1, max:
/// 10000)`) and `TumblingWindowInSeconds` (`@range(min: 0, max: 900)`), from
/// `smithy_query(query="operation", operation="CreateEventSourceMapping")`.
const BATCHING_WINDOW_MAX: i64 = 300;
/// See [`BATCHING_WINDOW_MAX`].
const PARALLELIZATION_MAX: i64 = 10;
/// See [`BATCHING_WINDOW_MAX`].
const BATCH_SIZE_MAX: i64 = 10_000;
/// See [`BATCHING_WINDOW_MAX`].
const RECORD_AGE_MAX: i64 = 604_800;
/// See [`BATCHING_WINDOW_MAX`].
const TUMBLING_WINDOW_MAX: i64 = 900;

/// The most mappings one listing page reports: "Note that ListEventSourceMappings returns
/// a maximum of 100 items in each response, even if you set the number higher."
const PAGE_CAP: usize = 100;

/// The largest `MaxItems` the model admits (`@range(min: 1, max: 10000)`), which the cap
/// above still holds under.
const MAX_ITEMS_MAX: usize = 10_000;

/// An Amazon SQS queue, "Amazon Simple Queue Service – The ARN of the queue".
pub(crate) const QUEUE_SOURCE: &str = "arn:aws:sqs:us-east-1:000000000000:order-intake";
/// An Amazon Kinesis data stream, "Amazon Kinesis – The ARN of the data stream".
pub(crate) const KINESIS_SOURCE: &str = "arn:aws:kinesis:us-east-1:000000000000:stream/order-events";
/// An Amazon DynamoDB stream, "Amazon DynamoDB Streams – The ARN of the stream".
pub(crate) const DYNAMODB_SOURCE: &str =
    "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/2026-10-23T11:46:49.000";
/// An Amazon MSK cluster, "Amazon Managed Streaming for Apache Kafka – The ARN of the
/// cluster".
pub(crate) const MSK_SOURCE: &str =
    "arn:aws:kafka:us-east-1:000000000000:cluster/order-events/11111111-2222-3333-4444-555555555555-1";
/// An Amazon MQ broker, "Amazon MQ – The ARN of the broker".
const MQ_SOURCE: &str = "arn:aws:mq:us-east-1:000000000000:broker:order-broker:b-1111";
/// An Amazon DocumentDB change stream, "Amazon DocumentDB – The ARN of the DocumentDB
/// change stream", whose ARNs are in the `rds` namespace.
const DOCDB_SOURCE: &str = "arn:aws:rds:us-east-1:000000000000:cluster:order-docdb";

/// A well-formed KMS key ARN, satisfying the model's `KMSKeyArn` pattern
/// (`^(arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*)|()$`).
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/0b1c2d3e-4f56-7890-abcd-ef1234567890";

/// A well-formed `OnFailure` destination, satisfying the model's `DestinationArn`
/// pattern.
const FAILURE_DESTINATION: &str = "arn:aws:sqs:us-east-1:000000000000:order-dlq";

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    pub(crate) fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    pub(crate) fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `EventSourceMappings` the body lists, as JSON objects.
    pub(crate) fn mappings(&self) -> Vec<Value> {
        self.json()
            .get("EventSourceMappings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The UUIDs the listed mappings carry, in the order they were reported.
    pub(crate) fn listed_uuids(&self) -> Vec<String> {
        self.mappings()
            .iter()
            .map(|mapping| {
                mapping
                    .get("UUID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// One response header, for a value the wire carries in the headers rather than in the
    /// body — `X-Amz-Function-Error` is one, which is where `Invoke` reports that the
    /// handler itself faulted.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    pub(crate) fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
pub(crate) fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request, signed with the credential this harness publishes — a key no
/// store holds, which is the account's own published credential.
pub(crate) async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    call_signed_by(router, None, method, uri, body, &[]).await
}

/// Dispatch one JSON request signed with the access key id `access_key_id` names, so the
/// request speaks for whichever principal holds that key — `None` is this harness's own
/// credential ([`call`]) — carrying the headers `headers` names beside the standing ones.
///
/// Which principal a request speaks for is what an authorization decision is made about, so
/// a suite that drives one signs each row for itself: this is the one place the credential a
/// test request presents is chosen.
pub(crate) async fn call_signed_by(
    router: &MultiServiceRouter,
    access_key_id: Option<&str>,
    method: &str,
    uri: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let authorization = match access_key_id {
        None => AUTHORIZATION.to_string(),
        Some(key) => format!(
            "AWS4-HMAC-SHA256 Credential={key}/20260101/{REGION}/lambda/aws4_request, \
             SignedHeaders=host, Signature=not-verified"
        ),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", authorization)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `CreateEventSourceMapping`.
pub(crate) async fn create_mapping(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", MAPPINGS_URI, body).await
}

/// `GetEventSourceMapping` on the mapping `uuid` names.
pub(crate) async fn get_mapping(router: &MultiServiceRouter, uuid: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await
}

/// `ListEventSourceMappings` with the query parameters `query` names (already joined).
pub(crate) async fn list_mappings(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        MAPPINGS_URI.to_string()
    } else {
        format!("{MAPPINGS_URI}?{query}")
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// `ListTags` on the resource `arn` names.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{TAGS_URI}/{arn}"), &Value::Null).await
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN an event source mapping of `uuid` carries, as the model's
/// `EventSourceMappingArn` pattern spells it.
fn mapping_arn(uuid: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:event-source-mapping:{uuid}")
}

/// Create a function for a mapping to invoke, publishing version 1 when asked.
async fn create_function_publishing(router: &MultiServiceRouter, name: &str, publish: bool) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Create a function for a mapping to invoke.
pub(crate) async fn create_function(router: &MultiServiceRouter, name: &str) {
    create_function_publishing(router, name, false).await;
}

/// Create a function for a mapping to invoke whose `Timeout` is `timeout` seconds — the
/// member a queue's own `VisibilityTimeout` is compared against when a mapping between the
/// two is made.
pub(crate) async fn create_timed_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    timeout: i64,
) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Timeout": timeout,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: expected CreateFunction {name:?} to answer 201, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The runtime a function whose package RUNS is created with, and the handler that package
/// exposes — the pair [`create_running_function`] uploads.
pub(crate) const RUNNING_RUNTIME: &str = "python3.12";
/// See [`RUNNING_RUNTIME`].
pub(crate) const RUNNING_HANDLER: &str = "lambda_function.lambda_handler";

/// How long a suite waits for work the CLOCK owes — a function going `Active`, a mapping
/// settling into the state its setting asks for — before it is a failure.
pub(crate) const CLOCK_BUDGET: Duration = Duration::from_secs(30);

/// How long between two looks while waiting for one of those.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Create a function whose deployment package RUNS `source` as its python handler, with
/// `timeout` seconds to run in and the environment `variables` states, and wait until it
/// reports `Active`.
///
/// The ONE creator of a running function in the suites over an event source: a handler's own
/// run is what makes a delivery observable, so every such suite goes through this rather than
/// carrying its own copy of the create and the wait.
pub(crate) async fn create_running_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    source: &str,
    timeout: i64,
    variables: &Value,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNNING_RUNTIME,
            "Handler": RUNNING_HANDLER,
            "Timeout": timeout,
            "Environment": { "Variables": variables },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(source)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction {name:?} to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, name).await;
}

/// Wait until the function `name` reports `State` `Active` with no update still in progress,
/// which is when a mapping on it polls and when its code is the code it was last given.
pub(crate) async fn await_function_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let read = call(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        let settled = read.string("State").as_deref() == Some("Active")
            && read.string("LastUpdateStatus").as_deref() != Some("InProgress");
        if settled {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never became Active: {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the mapping `uuid` names reports `state` as its `State`.
pub(crate) async fn await_mapping_state(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
    state: &str,
) {
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let read = get_mapping(router, uuid).await;
        if read.string("State").as_deref() == Some(state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping must settle into {state}, got {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the mapping `uuid` names reports a `LastProcessingResult`, answering what it
/// reported, or `None` when it never did inside [`CLOCK_BUDGET`].
///
/// A batch leaving its source is the START of a processing attempt, not the end of one: the
/// mapping reports "the result of the event source mapping's last processing attempt" only
/// once the invocation it handed the batch to has been decided, which is a span no caller
/// controls. So a suite that has seen the batch go reads the result the same way it reads
/// every other piece of work the mapping owes — by looking again until it is there.
///
/// THE ONE waiter for that member in these suites, so no row reads it once at an instant of
/// its own choosing.
pub(crate) async fn processing_result_within_budget(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
) -> Option<String> {
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let reading = get_mapping(router, uuid).await;
        if let Some(result) = reading.string("LastProcessingResult") {
            return Some(result);
        }
        assert_eq!(
            reading.status, 200,
            "case {case:?}: expected GetEventSourceMapping to answer 200, got {} {}",
            reading.status,
            reading.text()
        );
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `SendMessage` of `body` to the queue `url` addresses, carrying `group` as its
/// `MessageGroupId` when there is one — which a FIFO queue requires and a standard queue
/// takes no notice of.
pub(crate) async fn send_message(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    body: &str,
    group: Option<&str>,
) {
    let mut request = json!({ "QueueUrl": url, "MessageBody": body });
    if let Some(group) = group {
        request["MessageGroupId"] = json!(group);
    }
    let (status, sent) = call_sqs(router, "SendMessage", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
}

/// Lambda over the SAME engine as Amazon SQS, so a mapping resolves and polls the queue in
/// the service that owns it — exactly what `core/router` assembles. The harness every suite
/// that spans the two services uses.
pub(crate) fn router_with_queues() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A suite that makes the poll passes of its queue mappings ITSELF.
///
/// A mapping's poller is background work: the production loop polls, hands the runtime back
/// and polls again, and NOTHING a caller can read says whether a pass has happened yet. A
/// suite that waited a span of its own choosing for one would be betting on how fast the host
/// runs — and every span a pass may take is the emulator's, not the suite's: handing a batch
/// to a function goes through `Invoke`, which waits for this process's run and mount capacity
/// (`crate::state::execution::PERMIT_WAIT`) before the handler is even started, and that
/// capacity is shared with every other test running beside this one.
///
/// So a suite over what a pass DOES takes the store's poll claim instead
/// ([`claim_poll_passes`], which also keeps the production loop from starting for that store)
/// and then makes each pass with [`Self::poll_once`], which answers once the pass is done.
/// What the suite then reads is the state that pass left, at an instant that is no longer a
/// race: no sleep, no budget, and no assertion that depends on the host's speed.
pub(crate) struct DrivenPolling {
    /// The engine whose store's passes are this suite's.
    engine: Arc<engine::InMemoryEngine>,
    /// The way a pass reaches the emulator it is polling inside — the queue's own API and the
    /// function's `Invoke` — which is what the production loop is handed by its operation's
    /// context.
    callback: smithy_server_runtime::EmbeddedEmulator,
    /// The bookkeeping one pass carries to the next, kept across them exactly as the loop
    /// keeps it.
    session: PollSession,
}

impl DrivenPolling {
    /// Take the poll claim of `engine`'s store, so its passes are made by `poll_once` and by
    /// nothing else, with `router` as the emulator each pass reaches.
    ///
    /// Call it BEFORE the first mapping is made: the claim is what keeps the production loop
    /// from starting, and a loop already running would be a second poller beside these
    /// passes.
    pub(crate) fn claiming(
        engine: &Arc<engine::InMemoryEngine>,
        router: &MultiServiceRouter,
    ) -> Self {
        assert!(
            claim_poll_passes(engine),
            "the poll passes of this engine must be this suite's to make, and the claim on \
             them was already taken"
        );
        Self {
            engine: Arc::clone(engine),
            callback: smithy_server_runtime::EmbeddedEmulator::from_router(router.clone()),
            session: PollSession::default(),
        }
    }

    /// The engine the passes are made on, for a suite that reads a store directly.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Make ONE pass over every queue mapping of the store, and answer once it is done: every
    /// batch it took has been handed to its function and what the attempt came to is
    /// recorded.
    pub(crate) async fn poll_once(&mut self) {
        poll_passes_once(
            &self.callback,
            &mut self.session,
            &self.engine,
            SystemTime::now(),
        )
        .await;
    }
}

/// Lambda over the same engine as Amazon SQS, as [`router_with_queues`] assembles them, with
/// the poll passes of its mappings the suite's own ([`DrivenPolling`]).
pub(crate) fn driven_router_with_queues() -> (MultiServiceRouter, DrivenPolling) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let driven = DrivenPolling::claiming(&engine, &router);
    (router, driven)
}

/// What the queue `arn` names hides each of the messages it still HOLDS until, keyed by
/// message body: the instant the queue itself recorded, and `None` for a message it is not
/// hiding at all.
///
/// The hold a delivery takes is a DECISION the queue records — `now` plus the span the
/// delivery hides the message for — not a span of wall clock a reader has to catch the
/// message inside. So a suite over how long a poller holds a batch reads that instant, which
/// says the same thing at every instant it is read, instead of watching the message counts a
/// client reads (the same fact rendered at the instant of the read) and betting on when it
/// looked.
pub(crate) fn held_until(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> std::collections::BTreeMap<String, Option<SystemTime>> {
    let state = engine
        .state::<service_com_amazonaws_sqs_amazonsqs::state::ComAmazonawsSqsAmazonsqsState>();
    let store = state.lock();
    store
        .queue_by_arn(arn)
        .map(|queue| {
            queue
                .messages
                .iter()
                .map(|message| (message.body.clone(), message.invisible_until))
                .collect()
        })
        .unwrap_or_default()
}

/// Call one Amazon SQS operation (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`), the
/// way a client calls it.
pub(crate) async fn call_sqs(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "sqs.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A queue created with the attributes `attributes` names: its url and its ARN, both read
/// back out of Amazon SQS rather than spelled here, so a mapping is made on the ARN the
/// queue itself reports.
pub(crate) async fn create_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    attributes: &Value,
) -> (String, String) {
    let (status, created) = call_sqs(
        router,
        "CreateQueue",
        &json!({ "QueueName": name, "Attributes": attributes }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateQueue {name:?} to answer 200, got {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let arn = queue_attribute(router, case, &url, "QueueArn")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the queue {name:?} reports no QueueArn"));
    (url, arn)
}

/// One attribute of the queue `url` addresses, as `GetQueueAttributes` reports it.
pub(crate) async fn queue_attribute(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    name: &str,
) -> Option<String> {
    let attributes = one_answer_attributes(router, case, url, &[name]).await;
    attributes[name].as_str().map(str::to_string)
}

/// The Amazon SQS attribute counting the messages a queue can hand over right now, from
/// `QueueAttributeName`: "ApproximateNumberOfMessages — the approximate number of
/// visible messages in a queue".
pub(crate) const VISIBLE_MESSAGES: &str = "ApproximateNumberOfMessages";

/// The attribute counting the messages a queue has handed over and nobody has deleted yet:
/// "ApproximateNumberOfMessagesNotVisible — the approximate number of messages that are in
/// flight".
pub(crate) const IN_FLIGHT_MESSAGES: &str = "ApproximateNumberOfMessagesNotVisible";

/// The attribute counting the messages a queue is not handing over yet because their delay
/// has not elapsed: "ApproximateNumberOfMessagesDelayed — the approximate number of
/// messages in the queue that are delayed and not available for reading immediately".
pub(crate) const DELAYED_MESSAGES: &str = "ApproximateNumberOfMessagesDelayed";

/// `SetQueueAttributes` of `VisibilityTimeout` on the queue `url` addresses — how long one
/// delivery of its messages hides them for, which a client may change at any time and which
/// the suites over a mapping's hold on a batch set after the pair was wired.
///
/// THE ONE writer of that attribute in these suites.
pub(crate) async fn set_visibility_timeout(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    seconds: i64,
) {
    let (status, answered) = call_sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "VisibilityTimeout": seconds.to_string() },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SetQueueAttributes to answer 200, got {status} {answered}"
    );
}

/// The attributes `names` lists, from ONE `GetQueueAttributes` answer for the queue `url`
/// addresses: the `Attributes` object exactly as the service reported it. The one call site
/// of that operation in this harness, which every reading above goes through.
///
/// A queue's message counts are consistent WITH EACH OTHER only inside one answer, because
/// each call reports what the queue held at ITS OWN instant — and a message moves between
/// those counts whenever a delivery is made or a visibility timeout lapses. So a reader
/// that needs more than one of them asks for them together.
pub(crate) async fn one_answer_attributes(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    names: &[&str],
) -> Value {
    let (status, answered) = call_sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": names }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetQueueAttributes to answer 200, got {status} {answered}"
    );
    answered["Attributes"].clone()
}

/// The three message counts the queue `url` addresses reports in ONE answer, as
/// `(visible, in flight, delayed)` — see [`one_answer_attributes`] for why they are read
/// together.
pub(crate) async fn queue_message_counts(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> (u32, u32, u32) {
    let attributes = one_answer_attributes(
        router,
        case,
        url,
        &[VISIBLE_MESSAGES, IN_FLIGHT_MESSAGES, DELAYED_MESSAGES],
    )
    .await;
    let count = |name: &str| {
        attributes[name]
            .as_str()
            .and_then(|raw| raw.parse::<u32>().ok())
            .unwrap_or_else(|| {
                panic!("case {case:?}: the queue {url} reports no {name} in {attributes}")
            })
    };
    (
        count(VISIBLE_MESSAGES),
        count(IN_FLIGHT_MESSAGES),
        count(DELAYED_MESSAGES),
    )
}

/// How many messages a `(visible, in flight, delayed)` reading accounts for: every message
/// a queue holds is counted in exactly one of the three, whether it is waiting, in flight
/// with a consumer or still delayed.
pub(crate) fn messages_held((visible, in_flight, delayed): (u32, u32, u32)) -> u32 {
    visible.saturating_add(in_flight).saturating_add(delayed)
}

/// How many messages the queue `url` addresses HOLDS, from one answer.
pub(crate) async fn queue_held(router: &MultiServiceRouter, case: &str, url: &str) -> u32 {
    messages_held(queue_message_counts(router, case, url).await)
}

/// The `CreateEventSourceMapping` body for `function` and `event_source`, plus whatever
/// `extras` states.
pub(crate) fn create_request(function: &str, event_source: Option<&str>, extras: &Value) -> Value {
    let mut body = Map::new();
    body.insert("FunctionName".to_string(), json!(function));
    if let Some(event_source) = event_source {
        body.insert("EventSourceArn".to_string(), json!(event_source));
    }
    if let Some(extras) = extras.as_object() {
        for (member, value) in extras {
            body.insert(member.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// The whole refusal a `CreateEventSourceMapping` whose `EventSourceArn` belongs to a
/// service no event source mapping polls answers: the fixed sentence, then the rejected
/// ARN as the request sent it.
///
/// One owner for this message class, so the tables that drive it cannot drift apart.
pub(crate) fn unsupported_source_refusal(event_source: &str) -> String {
    format!("Unrecognized event source. Unsupported source arn : {event_source}")
}

/// Assert `answered` carries every member `expected` states, where a `Null` expectation
/// means the member must be ABSENT (the default its own documentation gives the member
/// for this event source is "no value at all").
fn assert_members(case: &str, label: &str, answered: &Value, expected: &Value) {
    for (member, want) in expected.as_object().expect("expectations are an object") {
        let got = answered.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            &got, want,
            "case {case:?}: {label} must report {member} as {want}, got {got} in {answered}"
        );
    }
}

/// Whether `uuid` is a hyphenated, 36-character UUID, as `UUIDString` and the
/// `EventSourceMappingArn` pattern require.
fn is_uuid(uuid: &str) -> bool {
    let groups: Vec<&str> = uuid.split('-').collect();
    uuid.len() == UUID_LEN
        && groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// One mapping row: which event source it polls, what else the request states, and what
/// the stored mapping must report.
struct MappingCase {
    name: &'static str,
    /// The event source ARN, or `None` for a self-managed Apache Kafka cluster, which is
    /// named by `SelfManagedEventSource` instead.
    event_source: Option<&'static str>,
    /// How the request addresses the function; `None` means the bare name.
    function_as: Option<String>,
    request_extras: Value,
    expected: Value,
}

/// The rows: one per event source the `EventSourceArn` member enumerates, plus rows that
/// state the optional settings at their published bounds, address the function by each
/// documented form, and disable the mapping.
fn mapping_cases(function: &str) -> Vec<MappingCase> {
    vec![
        MappingCase {
            name: "sqs-queue-defaults",
            event_source: Some(QUEUE_SOURCE),
            function_as: None,
            request_extras: json!({}),
            // "For Amazon SQS, the default is 10", "For Kinesis, DynamoDB, and Amazon SQS
            // event sources, the default batching window is 0 seconds", and the
            // stream-only error handling options apply to no queue.
            expected: json!({
                "BatchSize": 10,
                "MaximumBatchingWindowInSeconds": 0,
                "ParallelizationFactor": null,
                "MaximumRecordAgeInSeconds": null,
                "MaximumRetryAttempts": null,
                "BisectBatchOnFunctionError": null,
                "EventSourceArn": QUEUE_SOURCE,
            }),
        },
        MappingCase {
            name: "kinesis-stream-defaults",
            event_source: Some(KINESIS_SOURCE),
            function_as: None,
            // "Required for Amazon Kinesis and Amazon DynamoDB Stream event sources."
            request_extras: json!({ "StartingPosition": "LATEST" }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": 0,
                "ParallelizationFactor": 1,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "StartingPosition": "LATEST",
            }),
        },
        MappingCase {
            name: "dynamodb-stream-defaults",
            event_source: Some(DYNAMODB_SOURCE),
            function_as: None,
            request_extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": 0,
                "ParallelizationFactor": 1,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "StartingPosition": "TRIM_HORIZON",
            }),
        },
        MappingCase {
            name: "msk-cluster-defaults",
            event_source: Some(MSK_SOURCE),
            function_as: None,
            request_extras: json!({
                "Topics": ["order-events"],
                "AmazonManagedKafkaEventSourceConfig": { "ConsumerGroupId": "order-consumers" },
                "LoggingConfig": { "SystemLogLevel": "INFO" },
                "MetricsConfig": { "Metrics": ["EventCount", "ErrorCount"] },
            }),
            // "For Amazon MSK, Self-managed Apache Kafka, Amazon MQ, and DocumentDB event
            // sources, the default batching window is 500 ms", which this member's unit
            // cannot carry, so none is reported — and MSK IS one of the stream sources the
            // error handling options are available for.
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "ParallelizationFactor": null,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "Topics": ["order-events"],
                "LoggingConfig": { "SystemLogLevel": "INFO" },
                "MetricsConfig": { "Metrics": ["EventCount", "ErrorCount"] },
            }),
        },
        MappingCase {
            name: "self-managed-kafka-defaults",
            event_source: None,
            function_as: None,
            request_extras: json!({
                "SelfManagedEventSource": {
                    "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                },
                "SelfManagedKafkaEventSourceConfig": { "ConsumerGroupId": "self-managed-group" },
                "Topics": ["order-events"],
            }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "EventSourceArn": null,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "SelfManagedEventSource": {
                    "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                },
            }),
        },
        MappingCase {
            name: "mq-broker-defaults",
            event_source: Some(MQ_SOURCE),
            function_as: None,
            request_extras: json!({
                "Queues": ["order-jobs"],
                "SourceAccessConfigurations": [
                    { "Type": "BASIC_AUTH", "URI": "arn:aws:secretsmanager:us-east-1:000000000000:secret:mq-1" },
                ],
            }),
            // Amazon MQ is neither a shard source nor one of the stream sources the error
            // handling options are documented for.
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "ParallelizationFactor": null,
                "MaximumRecordAgeInSeconds": null,
                "MaximumRetryAttempts": null,
                "BisectBatchOnFunctionError": null,
                "Queues": ["order-jobs"],
            }),
        },
        MappingCase {
            name: "documentdb-change-stream-defaults",
            event_source: Some(DOCDB_SOURCE),
            function_as: None,
            request_extras: json!({
                "DocumentDBEventSourceConfig": {
                    "DatabaseName": "orders",
                    "CollectionName": "intake",
                    "FullDocument": "UpdateLookup",
                },
            }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "MaximumRecordAgeInSeconds": null,
                "DocumentDBEventSourceConfig": {
                    "DatabaseName": "orders",
                    "CollectionName": "intake",
                    "FullDocument": "UpdateLookup",
                },
            }),
        },
        MappingCase {
            name: "every-setting-at-its-upper-bound",
            event_source: Some("arn:aws:kinesis:us-east-1:000000000000:stream/bounded"),
            // "Function ARN – arn:aws:lambda:us-west-2:123456789012:function:MyFunction."
            function_as: Some(function_arn(function)),
            request_extras: json!({
                "StartingPosition": "TRIM_HORIZON",
                "BatchSize": BATCH_SIZE_MAX,
                "MaximumBatchingWindowInSeconds": BATCHING_WINDOW_MAX,
                "ParallelizationFactor": PARALLELIZATION_MAX,
                "MaximumRecordAgeInSeconds": RECORD_AGE_MAX,
                "MaximumRetryAttempts": 10_000,
                "TumblingWindowInSeconds": TUMBLING_WINDOW_MAX,
                "BisectBatchOnFunctionError": true,
                "FunctionResponseTypes": ["ReportBatchItemFailures"],
                "DestinationConfig": { "OnFailure": { "Destination": FAILURE_DESTINATION } },
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
            }),
            expected: json!({
                "BatchSize": BATCH_SIZE_MAX,
                "MaximumBatchingWindowInSeconds": BATCHING_WINDOW_MAX,
                "ParallelizationFactor": PARALLELIZATION_MAX,
                "MaximumRecordAgeInSeconds": RECORD_AGE_MAX,
                "MaximumRetryAttempts": 10_000,
                "TumblingWindowInSeconds": TUMBLING_WINDOW_MAX,
                "BisectBatchOnFunctionError": true,
                "FunctionResponseTypes": ["ReportBatchItemFailures"],
                "DestinationConfig": { "OnFailure": { "Destination": FAILURE_DESTINATION } },
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
            }),
        },
        MappingCase {
            name: "every-setting-at-its-lower-bound-disabled",
            event_source: Some("arn:aws:sqs:us-east-1:000000000000:bounded-queue"),
            // "Partial ARN – 123456789012:function:MyFunction."
            function_as: Some(format!("{ACCOUNT}:function:{function}")),
            request_extras: json!({
                "Enabled": false,
                "BatchSize": 1,
                "MaximumBatchingWindowInSeconds": 0,
                "ScalingConfig": { "MaximumConcurrency": 2 },
                "ProvisionedPollerConfig": { "MinimumPollers": 1, "MaximumPollers": 1 },
                "Tags": { "team": "orders" },
            }),
            expected: json!({
                "BatchSize": 1,
                "MaximumBatchingWindowInSeconds": 0,
                "ScalingConfig": { "MaximumConcurrency": 2 },
                "ProvisionedPollerConfig": { "MinimumPollers": 1, "MaximumPollers": 1 },
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_event_source_mapping_is_persisted_and_reported() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-reported-fn";
    create_function(&router, function).await;

    let mut uuids: BTreeSet<String> = BTreeSet::new();

    for case in mapping_cases(function) {
        let label = case.name;
        let request = create_request(
            case.function_as.as_deref().unwrap_or(function),
            case.event_source,
            &case.request_extras,
        );
        let created = create_mapping(&router, &request).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: CreateEventSourceMapping must answer 202 Accepted, got {} {}",
            created.status,
            created.text()
        );

        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {label:?}: expected a UUID, got {}", created.text()));
        assert!(
            is_uuid(&uuid),
            "case {label:?}: UUID must be a 36-character hyphenated UUID, got {uuid:?}"
        );
        assert!(
            uuids.insert(uuid.clone()),
            "case {label:?}: UUID {uuid:?} was already minted for another mapping"
        );

        let arn = created
            .string("EventSourceMappingArn")
            .unwrap_or_else(|| panic!("case {label:?}: expected an ARN, got {}", created.text()));
        assert_eq!(
            arn,
            mapping_arn(&uuid),
            "case {label:?}: the ARN must name this account's region and the mapping's UUID, \
             got {arn:?}"
        );
        assert!(
            arn.len() >= MAPPING_ARN_MIN_LEN && arn.len() <= MAPPING_ARN_MAX_LEN,
            "case {label:?}: the ARN must satisfy the modelled {MAPPING_ARN_MIN_LEN}..\
             {MAPPING_ARN_MAX_LEN} length, got {} in {arn:?}",
            arn.len()
        );
        // A mapping asked to be active is `Creating` while its poller is stood up; one
        // asked for with polling paused has no poller to stand up, so it is created
        // already in the terminal state its setting asks for.
        let created_state = created_state_for(&case.request_extras);
        assert_eq!(
            (
                created.string("FunctionArn"),
                created.string("State"),
                created.string("StateTransitionReason")
            ),
            (
                Some(function_arn(function)),
                Some(created_state.to_string()),
                Some(USER_INITIATED.to_string())
            ),
            "case {label:?}: a created mapping names the function it resolved and starts \
             {created_state}, got {}",
            created.text()
        );
        assert!(
            created.json().get("LastModified").is_some(),
            "case {label:?}: a created mapping reports when it was last modified, got {}",
            created.text()
        );
        assert!(
            created.json().get("LastProcessingResult").is_none(),
            "case {label:?}: no poller has processed anything, so no last processing result \
             is reported, got {}",
            created.text()
        );
        assert_members(label, "the create", &created.json(), &case.expected);

        // The round trip: the mapping is read back through GetEventSourceMapping, which
        // answers out of the store, so a mapping that was answered but never persisted
        // fails here.
        let fetched = get_mapping(&router, &uuid).await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: GetEventSourceMapping must report the stored mapping, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_members(label, "the get", &fetched.json(), &case.expected);
        assert_eq!(
            (
                fetched.string("UUID"),
                fetched.string("EventSourceMappingArn"),
                fetched.string("FunctionArn"),
                fetched.string("StateTransitionReason")
            ),
            (
                Some(uuid.clone()),
                Some(arn.clone()),
                Some(function_arn(function)),
                Some(USER_INITIATED.to_string())
            ),
            "case {label:?}: the get reports the same identity the create minted, got {}",
            fetched.text()
        );

        // And the listing reports the same record, narrowed to this mapping's own event
        // source where it has one.
        if let Some(event_source) = case.event_source {
            let listed = list_mappings(&router, &format!("EventSourceArn={event_source}")).await;
            assert_eq!(
                (listed.status, listed.listed_uuids()),
                (200, vec![uuid.clone()]),
                "case {label:?}: a listing of {event_source:?} reports exactly this mapping, \
                 got {} {}",
                listed.status,
                listed.text()
            );
            assert_members(
                label,
                "the listing",
                listed
                    .mappings()
                    .first()
                    .unwrap_or_else(|| panic!("case {label:?}: the listing reported no mapping")),
                &case.expected,
            );
        }
    }

    // Every mapping created above is reported by an unfiltered listing, exactly once.
    let listed = list_mappings(&router, "").await;
    assert_eq!(
        (listed.status, listed.listed_uuids().len()),
        (200, uuids.len()),
        "an unfiltered listing reports every stored mapping, got {} {}",
        listed.status,
        listed.text()
    );
    assert_eq!(
        listed.listed_uuids().into_iter().collect::<BTreeSet<_>>(),
        uuids,
        "an unfiltered listing reports exactly the mappings that were created"
    );
}

#[tokio::test]
async fn a_mapping_settles_into_the_state_its_enabled_setting_asks_for() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-settling-fn";
    create_function(&router, function).await;

    // "When true, the event source mapping is active. When false, Lambda pauses polling
    // and invocation. Default: True"
    let cases = [
        ("enabled-by-default", json!({}), STATE_ENABLED),
        ("enabled-explicitly", json!({ "Enabled": true }), STATE_ENABLED),
        ("disabled", json!({ "Enabled": false }), STATE_DISABLED),
    ];

    let mut uuids = Vec::new();
    for (index, (label, extras, expected)) in cases.iter().enumerate() {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:settling-{index}");
        let created = create_mapping(
            &router,
            &create_request(function, Some(&source), extras),
        )
        .await;
        // Only a mapping with a poller to stand up starts transitional; a paused one is
        // created in the terminal state it asked for.
        let created_state = created_state_for(extras);
        assert_eq!(
            (created.status, created.string("State")),
            (202, Some(created_state.to_string())),
            "case {label:?}: a created mapping starts {created_state}, got {} {}",
            created.status,
            created.text()
        );
        uuids.push((
            label,
            created.string("UUID").expect("the create answers a UUID"),
            *expected,
            created_state,
            created
                .json()
                .get("LastModified")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: the create reports LastModified as a timestamp")
                }),
        ));
    }

    // The transition is owed by the clock, not by anybody calling: nothing here sleeps on
    // the emulator's behalf, it just waits past the instant the create made due.
    let delay = service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY;
    tokio::time::sleep(delay + std::time::Duration::from_millis(100)).await;

    for (label, uuid, expected, created_state, created_at) in uuids {
        let mut settled_at = None;
        for read in 1..=2 {
            let fetched = get_mapping(&router, &uuid).await;
            assert_eq!(
                (fetched.status, fetched.string("State")),
                (200, Some(expected.to_string())),
                "case {label:?} read {read}: expected {expected} once the transition was due, \
                 got {} {}",
                fetched.status,
                fetched.text()
            );
            // "The date that the event source mapping was last updated or that its state
            // changed": the transition IS a state change, so the instant moves with it —
            // to when the clock owed it, which is why two reads report the same instant.
            let reported = fetched.json().get("LastModified").and_then(Value::as_f64);
            if created_state == expected {
                // Nothing transitioned: the mapping was already in this state when it was
                // created, so the instant it reports is still the creation's.
                assert_eq!(
                    reported,
                    Some(created_at),
                    "case {label:?} read {read}: a mapping created {expected} owes no \
                     transition, so LastModified stays at {created_at}, got {reported:?}"
                );
            } else {
                assert!(
                    reported.is_some_and(|modified| modified > created_at),
                    "case {label:?} read {read}: LastModified must move with the state change \
                     from {created_at}, got {reported:?}"
                );
            }
            if let Some(before) = settled_at {
                assert_eq!(
                    reported,
                    Some(before),
                    "case {label:?} read {read}: reading the mapping again must not move \
                     LastModified, got {reported:?} after {before}"
                );
            }
            settled_at = reported;
        }
        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let states: Vec<Value> = listed
            .mappings()
            .iter()
            .filter(|mapping| mapping.get("UUID").and_then(Value::as_str) == Some(uuid.as_str()))
            .map(|mapping| mapping.get("State").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            states,
            vec![json!(expected)],
            "case {label:?}: the listing reports the same settled state, got {}",
            listed.text()
        );
    }
}

#[tokio::test]
async fn a_listing_narrows_to_the_event_source_or_the_function_it_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_publishing(&router, "esm-filter-a", true).await;
    create_function(&router, "esm-filter-b").await;

    // Three mappings: two of function A (one of them against its published version 1) and
    // one of function B, two of which share an event source.
    let shared_source = "arn:aws:sqs:us-east-1:000000000000:shared-queue";
    let own_source = "arn:aws:sqs:us-east-1:000000000000:own-queue";

    let latest_of_a = create_mapping(
        &router,
        &create_request("esm-filter-a", Some(shared_source), &json!({})),
    )
    .await;
    let version_of_a = create_mapping(
        &router,
        &create_request("esm-filter-a:1", Some(own_source), &json!({})),
    )
    .await;
    let of_b = create_mapping(
        &router,
        &create_request("esm-filter-b", Some(shared_source), &json!({})),
    )
    .await;
    for (label, answer) in [
        ("latest-of-a", &latest_of_a),
        ("version-of-a", &version_of_a),
        ("of-b", &of_b),
    ] {
        assert_eq!(
            answer.status,
            202,
            "case {label:?}: the create must succeed, got {} {}",
            answer.status,
            answer.text()
        );
    }
    let latest_of_a = latest_of_a.string("UUID").expect("a UUID");
    let version_of_a = version_of_a.string("UUID").expect("a UUID");
    let of_b = of_b.string("UUID").expect("a UUID");

    assert_eq!(
        version_of_a_arn(&router, &version_of_a).await,
        Some(format!("{}:1", function_arn("esm-filter-a"))),
        "a mapping created against a qualified function name names that version"
    );

    // Each row: the query, and the mappings it must report.
    let cases: Vec<(&str, String, Vec<&String>)> = vec![
        (
            "by-event-source",
            format!("EventSourceArn={shared_source}"),
            vec![&latest_of_a, &of_b],
        ),
        (
            "by-other-event-source",
            format!("EventSourceArn={own_source}"),
            vec![&version_of_a],
        ),
        (
            "by-function-name",
            "FunctionName=esm-filter-a".to_string(),
            vec![&latest_of_a, &version_of_a],
        ),
        (
            "by-function-arn",
            format!("FunctionName={}", function_arn("esm-filter-a")),
            vec![&latest_of_a, &version_of_a],
        ),
        (
            "by-qualified-function",
            "FunctionName=esm-filter-a:1".to_string(),
            vec![&version_of_a],
        ),
        (
            "by-other-function",
            "FunctionName=esm-filter-b".to_string(),
            vec![&of_b],
        ),
        (
            "by-function-and-event-source",
            format!("FunctionName=esm-filter-a&EventSourceArn={shared_source}"),
            vec![&latest_of_a],
        ),
        (
            "by-event-source-nothing-maps",
            "EventSourceArn=arn:aws:sqs:us-east-1:000000000000:unmapped".to_string(),
            vec![],
        ),
    ];

    for (label, query, expected) in cases {
        let listed = list_mappings(&router, &query).await;
        let want: BTreeSet<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            (listed.status, listed.listed_uuids().into_iter().collect()),
            (200, want.clone()),
            "case {label:?}: {query:?} must report {want:?}, got {} {}",
            listed.status,
            listed.text()
        );
    }
}

/// The `FunctionArn` the mapping `uuid` reports, read back through the get.
async fn version_of_a_arn(router: &MultiServiceRouter, uuid: &str) -> Option<String> {
    get_mapping(router, uuid).await.string("FunctionArn")
}

#[tokio::test]
async fn a_listing_pages_through_every_mapping_exactly_once() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-paging-fn";
    create_function(&router, function).await;

    // One more mapping than a page may hold, so the documented cap is exercised past its
    // own bound.
    let total = PAGE_CAP + 1;
    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..total {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:paging-{index:04}");
        let answer = create_mapping(
            &router,
            &create_request(function, Some(&source), &json!({})),
        )
        .await;
        assert_eq!(
            answer.status,
            202,
            "mapping {index} must be created, got {} {}",
            answer.status,
            answer.text()
        );
        created.insert(answer.string("UUID").expect("the create answers a UUID"));
    }

    // "Note that ListEventSourceMappings returns a maximum of 100 items in each response,
    // even if you set the number higher."
    let capped = list_mappings(&router, &format!("MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        (capped.status, capped.listed_uuids().len()),
        (200, PAGE_CAP),
        "a page holds at most {PAGE_CAP} mappings however high MaxItems is, got {} {}",
        capped.status,
        capped.text()
    );
    assert!(
        capped.string("NextMarker").is_some(),
        "a truncated page carries the marker the next one continues after, got {}",
        capped.text()
    );

    // Every page size walks the whole set exactly once, in one stable order.
    for page_size in [1usize, 7, PAGE_CAP, MAX_ITEMS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= total + 1,
                "page size {page_size}: the walk must terminate, {pages} pages in"
            );
            let query = match marker.as_deref() {
                Some(marker) => format!("MaxItems={page_size}&Marker={marker}"),
                None => format!("MaxItems={page_size}"),
            };
            let page = list_mappings(&router, &query).await;
            assert_eq!(
                page.status,
                200,
                "page size {page_size}: the listing must succeed, got {} {}",
                page.status,
                page.text()
            );
            let uuids = page.listed_uuids();
            assert!(
                uuids.len() <= page_size.min(PAGE_CAP),
                "page size {page_size}: a page reports at most {} mappings, got {}",
                page_size.min(PAGE_CAP),
                uuids.len()
            );
            seen.extend(uuids);
            match page.string("NextMarker") {
                Some(next) => marker = Some(next),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: no mapping may be reported twice, got {} of {}",
            unique.len(),
            seen.len()
        );
        assert_eq!(
            unique, created,
            "page size {page_size}: the walk must report every created mapping"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "page size {page_size}: the order must be the stable one the marker continues"
        );
    }
}

#[tokio::test]
async fn encrypted_filter_criteria_are_reported_by_the_get_and_not_by_the_listing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-filter-criteria-fn";
    create_function(&router, function).await;

    let criteria = json!({ "Filters": [{ "Pattern": "{\"region\":[\"us-east-1\"]}" }] });
    // Two rows: the same criteria, encrypted with a customer managed key and not.
    let cases = [
        ("plaintext", json!({ "FilterCriteria": criteria }), true),
        (
            "encrypted",
            json!({ "FilterCriteria": criteria, "KMSKeyArn": KMS_KEY }),
            false,
        ),
    ];

    for (index, (label, extras, listed_in_plaintext)) in cases.iter().enumerate() {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:criteria-{index}");
        let created = create_mapping(&router, &create_request(function, Some(&source), extras)).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the create must succeed, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created.string("UUID").expect("the create answers a UUID");

        // "You can view this field in plaintext in the response of GetEventSourceMapping
        // and DeleteEventSourceMapping calls if you have kms:Decrypt permissions for the
        // correct KMS key."
        let fetched = get_mapping(&router, &uuid).await;
        assert_eq!(
            fetched.json().get("FilterCriteria"),
            Some(&criteria),
            "case {label:?}: the get reports the stored filter criteria, got {}",
            fetched.text()
        );

        // "If filter criteria is encrypted, this field shows up as null in the response of
        // ListEventSourceMapping API calls."
        let listed = list_mappings(&router, &format!("EventSourceArn={source}")).await;
        let reported = listed
            .mappings()
            .first()
            .and_then(|mapping| mapping.get("FilterCriteria").cloned());
        let expected = listed_in_plaintext.then(|| criteria.clone());
        assert_eq!(
            reported, expected,
            "case {label:?}: the listing must report {expected:?} for FilterCriteria, got {}",
            listed.text()
        );
    }
}

#[tokio::test]
async fn tags_a_mapping_was_created_with_are_reported_for_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-tagged-fn";
    create_function(&router, function).await;

    // Two rows: a mapping created with tags, and one created with none.
    let cases = [
        (
            "tagged",
            json!({ "Tags": { "team": "orders", "env": "test" } }),
            json!({ "env": "test", "team": "orders" }),
        ),
        ("untagged", json!({}), json!({})),
    ];

    for (index, (label, extras, expected)) in cases.iter().enumerate() {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:tagged-{index}");
        let created =
            create_mapping(&router, &create_request(function, Some(&source), extras)).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the create must succeed, got {} {}",
            created.status,
            created.text()
        );
        let arn = created
            .string("EventSourceMappingArn")
            .expect("the create answers an ARN");
        let tagged = list_tags(&router, &arn).await;
        assert_eq!(
            (tagged.status, tagged.json().get("Tags")),
            (200, Some(expected)),
            "case {label:?}: ListTags on {arn:?} must report {expected}, got {} {}",
            tagged.status,
            tagged.text()
        );
    }
}

/// One event-source row: the `EventSourceArn` a create names, what else that source
/// requires the create to state, and whether a mapping of it can be created at all.
struct EventSourceCase {
    name: &'static str,
    event_source: &'static str,
    request_extras: Value,
    /// Whether this account can map the resource the ARN names.
    mappable: bool,
}

/// The rows. The accepted ones are the catalogue the `EventSourceArn` member itself
/// enumerates — `smithy_query(query="doc", shape="CreateEventSourceMappingRequest",
/// member="EventSourceArn")`: "Amazon Kinesis – The ARN of the data stream or a stream
/// consumer. Amazon DynamoDB Streams – The ARN of the stream. Amazon Simple Queue Service
/// – The ARN of the queue. Amazon Managed Streaming for Apache Kafka – The ARN of the
/// cluster or the ARN of the VPC connection (for cross-account event source mappings).
/// Amazon MQ – The ARN of the broker. Amazon DocumentDB – The ARN of the DocumentDB change
/// stream." The refused ones are well-formed ARNs outside it: another vendor's resource,
/// another resource type of one of these vendors, and one of these resources in another
/// account (which only the documented MSK VPC connection form may be). A value that is no
/// ARN at all is refused by the modelled `Arn` pattern before the handler runs, so it is
/// not a row here.
fn event_source_cases() -> Vec<EventSourceCase> {
    vec![
        EventSourceCase {
            name: "a kinesis data stream",
            event_source: "arn:aws:kinesis:us-east-1:000000000000:stream/catalog-events",
            request_extras: json!({ "StartingPosition": "LATEST" }),
            mappable: true,
        },
        EventSourceCase {
            name: "a kinesis stream consumer",
            event_source:
                "arn:aws:kinesis:us-east-1:000000000000:stream/catalog-events/consumer/analytics:1700000000",
            request_extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            mappable: true,
        },
        EventSourceCase {
            name: "a dynamodb stream",
            event_source:
                "arn:aws:dynamodb:us-east-1:000000000000:table/catalog/stream/2026-10-23T11:46:49.000",
            request_extras: json!({ "StartingPosition": "LATEST" }),
            mappable: true,
        },
        EventSourceCase {
            name: "a queue",
            event_source: "arn:aws:sqs:us-east-1:000000000000:catalog-intake",
            request_extras: json!({}),
            mappable: true,
        },
        EventSourceCase {
            name: "an msk cluster",
            event_source:
                "arn:aws:kafka:us-east-1:000000000000:cluster/catalog-events/11111111-2222-3333-4444-555555555555-1",
            request_extras: json!({ "Topics": ["catalog"] }),
            mappable: true,
        },
        EventSourceCase {
            name: "an msk vpc connection of another account, the documented cross-account form",
            event_source:
                "arn:aws:kafka:us-east-1:999999999999:vpc-connection/11111111-2222-3333-4444-555555555555-1/22222222-3333-4444-5555-666666666666",
            request_extras: json!({ "Topics": ["catalog"] }),
            mappable: true,
        },
        EventSourceCase {
            name: "an amazon mq broker",
            event_source: "arn:aws:mq:us-east-1:000000000000:broker:catalog-broker:b-1111",
            request_extras: json!({ "Queues": ["catalog"] }),
            mappable: true,
        },
        EventSourceCase {
            name: "a documentdb change stream cluster",
            event_source: "arn:aws:rds:us-east-1:000000000000:cluster:catalog-docdb",
            // A mapping onto a DocumentDB cluster names the database it consumes inside it.
            request_extras: json!({ "DocumentDBEventSourceConfig": { "DatabaseName": "catalog" } }),
            mappable: true,
        },
        EventSourceCase {
            name: "a bucket, which is no event source",
            event_source: "arn:aws:s3:::catalog-dropbox",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "a topic, which is no event source",
            event_source: "arn:aws:sns:us-east-1:000000000000:catalog-events",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "a function, which is no event source",
            event_source: "arn:aws:lambda:us-east-1:000000000000:function:catalog-fn",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "a dynamodb table rather than its stream",
            event_source: "arn:aws:dynamodb:us-east-1:000000000000:table/catalog",
            request_extras: json!({ "StartingPosition": "LATEST" }),
            mappable: false,
        },
        EventSourceCase {
            name: "a database instance rather than a documentdb cluster",
            event_source: "arn:aws:rds:us-east-1:000000000000:db:catalog-db",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "an msk configuration rather than a cluster",
            event_source:
                "arn:aws:kafka:us-east-1:000000000000:configuration/catalog/11111111-2222-3333-4444-555555555555-1",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "an mq configuration rather than a broker",
            event_source: "arn:aws:mq:us-east-1:000000000000:configuration:catalog-config:c-1111",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "a queue of another account",
            event_source: "arn:aws:sqs:us-east-1:999999999999:catalog-foreign",
            request_extras: json!({}),
            mappable: false,
        },
        EventSourceCase {
            name: "a kinesis data stream of another account",
            event_source: "arn:aws:kinesis:us-east-1:999999999999:stream/catalog-foreign",
            request_extras: json!({ "StartingPosition": "LATEST" }),
            mappable: false,
        },
        EventSourceCase {
            name: "an msk cluster of another account, which is not the cross-account form",
            event_source:
                "arn:aws:kafka:us-east-1:999999999999:cluster/catalog-foreign/11111111-2222-3333-4444-555555555555-1",
            request_extras: json!({ "Topics": ["catalog"] }),
            mappable: false,
        },
        EventSourceCase {
            name: "a queue with no account at all",
            event_source: "arn:aws:sqs:us-east-1::catalog-accountless",
            request_extras: json!({}),
            mappable: false,
        },
    ]
}

/// The event source of a mapping is one of the resources `EventSourceArn` enumerates, in
/// this account: a resource of another vendor, another resource type of one of those
/// vendors, or one of them in another account (which only the documented MSK VPC
/// connection may be) names nothing a poller could read, so it is refused with the
/// modelled 400 and no mapping is stored for it.
#[tokio::test]
async fn only_an_arn_naming_an_event_source_of_this_account_is_mapped() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-source-catalog-fn";
    create_function(&router, function).await;

    let cases = event_source_cases();
    for case in &cases {
        let label = case.name;
        let answer = create_mapping(
            &router,
            &create_request(function, Some(case.event_source), &case.request_extras),
        )
        .await;
        if case.mappable {
            assert_eq!(
                answer.status,
                202,
                "case {label:?}: {} names an event source, so the create must be accepted, \
                 got {} {}",
                case.event_source,
                answer.status,
                answer.text()
            );
            let uuid = answer
                .string("UUID")
                .unwrap_or_else(|| panic!("case {label:?}: the create answers a UUID"));
            // Read back through the Get: the mapping is a stored record of that source.
            let fetched = get_mapping(&router, &uuid).await;
            assert_eq!(
                (fetched.status, fetched.string("EventSourceArn")),
                (200, Some(case.event_source.to_string())),
                "case {label:?}: the stored mapping must report the event source it was \
                 created against, got {} {}",
                fetched.status,
                fetched.text()
            );
        } else {
            assert_eq!(
                (
                    answer.status,
                    answer
                        .error_code()
                        .contains("InvalidParameterValueException")
                ),
                (400, true),
                "case {label:?}: {} names no event source this account can map, so the \
                 create must be refused with the modelled 400, got {} {}",
                case.event_source,
                answer.status,
                answer.text()
            );
        }
    }

    // Exactly the mappable rows are held, so no refusal stored a mapping of a source
    // nothing could poll.
    let listed = list_mappings(&router, "").await;
    let stored: BTreeSet<String> = listed
        .mappings()
        .iter()
        .filter_map(|mapping| {
            mapping
                .get("EventSourceArn")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let expected: BTreeSet<String> = cases
        .iter()
        .filter(|case| case.mappable)
        .map(|case| case.event_source.to_string())
        .collect();
    assert_eq!(
        stored, expected,
        "the store must hold exactly the mappings of the sources that can be mapped, got {}",
        listed.text()
    );
}

/// One refusal row: what to send, and the status (and modelled error code) it must be
/// answered with.
struct RefusalCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_request_no_event_source_mapping_can_satisfy_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-refusal-fn";
    create_function(&router, function).await;

    let taken = "arn:aws:sqs:us-east-1:000000000000:taken-queue";
    let created = create_mapping(&router, &create_request(function, Some(taken), &json!({}))).await;
    assert_eq!(
        created.status,
        202,
        "the mapping the conflict row re-creates must exist, got {} {}",
        created.status,
        created.text()
    );
    let known_uuid = created.string("UUID").expect("the create answers a UUID");

    let cases = vec![
        RefusalCase {
            name: "create-for-an-unknown-function",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request("esm-absent-fn", Some(taken), &json!({})),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "create-for-an-unknown-version",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                &format!("{function}:7"),
                Some("arn:aws:sqs:us-east-1:000000000000:versioned"),
                &json!({}),
            ),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "create-naming-no-event-source",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, None, &json!({})),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-naming-two-event-sources",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some("arn:aws:sqs:us-east-1:000000000000:both"),
                &json!({
                    "SelfManagedEventSource": {
                        "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                    },
                }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-on-a-kinesis-stream-without-a-starting-position",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, Some(KINESIS_SOURCE), &json!({})),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-on-a-dynamodb-stream-without-a-starting-position",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, Some(DYNAMODB_SOURCE), &json!({})),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-at-a-timestamp-on-a-source-that-does-not-support-it",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some(DYNAMODB_SOURCE),
                &json!({ "StartingPosition": "AT_TIMESTAMP", "StartingPositionTimestamp": 1_700_000_000 }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-at-a-timestamp-without-one",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some(KINESIS_SOURCE),
                &json!({ "StartingPosition": "AT_TIMESTAMP" }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-at-a-timestamp-in-the-future",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some(KINESIS_SOURCE),
                &json!({
                    "StartingPosition": "AT_TIMESTAMP",
                    "StartingPositionTimestamp": 4_000_000_000i64,
                }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-a-second-mapping-of-the-same-pair",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, Some(taken), &json!({})),
            status: 409,
            code: "ResourceConflictException",
        },
        RefusalCase {
            name: "get-a-mapping-this-account-does-not-hold",
            method: "GET",
            uri: format!("{MAPPINGS_URI}/11111111-2222-3333-4444-555555555555"),
            body: Value::Null,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "list-the-mappings-of-an-unknown-function",
            method: "GET",
            uri: format!("{MAPPINGS_URI}?FunctionName=esm-absent-fn"),
            body: Value::Null,
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];

    for case in cases {
        let answer = call(&router, case.method, &case.uri, &case.body).await;
        assert_eq!(
            (answer.status, answer.error_code().contains(case.code)),
            (case.status, true),
            "case {:?}: expected {} {}, got {} {}",
            case.name,
            case.status,
            case.code,
            answer.status,
            answer.text()
        );
    }

    // A refusal changes nothing: the one mapping this account holds is still the one it
    // held before.
    let listed = list_mappings(&router, "").await;
    assert_eq!(
        (listed.status, listed.listed_uuids()),
        (200, vec![known_uuid]),
        "every refusal above must leave the store as it was, got {} {}",
        listed.status,
        listed.text()
    );
}
