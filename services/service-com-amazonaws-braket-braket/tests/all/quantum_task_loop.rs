// SPDX-License-Identifier: CC0-1.0

//! The Braket quantum-task loop, end to end over HTTP.
//!
//! Every assertion here goes through the router: a request is dispatched to the
//! registered service exactly as the emulator's front door dispatches it, and the
//! response is read off the wire. Nothing reaches into the state struct, so what these
//! tests pin is the behaviour a Braket client sees.
//!
//! The suites assert RULES over a SET of inputs rather than one recorded call: each
//! table row differs along what its rule generalises over (the filtered member, the
//! page size, the shot count, the device status, the filter operator).
//!
//! One suite at the end is the exception: the store's retention bound is exercised
//! against the state struct directly, because filling the store through the API would
//! mean waiting out the lifecycle of a thousand tasks.
//!
//! The last group of suites pins the WIRE SHAPE of these paths rather than their
//! behaviour: the status code the create answers with and the members a client parses off
//! each response — the accepted task's status, the form each timestamp travels in, the
//! tags map, and the device-parameters document the task ran under.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The region the test's SigV4 credential scope names, so a minted ARN must carry it.
///
/// `pub(crate)` so the suites beside this one sign for the same Region as the harness
/// they borrow.
pub(crate) const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One response, reduced to what a client branches on.
///
/// `pub(crate)` so the suites beside this one (`arn_syntax`, `device_capabilities`)
/// drive the service through the same harness rather than standing up a second one.
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), if any.
    pub(crate) error: Option<String>,
    pub(crate) body: Value,
    /// The response entity EXACTLY as it came off the wire, before any JSON reading: what a
    /// suite asserting the FRAMING of an answer (an answer that carries no content carries no
    /// body) has to look at, which the parsed `body` above cannot show.
    pub(crate) bytes: Bytes,
    /// The `content-length` the answer states, if it states one — the other half of that
    /// framing.
    pub(crate) content_length: Option<String>,
}

impl Reply {
    /// The response body member `name`, as a string.
    pub(crate) fn str_member(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("response has no string member {name}: {}", self.body))
            .to_string()
    }

    /// The error body's `message` member, or the empty string when it carries none — so
    /// a refusal with no message fails the assertion that reads it rather than panicking
    /// inside the harness.
    pub(crate) fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// A Braket emulator behind the production router.
pub(crate) struct Braket {
    router: MultiServiceRouter,
    /// The one shared engine the service was built over. Held so a test can SEED the
    /// store (never to assert through it: every assertion goes over HTTP).
    engine: Arc<engine::InMemoryEngine>,
}

impl Braket {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// This service mounted beside the OWNER of the roles it resolves — IAM, which holds
    /// the execution role a hybrid job asks Braket to assume — on ONE shared engine, plus
    /// an IAM SDK client over the router's own in-process callback: the very seam this
    /// service's own role resolution reaches IAM through.
    ///
    /// Nothing in a suite using this reaches into IAM's store: a role exists only because
    /// IAM answered for it.
    pub(crate) fn with_iam() -> (Self, ::aws_sdk_iam::Client) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        let iam = ::aws_sdk_iam::Client::from_conf(
            ::aws_sdk_iam::Config::builder()
                .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
                .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        (Self { router, engine }, iam)
    }

    /// This service mounted beside the OWNER of the output location a quantum task's
    /// results go to — Amazon S3, which holds the bucket the create names and the result
    /// object a finished task's results are read from — on ONE shared engine, plus an S3
    /// SDK client over the router's own in-process callback: the very seam
    /// `CreateQuantumTask` resolves the output bucket and writes the result document
    /// through.
    ///
    /// Nothing in a suite using this reaches into S3's store: a bucket exists only
    /// because S3 answered for it, and a result object is read back through S3's own API.
    pub(crate) fn with_s3() -> (Self, ::aws_sdk_s3::Client) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        let s3 = ::service_common::s3_client::s3_client(&callback, REGION);
        (Self { router, engine }, s3)
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.call_in_region(REGION, method, path, body).await
    }
    /// As [`Braket::call`], with the request signed for `region` instead of this
    /// harness's own: a response that states the region the request was signed for can
    /// then be asserted over more than one region.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/braket/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|t| t.rsplit('#').next().unwrap_or(t).to_string())
        });
        Reply {
            status,
            error,
            body,
            bytes,
            content_length,
        }
    }

    async fn search_devices(&self, body: Value) -> Reply {
        self.call("POST", "/devices", Some(body)).await
    }

    pub(crate) async fn get_device(&self, device_arn: &str) -> Reply {
        self.call("GET", &format!("/device/{}", encode(device_arn)), None)
            .await
    }

    pub(crate) async fn create_task(&self, body: Value) -> Reply {
        self.call("POST", "/quantum-task", Some(body)).await
    }

    pub(crate) async fn get_task(&self, arn: &str) -> Reply {
        self.call("GET", &format!("/quantum-task/{}", encode(arn)), None)
            .await
    }

    async fn get_task_with_queue_info(&self, arn: &str) -> Reply {
        self.call(
            "GET",
            &format!(
                "/quantum-task/{}?additionalAttributeNames=QueueInfo",
                encode(arn)
            ),
            None,
        )
        .await
    }

    async fn search_tasks(&self, body: Value) -> Reply {
        self.call("POST", "/quantum-tasks", Some(body)).await
    }

    pub(crate) async fn cancel_task(&self, arn: &str, client_token: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/quantum-task/{}/cancel", encode(arn)),
            Some(json!({ "clientToken": client_token })),
        )
        .await
    }

    /// The whole seeded inventory, as `SearchDevices` with no filter returns it.
    pub(crate) async fn inventory(&self) -> Vec<Value> {
        let reply = self
            .search_devices(json!({ "filters": [], "maxResults": 100 }))
            .await;
        assert_eq!(reply.status, 200, "unfiltered SearchDevices: {}", reply.body);
        assert!(
            reply.body.get("nextToken").and_then(Value::as_str).is_none(),
            "the whole inventory fits in one page of 100: {}",
            reply.body
        );
        reply
            .body
            .get("devices")
            .and_then(Value::as_array)
            .cloned()
            .expect("devices is a required output member")
    }

    /// The ARN of one ONLINE device from the inventory.
    pub(crate) async fn an_online_device(&self) -> String {
        let inventory = self.inventory().await;
        let device = inventory
            .iter()
            .find(|d| member(d, "deviceStatus") == "ONLINE")
            .expect("the seeded inventory holds an ONLINE device");
        member(device, "deviceArn").to_string()
    }
}

/// Percent-encode a path segment, as an SDK encodes an `@httpLabel` value: an ARN
/// carries `:` and `/`, so an unencoded one would be read as several segments.
///
/// Written here rather than taken from `percent-encoding` because a dependency would
/// put this crate's `Cargo.lock` entry in the change, and that one line is what the
/// serialized merge into the shared mainline kept refusing to squash. Everything
/// outside the unreserved set of RFC 3986 section 2.3 is escaped, which is what the
/// server's own path decoder reverses.
pub(crate) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The named string member of a JSON object.
pub(crate) fn member<'v>(value: &'v Value, name: &str) -> &'v str {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("object has no string member {name}: {value}"))
}

/// Slack for the time between measuring a task's age in the test and the handler
/// reading the clock for it. A status is pinned to one value when the measured age is
/// clear of a due instant by this much; inside the band both neighbouring statuses are
/// what the clock owes, so both are accepted — which is what keeps the clock
/// assertions sharp without depending on machine speed.
const CLOCK_GUARD: Duration = Duration::from_millis(150);

/// Assert the status a task of measured age `age` reports is one the clock owes at
/// that age: `CREATED` before the queue span, `QUEUED` between the spans, `FAILED`
/// after the terminal span. The age is measured BEFORE the read, so the task's age at
/// the handler's own clock read is `age` or a little more.
fn assert_clock_consistent(status: &str, age: Duration, case: &str) {
    use service_com_amazonaws_braket_braket::state::{QUEUE_SPAN, TERMINAL_SPAN};

    let mut owed: Vec<&str> = Vec::new();
    if age < QUEUE_SPAN {
        owed.push("CREATED");
    }
    if age + CLOCK_GUARD >= QUEUE_SPAN && age < TERMINAL_SPAN {
        owed.push("QUEUED");
    }
    if age + CLOCK_GUARD >= TERMINAL_SPAN {
        owed.push("FAILED");
    }
    assert!(
        owed.contains(&status),
        "case {case} ({age:?} after the create returned): expected the status the clock \
         owes, one of {owed:?}, got {status}"
    );
}

/// A minimal but well-formed Braket action document, as the `action` JSON string.
fn action_document() -> String {
    json!({
        "braketSchemaHeader": { "name": "braket.ir.openqasm.program", "version": "1" },
        "source": "OPENQASM 3.0; qubit[2] q; h q[0]; cnot q[0], q[1];",
    })
    .to_string()
}

/// The body of a `CreateQuantumTask` call on `device_arn`, with the required members
/// filled in.
pub(crate) fn create_body(client_token: &str, device_arn: &str, shots: i64) -> Value {
    json!({
        "clientToken": client_token,
        "deviceArn": device_arn,
        "shots": shots,
        "outputS3Bucket": "braket-emulator-results",
        "outputS3KeyPrefix": "tasks",
        "action": action_document(),
    })
}

// ---------------------------------------------------------------------------
// The modelled vocabularies
// ---------------------------------------------------------------------------

/// Every status value this service can store or report is one the model declares.
///
/// The catalogs are generated from the Smithy model
/// (`ops::types::shared_types`), so this fails the moment the service invents a
/// status, type or queue name the wire does not accept.
#[test]
fn every_value_the_service_stores_is_one_the_model_declares() {
    use service_com_amazonaws_braket_braket::ops::types::{
        CANCELLATION_STATUS_VALUES, DEVICE_STATUS_REASON_CODE_VALUES, DEVICE_STATUS_VALUES,
        DEVICE_TYPE_VALUES, QUANTUM_TASK_STATUS_VALUES, QUEUE_NAME_VALUES, QUEUE_PRIORITY_VALUES,
    };
    use service_com_amazonaws_braket_braket::state as braket_state;

    let cases: [(&str, &str, &[&str]); 16] = [
        ("task status", braket_state::STATUS_CREATED, QUANTUM_TASK_STATUS_VALUES),
        ("task status", braket_state::STATUS_QUEUED, QUANTUM_TASK_STATUS_VALUES),
        ("task status", braket_state::STATUS_FAILED, QUANTUM_TASK_STATUS_VALUES),
        ("task status", braket_state::STATUS_COMPLETED, QUANTUM_TASK_STATUS_VALUES),
        ("task status", braket_state::STATUS_CANCELLING, QUANTUM_TASK_STATUS_VALUES),
        ("task status", braket_state::STATUS_CANCELLED, QUANTUM_TASK_STATUS_VALUES),
        ("cancellation status", braket_state::STATUS_CANCELLING, CANCELLATION_STATUS_VALUES),
        ("cancellation status", braket_state::STATUS_CANCELLED, CANCELLATION_STATUS_VALUES),
        ("device status", braket_state::DEVICE_ONLINE, DEVICE_STATUS_VALUES),
        ("device status", braket_state::DEVICE_OFFLINE, DEVICE_STATUS_VALUES),
        ("device status", braket_state::DEVICE_PAUSED, DEVICE_STATUS_VALUES),
        ("device status", braket_state::DEVICE_RETIRED, DEVICE_STATUS_VALUES),
        (
            "device status reason",
            braket_state::REASON_OUTSIDE_AVAILABILITY_WINDOW,
            DEVICE_STATUS_REASON_CODE_VALUES,
        ),
        (
            "device status reason",
            braket_state::REASON_MAINTENANCE,
            DEVICE_STATUS_REASON_CODE_VALUES,
        ),
        ("queue name", braket_state::QUEUE_QUANTUM_TASKS, QUEUE_NAME_VALUES),
        ("queue priority", braket_state::PRIORITY_PRIORITY, QUEUE_PRIORITY_VALUES),
    ];
    for (kind, value, catalog) in cases {
        assert!(
            catalog.contains(&value),
            "case {kind} {value:?}: expected a value the model declares, \
             but the catalog holds {catalog:?}"
        );
    }
    // The two type values, from the same generated catalog.
    for value in [
        braket_state::DEVICE_TYPE_QPU,
        braket_state::DEVICE_TYPE_SIMULATOR,
    ] {
        assert!(
            DEVICE_TYPE_VALUES.contains(&value),
            "case device type {value:?}: expected a modelled value, got {DEVICE_TYPE_VALUES:?}"
        );
    }
    assert!(
        QUEUE_NAME_VALUES.contains(&braket_state::QUEUE_JOBS)
            && QUEUE_PRIORITY_VALUES.contains(&braket_state::PRIORITY_NORMAL),
        "case queue vocabulary: expected JOBS_QUEUE and Normal to be modelled, \
         got {QUEUE_NAME_VALUES:?} / {QUEUE_PRIORITY_VALUES:?}"
    );
}

// ---------------------------------------------------------------------------
// SearchDevices
// ---------------------------------------------------------------------------

/// A `SearchDevices` filter keeps exactly the devices whose named member equals one of
/// the filter's values — for EVERY filterable member and EVERY value that member takes
/// in the inventory, not only for the one the issue cited.
#[tokio::test]
async fn search_devices_keeps_exactly_the_devices_each_filter_names() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;
    assert!(
        inventory.len() >= 2,
        "case inventory: expected a seeded inventory, got {} devices",
        inventory.len()
    );

    for name in [
        "deviceArn",
        "deviceName",
        "deviceStatus",
        "deviceType",
        "providerName",
    ] {
        let mut values: Vec<String> = inventory.iter().map(|d| member(d, name).to_string()).collect();
        values.sort();
        values.dedup();
        for value in &values {
            let expected: Vec<String> = inventory
                .iter()
                .filter(|d| member(d, name) == value)
                .map(|d| member(d, "deviceArn").to_string())
                .collect();
            let reply = braket
                .search_devices(json!({
                    "filters": [{ "name": name, "values": [value] }],
                    "maxResults": 100,
                }))
                .await;
            assert_eq!(
                reply.status, 200,
                "case filter {name}={value}: expected 200, got {} {}",
                reply.status, reply.body
            );
            let got: Vec<String> = reply.body["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .map(|d| member(d, "deviceArn").to_string())
                .collect();
            assert_eq!(
                got, expected,
                "case filter {name}={value}: expected {expected:?}, got {got:?}"
            );
            assert!(
                !got.is_empty(),
                "case filter {name}={value}: a value taken from the inventory must match \
                 at least one device, got {got:?}"
            );
        }
    }

    // Several values in one filter are an OR; several filters are an AND.
    let statuses: Vec<String> = inventory
        .iter()
        .map(|d| member(d, "deviceStatus").to_string())
        .collect();
    let online_simulators: Vec<String> = inventory
        .iter()
        .filter(|d| member(d, "deviceStatus") == "ONLINE" && member(d, "deviceType") == "SIMULATOR")
        .map(|d| member(d, "deviceArn").to_string())
        .collect();
    let reply = braket
        .search_devices(json!({
            "filters": [
                { "name": "deviceStatus", "values": ["ONLINE", "OFFLINE", "RETIRED"] },
                { "name": "deviceType", "values": ["SIMULATOR"] },
            ],
            "maxResults": 100,
        }))
        .await;
    let got: Vec<String> = reply.body["devices"]
        .as_array()
        .expect("devices")
        .iter()
        .map(|d| member(d, "deviceArn").to_string())
        .collect();
    assert_eq!(
        got, online_simulators,
        "case OR over every status AND deviceType=SIMULATOR: expected {online_simulators:?}, \
         got {got:?} (inventory statuses {statuses:?})"
    );

    // A filter naming something that is not a member of the device summary is refused,
    // not silently ignored. These names are all well-formed values of the filter's
    // `name` member (`@length` 1..64), so the operation itself is what refuses them,
    // as its modelled `ValidationException`.
    for name in ["deviceCapabilities", "DeviceArn", "arn"] {
        let reply = braket
            .search_devices(json!({
                "filters": [{ "name": name, "values": ["anything"] }],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case unsupported filter name {name:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case unsupported filter name {name:?}: expected ValidationException, got {:?}",
            reply.error
        );
    }

    // An EMPTY filter name is outside that member's `@length`, so the request never
    // reaches the operation: the body-rejection layer in front of the service answers
    // it (see `request_body_rejection.rs` for that rule over the whole service).
    let reply = braket
        .search_devices(json!({
            "filters": [{ "name": "", "values": ["anything"] }],
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case empty filter name: expected 400, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("BadRequestException"),
        "case empty filter name: expected BadRequestException, got {:?}",
        reply.error
    );
}

/// Paging returns every match exactly once, in one stable order, whatever the page
/// size — and the traversal ends by omitting `nextToken`.
#[tokio::test]
async fn search_devices_pages_through_the_whole_inventory_for_every_page_size() {
    let braket = Braket::new();
    let all: Vec<String> = braket
        .inventory()
        .await
        .iter()
        .map(|d| member(d, "deviceArn").to_string())
        .collect();

    for page_size in [1_usize, 2, 3, 7, all.len(), all.len() + 1] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "filters": [], "maxResults": page_size });
            if let Some(next) = &token {
                body["nextToken"] = json!(next);
            }
            let reply = braket.search_devices(body).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: expected 200, got {} {}",
                reply.status, reply.body
            );
            let page = reply.body["devices"].as_array().expect("devices").clone();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page returned {} devices",
                page.len()
            );
            seen.extend(page.iter().map(|d| member(d, "deviceArn").to_string()));
            token = reply
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= all.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case page size {page_size}: expected the whole inventory once, in order, got {seen:?}"
        );
    }

    // A token is opaque, and one issued for a different filter set is refused rather
    // than silently paging the wrong result.
    let first = braket
        .search_devices(json!({ "filters": [], "maxResults": 1 }))
        .await;
    let token = first.str_member("nextToken");
    assert!(
        !token.contains('{') && !token.contains("offset"),
        "case opaque token: expected an opaque token, got {token:?}"
    );
    let reply = braket
        .search_devices(json!({
            "filters": [{ "name": "deviceType", "values": ["QPU"] }],
            "maxResults": 1,
            "nextToken": token,
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case token reused with other filters: expected 400, got {} {}",
        reply.status, reply.body
    );
    for bogus in ["not-a-token", "zz", "76313a393939"] {
        let reply = braket
            .search_devices(json!({ "filters": [], "nextToken": bogus }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case bogus token {bogus:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// GetDevice
// ---------------------------------------------------------------------------

/// `GetDevice` reports the seeded device the ARN names — every one of them — and 404s
/// an ARN the inventory does not hold.
#[tokio::test]
async fn get_device_reports_each_seeded_device_and_rejects_unknown_arns() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let reply = braket.get_device(arn).await;
        assert_eq!(
            reply.status, 200,
            "case device {arn}: expected 200, got {} {}",
            reply.status, reply.body
        );
        for name in [
            "deviceArn",
            "deviceName",
            "deviceStatus",
            "deviceType",
            "providerName",
        ] {
            assert_eq!(
                member(&reply.body, name),
                member(summary, name),
                "case device {arn} member {name}: GetDevice and SearchDevices must agree, \
                 got {} vs {}",
                member(&reply.body, name),
                member(summary, name)
            );
        }
        let capabilities: Value = serde_json::from_str(member(&reply.body, "deviceCapabilities"))
            .unwrap_or_else(|e| {
                panic!(
                    "case device {arn}: deviceCapabilities must be a JSON document ({e}): {}",
                    member(&reply.body, "deviceCapabilities")
                )
            });
        assert!(
            capabilities["service"]["shotsRange"].is_array(),
            "case device {arn}: expected a capability document carrying service.shotsRange, \
             got {capabilities}"
        );
        let queues = reply.body["deviceQueueInfo"]
            .as_array()
            .unwrap_or_else(|| panic!("case device {arn}: expected deviceQueueInfo"));
        assert!(
            queues
                .iter()
                .all(|q| q["queueSize"].as_str().is_some_and(|s| s.parse::<u64>().is_ok())),
            "case device {arn}: every queueSize must be a number, got {queues:?}"
        );
    }

    // A well-formed device ARN the seeded inventory does not hold is a 404. An ARN
    // that is not a device ARN at all is invalid INPUT, not a missing resource: that
    // rule and its table live in `tests/all/arn_syntax.rs`.
    for unknown in [
        "arn:aws:braket:::device/quantum-simulator/amazon/nope",
        "arn:aws:braket:us-east-1::device/qpu/ionq/Aria-9",
        "arn:aws-us-gov:braket:us-gov-west-1:000000000000:device/qpu/nobody/Nothing",
    ] {
        let reply = braket.get_device(unknown).await;
        assert_eq!(
            reply.status, 404,
            "case unknown device {unknown}: expected 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case unknown device {unknown}: expected ResourceNotFoundException, got {:?}",
            reply.error
        );
    }
}

// ---------------------------------------------------------------------------
// CreateQuantumTask / GetQuantumTask
// ---------------------------------------------------------------------------

/// What `CreateQuantumTask` stores is what `GetQuantumTask` reports — over a table of
/// requests differing in device, shot count, key prefix, device parameters and tags.
#[tokio::test]
async fn a_created_quantum_task_reads_back_exactly_as_it_was_created() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;
    let online: Vec<String> = inventory
        .iter()
        .filter(|d| member(d, "deviceStatus") == "ONLINE")
        .map(|d| member(d, "deviceArn").to_string())
        .collect();
    assert!(
        online.len() >= 2,
        "case inventory: expected at least two ONLINE devices, got {online:?}"
    );

    // (shots, key prefix, deviceParameters, tags) — one row per thing the rule
    // generalises over.
    let rows: Vec<(i64, &str, Option<&str>, Value)> = vec![
        (1, "tasks", None, json!(null)),
        (100, "nested/key/prefix", Some("{\"braketSchemaHeader\":{\"name\":\"x\"}}"), json!(null)),
        (
            1_000,
            "p",
            None,
            json!({ "team": "quantum", "env": "test" }),
        ),
        (1, "trailing", Some("{}"), json!({ "only": "one" })),
    ];

    for (index, (shots, prefix, device_parameters, tags)) in rows.iter().enumerate() {
        let device_arn = &online[index % online.len()];
        let client_token = format!("token-{index}");
        let mut body = create_body(&client_token, device_arn, *shots);
        body["outputS3KeyPrefix"] = json!(prefix);
        body["outputS3Bucket"] = json!(format!("bucket-{index}"));
        if let Some(parameters) = device_parameters {
            body["deviceParameters"] = json!(parameters);
        }
        if tags.is_object() {
            body["tags"] = tags.clone();
        }

        let created = braket.create_task(body).await;
        assert_eq!(
            created.status, 200,
            "case row {index}: expected 200 OK, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        let expected_prefix = format!("arn:aws:braket:{REGION}:{ACCOUNT}:quantum-task/");
        assert!(
            arn.starts_with(&expected_prefix),
            "case row {index}: expected an ARN under {expected_prefix}, got {arn}"
        );
        let task_id = arn.rsplit('/').next().expect("an ARN resource part");
        assert_eq!(
            task_id.len(),
            36,
            "case row {index}: expected a UUID-shaped task id, got {task_id}"
        );

        let fetched = braket.get_task(&arn).await;
        assert_eq!(
            fetched.status, 200,
            "case row {index}: expected 200, got {} {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            member(&fetched.body, "quantumTaskArn"),
            arn,
            "case row {index}: the ARN must round-trip"
        );
        assert_eq!(
            member(&fetched.body, "deviceArn"),
            device_arn.as_str(),
            "case row {index}: deviceArn must round-trip"
        );
        assert_eq!(
            fetched.body["shots"].as_i64(),
            Some(*shots),
            "case row {index}: expected shots {shots}, got {}",
            fetched.body["shots"]
        );
        assert_eq!(
            member(&fetched.body, "outputS3Bucket"),
            format!("bucket-{index}"),
            "case row {index}: outputS3Bucket must round-trip"
        );
        assert_eq!(
            member(&fetched.body, "outputS3Directory"),
            format!("{prefix}/{task_id}"),
            "case row {index}: expected outputS3Directory <keyPrefix>/<taskId>, got {}",
            member(&fetched.body, "outputS3Directory")
        );
        // `deviceParameters` is what the task RAN UNDER: the submitted document as it was
        // sent, or the device's own default document for the program when the create
        // carried none. (The rule itself is pinned over a table by
        // `device_parameters_report_what_the_task_ran_under` below.)
        let reported_parameters = member(&fetched.body, "deviceParameters");
        match device_parameters {
            Some(sent) => assert_eq!(
                reported_parameters, *sent,
                "case row {index}: a submitted deviceParameters document comes back as sent"
            ),
            None => {
                let document: Value = serde_json::from_str(reported_parameters)
                    .unwrap_or_else(|e| {
                        panic!("case row {index}: {reported_parameters} is not JSON: {e}")
                    });
                assert!(
                    member(&document["braketSchemaHeader"], "name")
                        .ends_with("_device_parameters"),
                    "case row {index}: expected the device's parameters schema, got \
                     {document}"
                );
                assert_eq!(
                    document["paradigmParameters"]["qubitCount"].as_u64(),
                    Some(2),
                    "case row {index}: expected the program's qubit count, got {document}"
                );
            }
        }
        assert_eq!(
            member(&fetched.body, "status"),
            "CREATED",
            "case row {index}: a task just created is CREATED, got {}",
            member(&fetched.body, "status")
        );
        assert!(
            fetched.body.get("createdAt").is_some(),
            "case row {index}: createdAt is required, got {}",
            fetched.body
        );
        assert!(
            fetched.body.get("endedAt").is_none() && fetched.body.get("failureReason").is_none(),
            "case row {index}: a running task has not ended, got {}",
            fetched.body
        );
        if tags.is_object() {
            assert_eq!(
                fetched.body.get("tags"),
                Some(tags),
                "case row {index}: tags must round-trip, got {:?}",
                fetched.body.get("tags")
            );
        } else {
            assert_eq!(
                fetched.body.get("tags"),
                Some(&json!({})),
                "case row {index}: a task with no tags reports an empty map, got {:?}",
                fetched.body.get("tags")
            );
        }
        // `queueInfo` only when it is asked for.
        assert!(
            fetched.body.get("queueInfo").is_none(),
            "case row {index}: queueInfo must be withheld unless requested, got {}",
            fetched.body
        );
        let with_queue = braket.get_task_with_queue_info(&arn).await;
        let position = member(&with_queue.body["queueInfo"], "position");
        assert!(
            position.parse::<u64>().is_ok(),
            "case row {index}: expected a queue position for a queued task, got {position}"
        );
    }

    // The optional members a task may be created with come back as they were sent:
    // the experimental-capabilities union and the association list.
    let device_arn = &online[0];
    let mut extras = create_body("token-extras", device_arn, 1);
    extras["experimentalCapabilities"] = json!({ "enabled": "ALL" });
    extras["associations"] = json!([{
        "arn": format!("arn:aws:braket:{REGION}:{ACCOUNT}:reservation/abc"),
        "type": "RESERVATION_TIME_WINDOW_ARN",
    }]);
    let with_extras = braket.create_task(extras.clone()).await;
    assert_eq!(
        with_extras.status, 200,
        "case optional members: expected 200, got {} {}",
        with_extras.status, with_extras.body
    );
    let read_extras = braket
        .get_task(&with_extras.str_member("quantumTaskArn"))
        .await;
    assert_eq!(
        read_extras.body.get("experimentalCapabilities"),
        extras.get("experimentalCapabilities"),
        "case optional members: experimentalCapabilities must round-trip, got {}",
        read_extras.body
    );
    assert_eq!(
        read_extras.body.get("associations"),
        extras.get("associations"),
        "case optional members: associations must round-trip, got {}",
        read_extras.body
    );

    // A well-formed quantum-task ARN no record is held for is a 404; a value that is
    // not a quantum-task ARN at all is a 400 (see `tests/all/arn_syntax.rs`).
    for unknown in [
        "arn:aws:braket:eu-west-1:000000000000:quantum-task/00000000-0000-4000-8000-000000000000",
        "arn:aws-cn:braket:cn-north-1:123456789012:quantum-task/not-created-here",
    ] {
        let reply = braket.get_task(unknown).await;
        assert_eq!(
            reply.status, 404,
            "case unknown task {unknown}: expected 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case unknown task {unknown}: expected ResourceNotFoundException, got {:?}",
            reply.error
        );
    }
}

/// One `clientToken` creates one task: replaying the same request returns the first
/// ARN, and reusing the token for a different request is refused.
#[tokio::test]
async fn one_client_token_creates_exactly_one_quantum_task() {
    let braket = Braket::new();
    let device_arn = braket.an_online_device().await;

    // Each row is a different request body under its own token; replaying any of them
    // must return that row's ARN.
    let rows: Vec<(&str, i64, &str)> = vec![
        ("tok-a", 1, "tasks"),
        ("tok-b", 10, "other/prefix"),
        ("tok-c", 1, "tasks"),
    ];
    let mut minted: Vec<String> = Vec::new();
    for (token, shots, prefix) in &rows {
        let mut body = create_body(token, &device_arn, *shots);
        body["outputS3KeyPrefix"] = json!(prefix);
        let first = braket.create_task(body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case token {token}: expected 200, got {} {}",
            first.status, first.body
        );
        let arn = first.str_member("quantumTaskArn");
        for attempt in 0..3 {
            let replay = braket.create_task(body.clone()).await;
            assert_eq!(
                replay.status, 200,
                "case token {token} replay {attempt}: expected 200, got {} {}",
                replay.status, replay.body
            );
            assert_eq!(
                replay.str_member("quantumTaskArn"),
                arn,
                "case token {token} replay {attempt}: expected the same ARN"
            );
        }
        minted.push(arn);
    }

    // Exactly one task per token exists — replays created nothing.
    let listed = braket.search_tasks(json!({ "filters": [] })).await;
    let arns: Vec<String> = listed.body["quantumTasks"]
        .as_array()
        .expect("quantumTasks")
        .iter()
        .map(|t| member(t, "quantumTaskArn").to_string())
        .collect();
    assert_eq!(
        arns.len(),
        rows.len(),
        "case replay count: expected {} tasks, got {arns:?}",
        rows.len()
    );
    let mut sorted_minted = minted.clone();
    sorted_minted.sort();
    let mut sorted_arns = arns.clone();
    sorted_arns.sort();
    assert_eq!(
        sorted_arns, sorted_minted,
        "case replay count: the stored tasks must be exactly the minted ones"
    );

    // The same token with a different body is a different request: refused.
    for (field, changed) in [
        ("shots", json!(2)),
        ("outputS3Bucket", json!("another-bucket")),
        ("outputS3KeyPrefix", json!("elsewhere")),
        ("action", json!("{\"braketSchemaHeader\":{\"name\":\"other\"}}")),
        ("tags", json!({ "new": "tag" })),
    ] {
        let mut body = create_body("tok-a", &device_arn, 1);
        body["outputS3KeyPrefix"] = json!("tasks");
        body[field] = changed.clone();
        let reply = braket.create_task(body).await;
        assert_eq!(
            reply.status, 400,
            "case token reuse with different {field}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case token reuse with different {field}: expected ValidationException, got {:?}",
            reply.error
        );
    }
}

/// Which device a task may be created on is decided by the stored device: an unknown
/// ARN, an offline device and a retired device each get their own modelled refusal.
#[tokio::test]
async fn create_quantum_task_refuses_by_device_status_and_shot_range() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    // (device status, expected HTTP status, expected error code) — one row per
    // modelled DeviceStatus, plus an ARN that is in no inventory at all.
    let rows: Vec<(&str, u16, &str)> = vec![
        ("ONLINE", 200, ""),
        ("OFFLINE", 424, "DeviceOfflineException"),
        ("RETIRED", 410, "DeviceRetiredException"),
    ];
    for (status, expected_status, expected_error) in &rows {
        let device_arn = inventory
            .iter()
            .find(|d| member(d, "deviceStatus") == *status)
            .map(|d| member(d, "deviceArn").to_string())
            .unwrap_or_else(|| panic!("the seeded inventory must hold a {status} device"));
        let reply = braket
            .create_task(create_body(&format!("tok-{status}"), &device_arn, 1))
            .await;
        assert_eq!(
            reply.status, *expected_status,
            "case {status} device {device_arn}: expected {expected_status}, got {} {}",
            reply.status, reply.body
        );
        if !expected_error.is_empty() {
            assert_eq!(
                reply.error.as_deref(),
                Some(*expected_error),
                "case {status} device {device_arn}: expected {expected_error}, got {:?}",
                reply.error
            );
        }
    }

    for unknown in [
        "arn:aws:braket:::device/quantum-simulator/amazon/sv9",
        "arn:aws:braket:eu-west-1::device/qpu/nobody/Nothing",
        "arn:aws:braket:us-east-1:000000000000:device/qpu/nobody/Nothing",
    ] {
        let reply = braket
            .create_task(create_body("tok-unknown", unknown, 1))
            .await;
        assert_eq!(
            reply.status, 400,
            "case unknown device {unknown}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case unknown device {unknown}: expected ValidationException, got {:?}",
            reply.error
        );
    }

    // A shot count outside the range the device's own capability document publishes is
    // refused; each published bound is accepted.
    for summary in inventory
        .iter()
        .filter(|d| member(d, "deviceStatus") == "ONLINE")
    {
        let arn = member(summary, "deviceArn");
        let capabilities: Value =
            serde_json::from_str(member(&braket.get_device(arn).await.body, "deviceCapabilities"))
                .expect("a JSON capability document");
        let range = capabilities["service"]["shotsRange"]
            .as_array()
            .expect("service.shotsRange");
        let (min, max) = (
            range[0].as_i64().expect("a numeric lower bound"),
            range[1].as_i64().expect("a numeric upper bound"),
        );
        for (shots, expected) in [(min, 200_u16), (max, 200), (max + 1, 400)] {
            let token = format!("tok-{arn}-{shots}");
            let reply = braket.create_task(create_body(&token, arn, shots)).await;
            assert_eq!(
                reply.status, expected,
                "case device {arn} with {shots} shots (published range {min}..={max}): \
                 expected {expected}, got {} {}",
                reply.status, reply.body
            );
        }
        if min > 0 {
            let reply = braket
                .create_task(create_body(&format!("tok-{arn}-under"), arn, min - 1))
                .await;
            assert_eq!(
                reply.status, 400,
                "case device {arn} with {} shots (below the published minimum {min}): \
                 expected 400, got {} {}",
                min - 1,
                reply.status,
                reply.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SearchQuantumTasks
// ---------------------------------------------------------------------------

/// A `SearchQuantumTasks` filter keeps exactly the stored tasks its name, operator and
/// values select, and paging walks the matches once in a stable order.
#[tokio::test]
async fn search_quantum_tasks_applies_every_filter_name_and_operator() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;
    let online: Vec<String> = inventory
        .iter()
        .filter(|d| member(d, "deviceStatus") == "ONLINE")
        .map(|d| member(d, "deviceArn").to_string())
        .collect();

    // Four tasks over two devices, each with a distinct creation instant.
    let mut created: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        let device_arn = online[index % 2].clone();
        let reply = braket
            .create_task(create_body(&format!("tok-{index}"), &device_arn, 1))
            .await;
        assert_eq!(
            reply.status, 200,
            "case setup {index}: expected 200, got {} {}",
            reply.status, reply.body
        );
        created.push((reply.str_member("quantumTaskArn"), device_arn));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let created_at_of = |arn: &str, body: &Value| -> String {
        body["quantumTasks"]
            .as_array()
            .expect("quantumTasks")
            .iter()
            .find(|t| member(t, "quantumTaskArn") == arn)
            .map(|t| t["createdAt"].to_string())
            .unwrap_or_default()
    };
    let all = braket.search_tasks(json!({ "filters": [] })).await;
    assert_eq!(
        all.body["quantumTasks"].as_array().expect("quantumTasks").len(),
        4,
        "case no filters: expected every stored task, got {}",
        all.body
    );
    assert!(
        !created_at_of(&created[0].0, &all.body).is_empty(),
        "case no filters: a summary must carry createdAt, got {}",
        all.body
    );

    // Each row: (filter, the ARNs it must return).
    let first_device = &created[0].1;
    let on_first_device: Vec<String> = created
        .iter()
        .filter(|(_, device)| device == first_device)
        .map(|(arn, _)| arn.clone())
        .collect();
    let rows: Vec<(Value, Vec<String>)> = vec![
        (
            json!({ "name": "deviceArn", "operator": "EQUAL", "values": [first_device] }),
            on_first_device.clone(),
        ),
        (
            json!({ "name": "quantumTaskArn", "operator": "EQUAL", "values": [created[2].0] }),
            vec![created[2].0.clone()],
        ),
        (
            json!({
                "name": "quantumTaskArn",
                "operator": "EQUAL",
                "values": [created[0].0, created[3].0],
            }),
            vec![created[0].0.clone(), created[3].0.clone()],
        ),
        (
            json!({ "name": "jobArn", "operator": "EQUAL", "values": ["arn:aws:braket:eu-west-1:000000000000:job/none"] }),
            Vec::new(),
        ),
        (
            json!({ "name": "createdAt", "operator": "GTE", "values": ["2000-01-01T00:00:00Z"] }),
            created.iter().map(|(arn, _)| arn.clone()).collect(),
        ),
        (
            json!({ "name": "createdAt", "operator": "LT", "values": ["2000-01-01T00:00:00Z"] }),
            Vec::new(),
        ),
        (
            json!({
                "name": "createdAt",
                "operator": "BETWEEN",
                "values": ["2000-01-01T00:00:00Z", "2999-01-01T00:00:00Z"],
            }),
            created.iter().map(|(arn, _)| arn.clone()).collect(),
        ),
        (
            json!({ "name": "createdAt", "operator": "GT", "values": ["2999-01-01T00:00:00Z"] }),
            Vec::new(),
        ),
    ];
    for (index, (filter, expected)) in rows.iter().enumerate() {
        let reply = braket
            .search_tasks(json!({ "filters": [filter], "maxResults": 100 }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case filter row {index} {filter}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let mut got: Vec<String> = reply.body["quantumTasks"]
            .as_array()
            .expect("quantumTasks")
            .iter()
            .map(|t| member(t, "quantumTaskArn").to_string())
            .collect();
        let mut want = expected.clone();
        got.sort();
        want.sort();
        assert_eq!(
            got, want,
            "case filter row {index} {filter}: expected {want:?}, got {got:?}"
        );
    }

    // A `status` filter partitions the tasks: each status returns only tasks in that
    // status, and together the modelled statuses return every task exactly once. That
    // holds whatever status the clock has moved each task to by now.
    let mut partitioned: Vec<String> = Vec::new();
    for status in service_com_amazonaws_braket_braket::ops::types::QUANTUM_TASK_STATUS_VALUES {
        let reply = braket
            .search_tasks(json!({
                "filters": [{ "name": "status", "operator": "EQUAL", "values": [status] }],
                "maxResults": 100,
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case status filter {status}: expected 200, got {} {}",
            reply.status, reply.body
        );
        for task in reply.body["quantumTasks"].as_array().expect("quantumTasks") {
            assert_eq!(
                member(task, "status"),
                *status,
                "case status filter {status}: a returned task reports {}",
                member(task, "status")
            );
            partitioned.push(member(task, "quantumTaskArn").to_string());
        }
    }
    partitioned.sort();
    let mut every_arn: Vec<String> = created.iter().map(|(arn, _)| arn.clone()).collect();
    every_arn.sort();
    assert_eq!(
        partitioned, every_arn,
        "case status partition: expected every task under exactly one status, got {partitioned:?}"
    );

    // Two filters hold together.
    let reply = braket
        .search_tasks(json!({
            "filters": [
                { "name": "deviceArn", "operator": "EQUAL", "values": [first_device] },
                { "name": "status", "operator": "EQUAL", "values": ["COMPLETED"] },
            ],
        }))
        .await;
    assert!(
        reply.body["quantumTasks"]
            .as_array()
            .expect("quantumTasks")
            .is_empty(),
        "case AND of two filters: expected no match, got {}",
        reply.body
    );

    // Paging over the four tasks, for every page size.
    for page_size in [1_usize, 2, 3, 4, 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "filters": [], "maxResults": page_size });
            if let Some(next) = &token {
                body["nextToken"] = json!(next);
            }
            let reply = braket.search_tasks(body).await;
            let page = reply.body["quantumTasks"].as_array().expect("quantumTasks");
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page returned {} tasks",
                page.len()
            );
            seen.extend(page.iter().map(|t| member(t, "quantumTaskArn").to_string()));
            token = reply
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            4,
            "case page size {page_size}: expected all four tasks exactly once, got {seen:?}"
        );
    }

    // Refusals: an unsupported name, an unsupported operator, an ordering operator on
    // a member that is not ordered, and a value count the operator cannot read.
    let refusals: Vec<Value> = vec![
        json!({ "name": "shots", "operator": "EQUAL", "values": ["1"] }),
        json!({ "name": "deviceArn", "operator": "CONTAINS", "values": ["x"] }),
        json!({ "name": "deviceArn", "operator": "GT", "values": ["a"] }),
        json!({ "name": "status", "operator": "LTE", "values": ["CREATED"] }),
        json!({ "name": "quantumTaskArn", "operator": "BETWEEN", "values": ["a", "b"] }),
        json!({ "name": "createdAt", "operator": "BETWEEN", "values": ["2000-01-01T00:00:00Z"] }),
        json!({
            "name": "createdAt",
            "operator": "BETWEEN",
            "values": [
                "2000-01-01T00:00:00Z",
                "2001-01-01T00:00:00Z",
                "2002-01-01T00:00:00Z",
            ],
        }),
        json!({
            "name": "createdAt",
            "operator": "GT",
            "values": ["2000-01-01T00:00:00Z", "2001-01-01T00:00:00Z"],
        }),
        json!({ "name": "createdAt", "operator": "LT", "values": [] }),
        json!({ "name": "createdAt", "operator": "EQUAL", "values": ["yesterday"] }),
        json!({ "name": "deviceArn", "operator": "EQUAL", "values": [] }),
    ];
    for filter in &refusals {
        let reply = braket.search_tasks(json!({ "filters": [filter] })).await;
        assert_eq!(
            reply.status, 400,
            "case refused filter {filter}: expected 400, got {} {}",
            reply.status, reply.body
        );
    }

    // The work one request can ask for is bounded: more filters than the model allows,
    // or more values in one filter, is refused before any task is read.
    let one = json!({ "name": "status", "operator": "EQUAL", "values": ["CREATED"] });
    let too_many_filters: Vec<Value> =
        (0..=service_com_amazonaws_braket_braket::state::MAX_FILTERS_PER_REQUEST)
            .map(|_| one.clone())
            .collect();
    let reply = braket
        .search_tasks(json!({ "filters": too_many_filters }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case {} filters: expected 400, got {} {}",
        service_com_amazonaws_braket_braket::state::MAX_FILTERS_PER_REQUEST + 1,
        reply.status,
        reply.body
    );
    let too_many_values: Vec<String> =
        (0..=service_com_amazonaws_braket_braket::state::MAX_VALUES_PER_FILTER)
            .map(|index| format!("CREATED-{index}"))
            .collect();
    let reply = braket
        .search_tasks(json!({
            "filters": [{ "name": "status", "operator": "EQUAL", "values": too_many_values }],
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case {} values in one filter: expected 400, got {} {}",
        service_com_amazonaws_braket_braket::state::MAX_VALUES_PER_FILTER + 1,
        reply.status,
        reply.body
    );
    // The same two bounds hold for SearchDevices.
    let device_filter = json!({ "name": "deviceType", "values": ["SIMULATOR"] });
    let reply = braket
        .search_devices(json!({
            "filters": (0..=service_com_amazonaws_braket_braket::state::MAX_FILTERS_PER_REQUEST)
                .map(|_| device_filter.clone())
                .collect::<Vec<Value>>(),
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case too many device filters: expected 400, got {} {}",
        reply.status, reply.body
    );
    let reply = braket
        .search_devices(json!({
            "filters": [{ "name": "deviceType", "values": too_many_values }],
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case too many values in a device filter: expected 400, got {} {}",
        reply.status, reply.body
    );
}

// ---------------------------------------------------------------------------
// The lifecycle, and CancelQuantumTask
// ---------------------------------------------------------------------------

/// A task's status advances on the clock, not on a call: it is `CREATED` the instant
/// after it is created, reaches `QUEUED`, and ends in a terminal status with `endedAt`.
///
/// This harness mounts no object store, so the result document of the run cannot be
/// written anywhere and the task ends in the modelled failure status carrying that
/// reason — a task is reported as completed only where its results really are (see
/// `tests/all/quantum_task_execution.rs`, which mounts Amazon S3 beside this service).
#[tokio::test]
async fn a_quantum_task_advances_on_the_clock_and_ends_in_a_modelled_status() {
    use service_com_amazonaws_braket_braket::state::TERMINAL_SPAN;

    let braket = Braket::new();
    let device_arn = braket.an_online_device().await;
    let created = braket
        .create_task(create_body("tok-clock", &device_arn, 1))
        .await;
    // The task's age is measured from here, just after the create returned, so every
    // assertion below is a function of the elapsed time rather than of a fixed sleep:
    // a stalled machine changes which status is owed, and the test asserts the owed
    // one instead of flaking.
    let accepted_at = std::time::Instant::now();
    let arn = created.str_member("quantumTaskArn");

    // The transition is never due immediately, so the caller still sees CREATED.
    let age = accepted_at.elapsed();
    let right_after = braket.get_task(&arn).await;
    assert_clock_consistent(
        member(&right_after.body, "status"),
        age,
        "immediately after create",
    );

    tokio::time::sleep(Duration::from_millis(700)).await;
    let age = accepted_at.elapsed();
    let queued = braket.get_task(&arn).await;
    assert_clock_consistent(member(&queued.body, "status"), age, "700ms after create");
    // A read reports the status; it does not move it on.
    let age = accepted_at.elapsed();
    let again = braket.get_task(&arn).await;
    assert_eq!(
        member(&again.body, "status"),
        member(&queued.body, "status"),
        "case read twice ({age:?} old): a read must not advance the status, got {}",
        again.body
    );

    // Past the terminal instant, whatever the machine did in between.
    while accepted_at.elapsed() < TERMINAL_SPAN + CLOCK_GUARD {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ended = braket.get_task(&arn).await;
    assert_eq!(
        member(&ended.body, "status"),
        "FAILED",
        "case {:?} after create: expected the modelled failure status, got {}",
        accepted_at.elapsed(),
        ended.body
    );
    assert!(
        ended.body.get("endedAt").is_some(),
        "case terminal task: endedAt must be set, got {}",
        ended.body
    );
    assert!(
        member(&ended.body, "failureReason").contains("could not be written"),
        "case terminal task: expected a reason saying the result could not be stored, \
         got {}",
        member(&ended.body, "failureReason")
    );
    // The terminal status is stable: the clock owes nothing more.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let still = braket.get_task(&arn).await;
    assert_eq!(
        member(&still.body, "status"),
        "FAILED",
        "case after the terminal status: expected FAILED to stick, got {}",
        still.body
    );
    // It has left the queue, so it has no queue position.
    let with_queue = braket.get_task_with_queue_info(&arn).await;
    assert_eq!(
        member(&with_queue.body["queueInfo"], "position"),
        "None",
        "case terminal task queue info: expected no position, got {}",
        with_queue.body["queueInfo"]
    );
    // A terminal task is also gone from its device's quantum-task queue depth.
    let device = braket.get_device(&device_arn).await;
    let depth: u64 = device.body["deviceQueueInfo"]
        .as_array()
        .expect("deviceQueueInfo")
        .iter()
        .filter_map(|q| q["queueSize"].as_str())
        .filter_map(|s| s.parse::<u64>().ok())
        .sum();
    assert_eq!(
        depth, 0,
        "case device queue depth: expected 0 once every task is terminal, got {}",
        device.body["deviceQueueInfo"]
    );
}

/// Cancelling moves the stored task, and what may be cancelled depends on the status
/// the task is in.
#[tokio::test]
async fn cancel_quantum_task_moves_the_record_and_conflicts_on_a_finished_task() {
    let braket = Braket::new();
    let device_arn = braket.an_online_device().await;

    let created = braket
        .create_task(create_body("tok-cancel", &device_arn, 1))
        .await;
    let arn = created.str_member("quantumTaskArn");

    let cancelled = braket.cancel_task(&arn, "cancel-token-1").await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel a CREATED task: expected 200, got {} {}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        member(&cancelled.body, "quantumTaskArn"),
        arn,
        "case cancel: the ARN must come back"
    );
    assert_eq!(
        member(&cancelled.body, "cancellationStatus"),
        "CANCELLING",
        "case cancel a CREATED task: expected CANCELLING, got {}",
        cancelled.body
    );
    let after = braket.get_task(&arn).await;
    assert_eq!(
        member(&after.body, "status"),
        "CANCELLING",
        "case after cancel: GetQuantumTask must report the cancellation, got {}",
        after.body
    );

    // `@idempotent`: cancelling again reports where the cancellation has got to.
    for token in ["cancel-token-1", "cancel-token-2"] {
        let again = braket.cancel_task(&arn, token).await;
        assert_eq!(
            again.status, 200,
            "case re-cancel with {token}: expected 200, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            member(&again.body, "cancellationStatus"),
            "CANCELLING",
            "case re-cancel with {token}: expected CANCELLING, got {}",
            again.body
        );
    }

    // The CANCELLING -> CANCELLED transition is owed by the clock.
    tokio::time::sleep(Duration::from_millis(850)).await;
    let terminal = braket.get_task(&arn).await;
    assert_eq!(
        member(&terminal.body, "status"),
        "CANCELLED",
        "case 850ms after cancel: expected CANCELLED, got {}",
        terminal.body
    );
    assert!(
        terminal.body.get("endedAt").is_some(),
        "case cancelled task: endedAt must be set, got {}",
        terminal.body
    );
    let once_cancelled = braket.cancel_task(&arn, "cancel-token-3").await;
    assert_eq!(
        member(&once_cancelled.body, "cancellationStatus"),
        "CANCELLED",
        "case cancel an already cancelled task: expected CANCELLED, got {}",
        once_cancelled.body
    );
    // And a cancelled task is findable by its new status.
    let found = braket
        .search_tasks(json!({
            "filters": [{ "name": "status", "operator": "EQUAL", "values": ["CANCELLED"] }],
        }))
        .await;
    let arns: Vec<String> = found.body["quantumTasks"]
        .as_array()
        .expect("quantumTasks")
        .iter()
        .map(|t| member(t, "quantumTaskArn").to_string())
        .collect();
    assert_eq!(
        arns,
        vec![arn.clone()],
        "case search by CANCELLED: expected the cancelled task, got {arns:?}"
    );

    // A task that finished without being cancelled can no longer be cancelled.
    let other = braket
        .create_task(create_body("tok-finished", &device_arn, 1))
        .await;
    let finished_arn = other.str_member("quantumTaskArn");
    tokio::time::sleep(Duration::from_millis(1_400)).await;
    assert_eq!(
        member(&braket.get_task(&finished_arn).await.body, "status"),
        "FAILED",
        "case setup: the task must have reached a terminal status"
    );
    let conflict = braket.cancel_task(&finished_arn, "cancel-token-4").await;
    assert_eq!(
        conflict.status, 409,
        "case cancel a finished task: expected 409, got {} {}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error.as_deref(),
        Some("ConflictException"),
        "case cancel a finished task: expected ConflictException, got {:?}",
        conflict.error
    );

    // A well-formed ARN no task is held for is a 404; a value that is not a
    // quantum-task ARN at all is a 400 (see `tests/all/arn_syntax.rs`).
    for unknown in [
        "arn:aws:braket:eu-west-1:000000000000:quantum-task/11111111-1111-4111-8111-111111111111",
        "arn:aws:braket:::quantum-task/never-created",
    ] {
        let reply = braket.cancel_task(unknown, "cancel-token-5").await;
        assert_eq!(
            reply.status, 404,
            "case cancel unknown {unknown}: expected 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case cancel unknown {unknown}: expected ResourceNotFoundException, got {:?}",
            reply.error
        );
    }
}

// ---------------------------------------------------------------------------
// The bounds on the caller-driven stores
// ---------------------------------------------------------------------------

/// The state's own task record, named once for the tests that seed the store.
type QuantumTaskRecord = service_com_amazonaws_braket_braket::state::QuantumTask;

/// A synthetic task record, for the tests that seed the store rather than driving a
/// thousand lifecycles through the API. `terminal_due` decides whether the clock has
/// anything left to do with it.
fn synthetic_task(
    arn: &str,
    device_arn: &str,
    status: &str,
    created_at: SystemTime,
    terminal_due: SystemTime,
) -> QuantumTaskRecord {
    use service_com_amazonaws_braket_braket::state::STATUS_CREATED;
    QuantumTaskRecord {
        quantum_task_arn: arn.to_string(),
        status: status.to_string(),
        device_arn: device_arn.to_string(),
        device_parameters: "{}".to_string(),
        action: "{}".to_string(),
        shots: 1,
        output_s3_bucket: "bucket".to_string(),
        output_s3_key_prefix: "tasks".to_string(),
        output_s3_directory: "tasks/seed".to_string(),
        created_at,
        ended_at: (status != STATUS_CREATED).then_some(created_at),
        failure_reason: None,
        job_arn: None,
        job_token: None,
        associations: Vec::new(),
        tags: BTreeMap::new(),
        experimental_capabilities: None,
        queued_due: terminal_due,
        terminal_due,
        cancelled_due: None,
        // A seeded record stands in for one whose program produced no result, which is
        // the terminal status the clock owes it.
        outcome: service_com_amazonaws_braket_braket::state::TaskOutcome::Failed {
            reason: service_com_amazonaws_braket_braket::state::NOT_EXECUTED_FAILURE_REASON
                .to_string(),
        },
        result_object_key: None,
    }
}

/// A device admits only its quota of quantum tasks in flight, and the quota is per
/// device: the refusal is the modelled `ServiceQuotaExceededException` (402), and
/// another device is unaffected.
///
/// The quota is reached by SEEDING the store with that many in-flight records whose
/// clock transitions are an hour out, so the assertion cannot depend on how fast the
/// machine pushes requests; the create that is refused, and the one that is not, both
/// go over HTTP.
#[tokio::test]
async fn a_device_admits_only_its_quota_of_tasks_in_flight() {
    use service_com_amazonaws_braket_braket::state::{
        service_state, MAX_TASKS_IN_FLIGHT_PER_DEVICE as QUOTA, STATUS_CREATED,
    };

    let braket = Braket::new();
    let inventory = braket.inventory().await;
    let online: Vec<String> = inventory
        .iter()
        .filter(|d| member(d, "deviceStatus") == "ONLINE")
        .map(|d| member(d, "deviceArn").to_string())
        .collect();
    assert!(
        online.len() >= 2,
        "case inventory: the quota is per device, so this case needs two ONLINE devices, \
         got {online:?}"
    );
    let device_arn = &online[0];

    let now = SystemTime::now();
    {
        let state = service_state(&*braket.engine).expect("the Braket store");
        let mut guard = state.lock();
        for index in 0..QUOTA {
            let arn =
                format!("arn:aws:braket:eu-west-1:000000000000:quantum-task/seed-{index:08}");
            guard.quantum_tasks.insert(
                arn.clone(),
                synthetic_task(
                    &arn,
                    device_arn,
                    STATUS_CREATED,
                    now,
                    now + Duration::from_secs(3_600),
                ),
            );
        }
    }

    let refusal = braket
        .create_task(create_body("quota-over", device_arn, 1))
        .await;
    assert_eq!(
        refusal.status, 402,
        "case quota reached on {device_arn}: expected 402, got {} {}",
        refusal.status, refusal.body
    );
    assert_eq!(
        refusal.error.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case quota reached on {device_arn}: expected ServiceQuotaExceededException, got {:?}",
        refusal.error
    );

    // The quota is the DEVICE's, so another device still accepts work.
    let elsewhere = braket
        .create_task(create_body("quota-other-device", &online[1], 1))
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "case quota on another device {}: expected 200, got {} {}",
        online[1], elsewhere.status, elsewhere.body
    );

    // And a slot freed by the clock is a slot the device admits again: drop one of the
    // seeded in-flight records and the same create succeeds.
    {
        let state = service_state(&*braket.engine).expect("the Braket store");
        let mut guard = state.lock();
        let first = guard
            .quantum_tasks
            .values()
            .find(|task| task.device_arn == *device_arn)
            .map(|task| task.quantum_task_arn.clone())
            .expect("a seeded task on this device");
        guard.quantum_tasks.remove(&first);
    }
    let admitted = braket
        .create_task(create_body("quota-after-a-slot-frees", device_arn, 1))
        .await;
    assert_eq!(
        admitted.status, 200,
        "case a freed slot on {device_arn}: expected 200, got {} {}",
        admitted.status, admitted.body
    );
}

/// The task store keeps fewer than `MAX_QUANTUM_TASKS` records, dropping the OLDEST
/// FINISHED ones and the idempotency tokens that name them — and never a task that is
/// still in flight.
///
/// Driven against the state struct, because filling the store over HTTP would mean
/// waiting out a thousand task lifecycles.
#[test]
fn the_store_drops_only_the_oldest_finished_records_over_capacity() {
    use service_com_amazonaws_braket_braket::state::{
        ComAmazonawsBraketBraketState, IdempotencyRecord, MAX_QUANTUM_TASKS, STATUS_CREATED,
        STATUS_FAILED,
    };

    let arn_of = |index: usize| format!("arn:aws:braket:eu-west-1:000000000000:quantum-task/{index:08}");

    // Twice the capacity in finished records, oldest first, plus a handful still in
    // flight — each with an idempotency token naming it.
    let over = MAX_QUANTUM_TASKS * 2;
    let in_flight = 5;
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut state = ComAmazonawsBraketBraketState::default();
    for index in 0..over + in_flight {
        let status = if index < over {
            STATUS_FAILED
        } else {
            STATUS_CREATED
        };
        let created_at = base + Duration::from_secs(index as u64);
        let arn = arn_of(index);
        state.idempotency.insert(
            format!("token-{index}"),
            IdempotencyRecord {
                fingerprint: String::new(),
                resource_arn: arn.clone(),
                recorded_at: created_at,
            },
        );
        state.quantum_tasks.insert(
            arn.clone(),
            synthetic_task(
                &arn,
                "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
                status,
                created_at,
                created_at,
            ),
        );
    }

    state.evict_finished_tasks_over_capacity();

    assert!(
        state.quantum_tasks.len() < MAX_QUANTUM_TASKS,
        "case over capacity: expected fewer than {MAX_QUANTUM_TASKS} records, got {}",
        state.quantum_tasks.len()
    );
    assert_eq!(
        state.idempotency.len(),
        state.quantum_tasks.len(),
        "case over capacity: every surviving token must name a surviving task, got {} tokens \
         for {} tasks",
        state.idempotency.len(),
        state.quantum_tasks.len()
    );
    for index in over..over + in_flight {
        assert!(
            state.quantum_tasks.contains_key(&arn_of(index)),
            "case over capacity: an in-flight task must never be dropped, {} is gone",
            arn_of(index)
        );
    }
    // The oldest finished records went, the newest stayed.
    assert!(
        !state.quantum_tasks.contains_key(&arn_of(0)),
        "case over capacity: the oldest finished record must be dropped first"
    );
    assert!(
        state.quantum_tasks.contains_key(&arn_of(over - 1)),
        "case over capacity: the newest finished record must be kept"
    );
    // Nothing is dropped while the store is within capacity.
    let before = state.quantum_tasks.len();
    state.evict_finished_tasks_over_capacity();
    assert_eq!(
        state.quantum_tasks.len(),
        before,
        "case within capacity: expected no eviction, went from {before} to {}",
        state.quantum_tasks.len()
    );
}

/// The `clientToken` index drops exactly the entries that can no longer serve a
/// replay — one whose task is gone, one older than the TTL — and, past its capacity,
/// the oldest of what is left. A fresh token naming a stored task always survives.
#[test]
fn the_token_index_drops_only_the_entries_that_cannot_replay() {
    use service_com_amazonaws_braket_braket::state::{
        ComAmazonawsBraketBraketState, IdempotencyRecord, IDEMPOTENCY_TTL,
        MAX_IDEMPOTENCY_TOKENS, STATUS_FAILED,
    };

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let mut state = ComAmazonawsBraketBraketState::default();
    let remember = |state: &mut ComAmazonawsBraketBraketState,
                        token: &str,
                        arn: &str,
                        recorded_at: SystemTime,
                        with_task: bool| {
        if with_task {
            state.quantum_tasks.insert(
                arn.to_string(),
                synthetic_task(
                    arn,
                    "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
                    STATUS_FAILED,
                    recorded_at,
                    recorded_at,
                ),
            );
        }
        state.idempotency.insert(
            token.to_string(),
            IdempotencyRecord {
                fingerprint: "digest".to_string(),
                resource_arn: arn.to_string(),
                recorded_at,
            },
        );
    };

    // (case, token, recorded_at, task still stored, expected to survive)
    let rows: Vec<(&str, &str, SystemTime, bool, bool)> = vec![
        ("fresh token, task stored", "keep-fresh", now, true, true),
        (
            "token at the TTL boundary, task stored",
            "drop-at-ttl",
            now - IDEMPOTENCY_TTL,
            true,
            false,
        ),
        (
            "token just inside the TTL, task stored",
            "keep-inside-ttl",
            now - IDEMPOTENCY_TTL + Duration::from_secs(1),
            true,
            true,
        ),
        (
            "token well past the TTL, task stored",
            "drop-past-ttl",
            now - IDEMPOTENCY_TTL - Duration::from_secs(3_600),
            true,
            false,
        ),
        (
            "fresh token whose task was evicted",
            "drop-orphan",
            now,
            false,
            false,
        ),
    ];
    for (index, (_, token, recorded_at, with_task, _)) in rows.iter().enumerate() {
        let arn = format!("arn:aws:braket:eu-west-1:000000000000:quantum-task/ttl-{index:08}");
        remember(&mut state, token, &arn, *recorded_at, *with_task);
    }

    state.prune_idempotency(now);

    for (case, token, _, _, survives) in &rows {
        assert_eq!(
            state.idempotency.contains_key(*token),
            *survives,
            "case {case}: expected survives={survives} for {token}, index now holds {:?}",
            state.idempotency.keys().collect::<Vec<&String>>()
        );
    }

    // Past the cap, the oldest surviving entries go: every token here is fresh and
    // names a stored task, so only the cap can remove one.
    let over = MAX_IDEMPOTENCY_TOKENS + 10;
    let mut state = ComAmazonawsBraketBraketState::default();
    for index in 0..over {
        let arn = format!("arn:aws:braket:eu-west-1:000000000000:quantum-task/cap-{index:08}");
        remember(
            &mut state,
            &format!("cap-token-{index:08}"),
            &arn,
            now - Duration::from_secs((over - index) as u64),
            true,
        );
    }
    state.prune_idempotency(now);
    assert!(
        state.idempotency.len() < MAX_IDEMPOTENCY_TOKENS,
        "case over the cap: expected fewer than {MAX_IDEMPOTENCY_TOKENS} tokens, got {}",
        state.idempotency.len()
    );
    assert!(
        !state.idempotency.contains_key("cap-token-00000000"),
        "case over the cap: the oldest token must be dropped first"
    );
    assert!(
        state
            .idempotency
            .contains_key(&format!("cap-token-{:08}", over - 1)),
        "case over the cap: the newest token must be kept"
    );
}

// ---------------------------------------------------------------------------
// The wire SHAPE of the create and the reads
//
// The status code the create answers with, and the members a client parses off each
// response: the status the create reports for the task it accepted, the form a timestamp
// travels in, the tags map, and the device-parameters document the task ran under.
// ---------------------------------------------------------------------------

/// The status `CreateQuantumTask` answers with, and the status its body reports for the
/// task it just accepted.
const CREATE_STATUS: u16 = 200;
const ACCEPTED_STATUS: &str = "INITIALIZED";

/// The suffix every Braket device-parameters schema name ends in, whichever device family
/// publishes it (`braket.device_schema.simulators.gate_model_simulator_device_parameters`
/// for a simulator, `braket.device_schema.<provider>.<provider>_device_parameters` for a
/// QPU).
const PARAMETERS_SCHEMA_SUFFIX: &str = "_device_parameters";

/// An OpenQASM action document declaring `qubits` qubits, as a client sends it.
fn action_of(qubits: usize) -> String {
    let mut source = format!("OPENQASM 3.0; qubit[{qubits}] q;");
    for qubit in 0..qubits {
        source.push_str(&format!(" h q[{qubit}];"));
    }
    json!({
        "braketSchemaHeader": { "name": "braket.ir.openqasm.program", "version": "1" },
        "source": source,
    })
    .to_string()
}

/// Whether `value` is the RFC 3339 date-time STRING a `date-time` member travels as.
fn is_date_time(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|rendered| {
        ::aws_smithy_types::DateTime::from_str(
            rendered,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .is_ok()
    })
}

/// Poll `GetQuantumTask` until the task reaches a terminal status, and answer what it then
/// reports.
///
/// The terminal transition is owed by the clock, so this waits for the CONDITION rather
/// than for a span; a task that never gets there fails the suite instead of hanging it.
async fn poll_until_terminal(braket: &Braket, arn: &str) -> Value {
    for _ in 0..200 {
        let reply = braket.get_task(arn).await;
        assert_eq!(
            reply.status, 200,
            "case poll {arn}: expected 200, got {} {}",
            reply.status, reply.body
        );
        if matches!(
            member(&reply.body, "status"),
            "COMPLETED" | "FAILED" | "CANCELLED"
        ) {
            return reply.body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case poll {arn}: the task never reached a terminal status");
}

/// RULE: the create answers HTTP 200, and its body carries the task's ARN together with the
/// status of the task the call just accepted. The rows differ along what the request
/// carries — the device, the shot count, the tag set, a parameters document — none of which
/// changes the answer's shape.
#[tokio::test]
async fn the_create_answers_200_with_the_accepted_tasks_status() {
    let braket = Braket::new();
    let simulator = braket.an_online_device().await;
    let qpu = crate::spending_limits::a_device(&braket, "QPU", "ONLINE").await;

    // (case, device, shots, tags, deviceParameters)
    let rows: Vec<(&str, &String, i64, Value, Option<&str>)> = vec![
        ("the smallest request", &simulator, 1, json!(null), None),
        (
            "a request carrying tags",
            &simulator,
            10,
            json!({ "team": "quantum" }),
            None,
        ),
        (
            "a request carrying its own device parameters",
            &simulator,
            100,
            json!(null),
            Some("{\"braketSchemaHeader\": {\"name\": \"x\", \"version\": \"1\"}}"),
        ),
        ("a quantum processor", &qpu, 1, json!(null), None),
    ];

    for (index, (case, device, shots, tags, parameters)) in rows.iter().enumerate() {
        let mut body = create_body(&format!("wire-shape-{index}"), device, *shots);
        if tags.is_object() {
            body["tags"] = tags.clone();
        }
        if let Some(document) = parameters {
            body["deviceParameters"] = json!(document);
        }
        let created = braket.create_task(body).await;
        assert_eq!(
            created.status, CREATE_STATUS,
            "case {case:?}: expected HTTP {CREATE_STATUS}, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        assert!(
            arn.contains(":quantum-task/"),
            "case {case:?}: expected a quantum-task ARN, got {arn}"
        );
        assert_eq!(
            created.body.get("status").and_then(Value::as_str),
            Some(ACCEPTED_STATUS),
            "case {case:?}: expected the accepted task's status, got {}",
            created.body
        );
        // The status a READ reports is the stored one, which the clock moves.
        let read = braket.get_task(&arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created task reads back, got {} {}",
            read.status, read.body
        );
        assert!(
            matches!(
                member(&read.body, "status"),
                "CREATED" | "QUEUED" | "RUNNING" | "COMPLETED" | "FAILED"
            ),
            "case {case:?}: expected a modelled QuantumTaskStatus, got {}",
            read.body
        );
    }
}

/// This crate's Smithy model, as the file the service is generated from.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-braket-braket.json"
));

/// Every timestamp member this model declares, as `(member id, the format it declares or
/// `None`)` — derived from the model file itself, so the catalog the service carries is
/// checked against the model rather than against a second hand-written list.
fn modelled_timestamp_members() -> Vec<(String, Option<String>)> {
    let model: Value = serde_json::from_str(MODEL).expect("the crate's Smithy model parses");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model declares shapes")
        .clone();
    let mut members = Vec::new();
    for (shape_id, shape) in &shapes {
        let Some(declared) = shape.get("members").and_then(Value::as_object) else {
            continue;
        };
        for (name, definition) in declared {
            let target = definition["target"].as_str().unwrap_or_default();
            let targets_timestamp = target == "smithy.api#Timestamp"
                || shapes
                    .get(target)
                    .is_some_and(|target| target["type"] == "timestamp");
            if !targets_timestamp {
                continue;
            }
            let format = definition
                .get("traits")
                .and_then(|traits| traits.get("smithy.api#timestampFormat"))
                .and_then(Value::as_str)
                .map(str::to_string);
            members.push((format!("{shape_id}${name}"), format));
        }
    }
    members
}

/// RULE: the service carries exactly the timestamp members its model declares the
/// `date-time` form on, and carries none of the members it declares another form on — the
/// catalog the protocol seam supplies that form from
/// (`service_com_amazonaws_braket_braket::service::DATE_TIME_MEMBERS`).
///
/// The table is the model's whole timestamp vocabulary, so a member the next model update
/// adds, removes or re-declares fails here rather than reaching the wire in the wrong form.
#[test]
fn the_service_carries_exactly_the_date_time_members_its_model_declares() {
    use std::collections::BTreeSet;

    use service_com_amazonaws_braket_braket::service::DATE_TIME_MEMBERS;

    let members = modelled_timestamp_members();
    assert!(
        members.len() > DATE_TIME_MEMBERS.len(),
        "case the model: this model declares both timestamp forms, so it must carry more \
         timestamp members than the date-time ones, found {} in all",
        members.len()
    );

    let declared_date_time: BTreeSet<&str> = members
        .iter()
        .filter(|(_, format)| format.as_deref() == Some("date-time"))
        .map(|(id, _)| id.as_str())
        .collect();
    let carried: BTreeSet<&str> = DATE_TIME_MEMBERS.iter().copied().collect();
    assert_eq!(
        carried, declared_date_time,
        "case date-time members: the service must carry exactly the members the model \
         declares date-time on; it carries {carried:?} and the model declares \
         {declared_date_time:?}"
    );

    let other_form: BTreeSet<&str> = members
        .iter()
        .filter(|(_, format)| format.as_deref() != Some("date-time"))
        .map(|(id, _)| id.as_str())
        .collect();
    assert!(
        !other_form.is_empty(),
        "case the other members: this model declares epoch-seconds timestamp members too, \
         so the catalog must be selective"
    );
    let overlap: Vec<&&str> = other_form.intersection(&carried).collect();
    assert!(
        overlap.is_empty(),
        "case the other members: a member the model does not declare date-time on must not \
         be carried, got {overlap:?}"
    );
}

/// RULE: a timestamp member travels in the wire form its model declares — the `date-time`
/// ones as RFC 3339 strings, the two `epoch-seconds` ones as numbers. The rows span the
/// read paths of all three resources this service reports a timestamp for.
#[tokio::test]
async fn a_timestamp_travels_in_the_form_its_model_declares() {
    use crate::spending_limits::{a_device, create_limit, limit_body, limits_on, now_epoch, period};

    let braket = Braket::new();
    let simulator = braket.an_online_device().await;
    let qpu = a_device(&braket, "QPU", "ONLINE").await;

    // A quantum task, read through its own door and through the search.
    let created = braket
        .create_task(create_body("wire-time-task", &simulator, 1))
        .await;
    assert_eq!(
        created.status, CREATE_STATUS,
        "creating the task to read: got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");
    let terminal = poll_until_terminal(&braket, &arn).await;
    let searched = braket.search_tasks(json!({ "filters": [] })).await;
    let summary = searched
        .body
        .get("quantumTasks")
        .and_then(Value::as_array)
        .and_then(|tasks| tasks.first())
        .cloned()
        .unwrap_or_else(|| panic!("SearchQuantumTasks reports no task: {}", searched.body));

    // A hybrid job, read through its own door.
    let job = braket
        .create_job(crate::hybrid_job_loop::job_body(
            "wire-time-job",
            "wire-time-job",
            &simulator,
        ))
        .await;
    assert_eq!(
        job.status, 201,
        "creating the job to read: got {} {}",
        job.status, job.body
    );
    let job_read = braket.get_job(&job.str_member("jobArn")).await;

    // A spending limit, read through the search, with the period it was created with.
    let start = now_epoch();
    let mut limit = limit_body("wire-time-limit", &qpu, "500");
    limit["timePeriod"] = period(start, start + 3_600);
    let minted = create_limit(&braket, limit).await;
    assert!(
        minted.status < 300,
        "creating the limit to read: got {} {}",
        minted.status,
        minted.body
    );
    let limit = limits_on(&braket, &qpu)
        .await
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("SearchSpendingLimits reports no limit on {qpu}"));

    // Every member the model declares `date-time` on, across the three resources.
    let date_time_members: Vec<(&str, Option<&Value>)> = vec![
        ("GetQuantumTask createdAt", terminal.get("createdAt")),
        ("GetQuantumTask endedAt", terminal.get("endedAt")),
        ("QuantumTaskSummary createdAt", summary.get("createdAt")),
        ("QuantumTaskSummary endedAt", summary.get("endedAt")),
        ("GetJob createdAt", job_read.body.get("createdAt")),
        ("SpendingLimitSummary createdAt", limit.get("createdAt")),
        ("SpendingLimitSummary updatedAt", limit.get("updatedAt")),
    ];
    for (case, value) in date_time_members {
        assert!(
            is_date_time(value),
            "case {case}: expected an RFC 3339 date-time string, got {value:?}"
        );
    }

    // And the two the model declares `epoch-seconds` on, which stay numbers — and the
    // numbers they were created with.
    for (case, name, expected) in [
        ("TimePeriod startAt", "startAt", start),
        ("TimePeriod endAt", "endAt", start + 3_600),
    ] {
        let value = limit.get("timePeriod").and_then(|period| period.get(name));
        assert_eq!(
            value.and_then(Value::as_i64),
            Some(expected),
            "case {case}: expected the epoch second {expected}, got {value:?}"
        );
    }
}

/// How many fractional-second digits an instant a reader of this API can HOLD carries: six,
/// a whole microsecond, which is the resolution of the date-time value a generated client
/// parses a `date-time` member into (a Python `datetime`, a Java `Instant` rendered back
/// through one, the `DateTime` these suites parse with). A rendering finer than that denotes
/// an instant no reader of it can repeat.
const MICROSECOND_DIGITS: usize = 6;

/// The fractional-second digits of one rendered `date-time`, as it was written: what stands
/// between the `.` and the zone, and the empty string for a rendering that carries no
/// fraction at all.
fn fraction_digits(rendered: &str) -> &str {
    match rendered.split_once('.') {
        Some((_, fraction)) => fraction.trim_end_matches(|c: char| !c.is_ascii_digit()),
        None => "",
    }
}

/// One rendered `date-time` as a typed client holds it and sends it back: the same instant
/// with its fraction cut to the whole microsecond such a client carries
/// ([`MICROSECOND_DIGITS`]). A rendering already at that resolution comes back unchanged,
/// which is the point of the rule below.
fn as_a_client_holds_it(rendered: &str) -> String {
    let digits = fraction_digits(rendered);
    if digits.len() <= MICROSECOND_DIGITS {
        return rendered.to_string();
    }
    let (whole, _) = rendered
        .split_once('.')
        .expect("a rendering with a fraction carries the point it was found by");
    format!("{whole}.{}Z", &digits[..MICROSECOND_DIGITS])
}

/// RULE: an instant this service records is ONE value — every path that reports it renders
/// the same `date-time`, and that rendering is at a resolution its readers hold (a whole
/// microsecond), so the instant a client parses out of one read is the instant the service
/// holds and the value it can send back.
///
/// The rows span every resource this service records an instant for and every member it
/// reports one in, with the tasks differing along the shot count and the device so no two
/// rows share an instant.
#[tokio::test]
async fn a_recorded_instant_is_served_at_the_resolution_its_readers_hold() {
    use crate::spending_limits::{a_device, create_limit, limit_body, limits_on, now_epoch, period};

    let braket = Braket::new();
    let simulator = braket.an_online_device().await;
    let qpu = a_device(&braket, "QPU", "ONLINE").await;

    // (case, device, shots) — three tasks, so three unrelated instants are rendered.
    let task_rows: Vec<(&str, &String, i64)> = vec![
        ("a one-shot task on a simulator", &simulator, 1),
        ("a many-shot task on a simulator", &simulator, 100),
        ("a task on a quantum processor", &qpu, 10),
    ];

    let mut rendered: Vec<(String, String)> = Vec::new();
    for (index, (case, device, shots)) in task_rows.iter().enumerate() {
        let created = braket
            .create_task(create_body(
                &format!("instant-resolution-{index}"),
                device,
                *shots,
            ))
            .await;
        assert_eq!(
            created.status, CREATE_STATUS,
            "case {case:?}: creating the task, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        let terminal = poll_until_terminal(&braket, &arn).await;
        let searched = braket
            .search_tasks(json!({
                "filters": [{ "name": "quantumTaskArn", "operator": "EQUAL", "values": [arn] }],
            }))
            .await;
        let summary = searched
            .body
            .get("quantumTasks")
            .and_then(Value::as_array)
            .and_then(|tasks| tasks.first())
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the search reports no task: {}", searched.body)
            });
        for (member_name, read, searched) in [
            ("createdAt", terminal.get("createdAt"), summary.get("createdAt")),
            ("endedAt", terminal.get("endedAt"), summary.get("endedAt")),
        ] {
            let read = read.and_then(Value::as_str).unwrap_or_else(|| {
                panic!("case {case:?}: GetQuantumTask reports no {member_name}: {terminal}")
            });
            rendered.push((format!("{case} {member_name}"), read.to_string()));
            // The two read paths report ONE instant, so they render one text.
            assert_eq!(
                searched.and_then(Value::as_str),
                Some(read),
                "case {case:?}: expected the search to report the {member_name} \
                 {read:?} the read reports, got {searched:?}"
            );
        }
    }

    // A hybrid job, and a spending limit, which records two instants.
    let job = braket
        .create_job(crate::hybrid_job_loop::job_body(
            "instant-resolution-job",
            "instant-resolution-job",
            &simulator,
        ))
        .await;
    assert_eq!(
        job.status, 201,
        "creating the job to read: got {} {}",
        job.status, job.body
    );
    let job_read = braket.get_job(&job.str_member("jobArn")).await;
    rendered.push((
        "a hybrid job createdAt".to_string(),
        job_read
            .body
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("GetJob reports no createdAt: {}", job_read.body))
            .to_string(),
    ));

    let start = now_epoch();
    let mut body = limit_body("instant-resolution-limit", &qpu, "5.00");
    body["timePeriod"] = period(start, start + 3_600);
    let minted = create_limit(&braket, body).await;
    assert!(
        minted.status < 300,
        "creating the limit to read: got {} {}",
        minted.status,
        minted.body
    );
    let limit = limits_on(&braket, &qpu)
        .await
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("SearchSpendingLimits reports no limit on {qpu}"));
    for member_name in ["createdAt", "updatedAt"] {
        rendered.push((
            format!("a spending limit {member_name}"),
            limit
                .get(member_name)
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("SearchSpendingLimits reports no {member_name}: {limit}")
                })
                .to_string(),
        ));
    }

    for (case, instant) in &rendered {
        let digits = fraction_digits(instant);
        assert!(
            digits.len() <= MICROSECOND_DIGITS,
            "case {case:?}: expected an instant at a resolution its readers hold \
             ({MICROSECOND_DIGITS} fractional digits at most), got {instant:?} with \
             {} of them",
            digits.len()
        );
        // The rendering a client holds is therefore the rendering the service served, so
        // the instant travels back unchanged.
        assert_eq!(
            as_a_client_holds_it(instant),
            *instant,
            "case {case:?}: expected the instant a client holds to be the one served, \
             got {:?}",
            as_a_client_holds_it(instant)
        );
    }
}

/// RULE: the instant a read reports is the instant the service compares a filter against, so
/// a `createdAt` a client read back — held at the resolution that client's own date-time type
/// carries — selects the very task it came from. The rows differ along the shot count and the
/// device, so each row's filter bound is its own instant.
#[tokio::test]
async fn a_created_at_read_back_selects_the_task_it_came_from() {
    let braket = Braket::new();
    let simulator = braket.an_online_device().await;
    let qpu = crate::spending_limits::a_device(&braket, "QPU", "ONLINE").await;

    // (case, device, shots)
    let rows: Vec<(&str, &String, i64)> = vec![
        ("a one-shot task on a simulator", &simulator, 1),
        ("a many-shot task on a simulator", &simulator, 50),
        ("a task on a quantum processor", &qpu, 10),
    ];

    for (index, (case, device, shots)) in rows.iter().enumerate() {
        let created = braket
            .create_task(create_body(
                &format!("instant-round-trip-{index}"),
                device,
                *shots,
            ))
            .await;
        assert_eq!(
            created.status, CREATE_STATUS,
            "case {case:?}: creating the task, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        let read = braket.get_task(&arn).await;
        let reported = read
            .body
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no createdAt in {}", read.body))
            .to_string();
        // What a typed client holds after parsing that value, and so what it sends back.
        let held = as_a_client_holds_it(&reported);
        let searched = braket
            .search_tasks(json!({
                "filters": [{ "name": "createdAt", "operator": "EQUAL", "values": [held] }],
            }))
            .await;
        assert_eq!(
            searched.status, 200,
            "case {case:?}: filtering on the createdAt read back, got {} {}",
            searched.status, searched.body
        );
        let selected: Vec<&str> = searched
            .body
            .get("quantumTasks")
            .and_then(Value::as_array)
            .map(|tasks| tasks.iter().map(|task| member(task, "quantumTaskArn")).collect())
            .unwrap_or_default();
        assert!(
            selected.contains(&arn.as_str()),
            "case {case:?}: expected the createdAt {held:?} read back to select {arn}, \
             got {selected:?}"
        );
    }
}

/// RULE: a quantum task's `tags` are reported as a map, empty when the task carries none.
/// The rows span the tag sets a create may carry, and both read paths report the same set.
#[tokio::test]
async fn a_tasks_tags_are_reported_as_a_map_even_when_it_carries_none() {
    let braket = Braket::new();
    let device = braket.an_online_device().await;

    // (case, the tags the create carries, the map the reads must report)
    let rows: Vec<(&str, Value, Value)> = vec![
        ("no tags at all", json!(null), json!({})),
        ("an empty map", json!({}), json!({})),
        (
            "one tag",
            json!({ "team": "quantum" }),
            json!({ "team": "quantum" }),
        ),
        (
            "several tags, one with an empty value",
            json!({ "team": "quantum", "note": "" }),
            json!({ "team": "quantum", "note": "" }),
        ),
    ];

    for (index, (case, submitted, expected)) in rows.iter().enumerate() {
        let mut body = create_body(&format!("wire-tags-{index}"), &device, 1);
        if submitted.is_object() {
            body["tags"] = submitted.clone();
        }
        let created = braket.create_task(body).await;
        assert_eq!(
            created.status, CREATE_STATUS,
            "case {case:?}: creating the task, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        let read = braket.get_task(&arn).await;
        assert_eq!(
            read.body.get("tags"),
            Some(expected),
            "case {case:?}: expected the tags {expected}, got {:?}",
            read.body.get("tags")
        );
        let searched = braket
            .search_tasks(json!({
                "filters": [{
                    "name": "quantumTaskArn",
                    "operator": "EQUAL",
                    "values": [arn],
                }],
            }))
            .await;
        let summary = searched
            .body
            .get("quantumTasks")
            .and_then(Value::as_array)
            .and_then(|tasks| tasks.first())
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the search reports no task: {}", searched.body)
            });
        assert_eq!(
            summary.get("tags"),
            Some(expected),
            "case {case:?}: the summary reports the same set, got {:?}",
            summary.get("tags")
        );
    }
}

/// RULE: `deviceParameters` is the document the task ran under — the submitted document as
/// it was sent, or the device's own default parameters document for the program. The rows
/// differ along the device family that decides the schema name and the program that decides
/// the qubit count.
#[tokio::test]
async fn device_parameters_report_what_the_task_ran_under() {
    let braket = Braket::new();
    let simulator = braket.an_online_device().await;
    let qpu = crate::spending_limits::a_device(&braket, "QPU", "ONLINE").await;

    // (case, device, qubits the program declares, the document the create carries)
    let rows: Vec<(&str, &String, usize, Option<&str>)> = vec![
        ("a simulator and a two-qubit program", &simulator, 2, None),
        ("a simulator and a three-qubit program", &simulator, 3, None),
        ("a quantum processor", &qpu, 2, None),
        (
            "a document the create carried",
            &simulator,
            2,
            Some(
                "{\"braketSchemaHeader\": {\"name\": \
                 \"braket.device_schema.simulators.gate_model_simulator_device_parameters\", \
                 \"version\": \"1\"}, \"paradigmParameters\": {\"qubitCount\": 9}}",
            ),
        ),
        (
            "an empty document the create carried",
            &simulator,
            2,
            Some("{}"),
        ),
    ];

    for (index, (case, device, qubits, submitted)) in rows.iter().enumerate() {
        let mut body = create_body(&format!("wire-params-{index}"), device, 1);
        body["action"] = json!(action_of(*qubits));
        if let Some(document) = submitted {
            body["deviceParameters"] = json!(document);
        }
        let created = braket.create_task(body).await;
        assert_eq!(
            created.status, CREATE_STATUS,
            "case {case:?}: creating the task, got {} {}",
            created.status, created.body
        );
        let read = braket.get_task(&created.str_member("quantumTaskArn")).await;
        let reported = member(&read.body, "deviceParameters");
        match submitted {
            Some(document) => assert_eq!(
                reported, *document,
                "case {case:?}: a submitted document is reported as it was sent"
            ),
            None => {
                let parsed: Value = serde_json::from_str(reported)
                    .unwrap_or_else(|e| panic!("case {case:?}: {reported} is not JSON: {e}"));
                let schema = member(&parsed["braketSchemaHeader"], "name");
                assert!(
                    schema.ends_with(PARAMETERS_SCHEMA_SUFFIX)
                        && schema.starts_with("braket.device_schema."),
                    "case {case:?}: expected the device's parameters schema, got {parsed}"
                );
                assert_eq!(
                    parsed["paradigmParameters"]["qubitCount"].as_u64(),
                    Some(*qubits as u64),
                    "case {case:?}: expected the program's {qubits} qubits, got {parsed}"
                );
                assert_eq!(
                    parsed["paradigmParameters"]["disableQubitRewiring"].as_bool(),
                    Some(false),
                    "case {case:?}: expected rewiring to stay enabled, got {parsed}"
                );
            }
        }
    }
}
