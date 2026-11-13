// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the CloudWatch Synthetics canary lifecycle slice.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, so what is exercised is the HTTP behaviour (routing, `restJson1` framing,
//! status codes and error shapes) and not just the handler functions.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// Comfortably longer than the provisioning span the emulator owes, so a canary created
/// at the start of a step is `READY` by the next one.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// Shorter than the span after which the emulator owes a started run its outcome, so an
/// assertion about an OPEN run is only made when the read provably happened inside it.
const RUN_STILL_OPEN_WITHIN: Duration = Duration::from_millis(250);

/// The store's own bounds, mirrored here so the tests assert the documented numbers.
/// CloudWatch Synthetics' default quota is 100 canaries per Region.
const MAX_CANARIES: usize = 100;

/// The emulator-chosen cap on retained runs per canary.
const MAX_RETAINED_RUNS_PER_CANARY: usize = 100;

/// One answer from the emulator: enough to assert on a success or a modelled failure.
///
/// `pub(crate)` because the sibling suites in this test target share this harness rather
/// than each growing their own.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The `restJson1` error shape, from `x-amzn-errortype`.
    pub(crate) error_code: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    /// The JSON at a dotted path, or `Value::Null`.
    pub(crate) fn at(&self, path: &str) -> &Value {
        let mut cursor = &self.body;
        for segment in path.split('.') {
            cursor = &cursor[segment];
        }
        cursor
    }

    pub(crate) fn string_at(&self, path: &str) -> String {
        self.at(path)
            .as_str()
            .unwrap_or_else(|| panic!("expected a string at {path}, got {:?}", self.at(path)))
            .to_string()
    }
}

/// A single emulated synthetics endpoint in one Region.
pub(crate) struct Synthetics {
    router: MultiServiceRouter,
    region: String,
}

impl Synthetics {
    pub(crate) fn new(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_synthetics_synthetics::EmulatedService::new(engine),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// This service mounted beside the OWNERS of the resources a canary RUN uses — IAM,
    /// which holds the execution role whose policies decide whether a run may write its
    /// artifacts, and Amazon S3, which holds the bucket they are written to — on ONE shared
    /// engine, plus an SDK client for each over the router's own in-process callback: the
    /// very seam a run reaches them through.
    ///
    /// Nothing in a suite using this reaches into either store: a role exists only because
    /// IAM answered for it, and an artifact only because Amazon S3 serves it back.
    pub(crate) fn with_account_services(
        region: &str,
    ) -> (Self, ::aws_sdk_iam::Client, ::aws_sdk_s3::Client) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_synthetics_synthetics::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        // The clients are the tree's OWN constructions of these two hops
        // (`service_common::iam_client` / `s3_client`), which is the very seam a canary run
        // reaches IAM and Amazon S3 through — so the harness drives the services exactly as
        // the handler does rather than carrying a second copy of that configuration.
        let iam = ::service_common::iam_client::iam_client_through(&callback, region);
        let s3 = ::service_common::s3_client::s3_client(&callback, region);
        (
            Self {
                router,
                region: region.to_string(),
            },
            iam,
            s3,
        )
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        self.call_in_region(&self.region, method, path, body).await
    }

    /// [`Synthetics::call`], but signed for `region` rather than this endpoint's own, so
    /// one store can be addressed from two Regions — which is what the global-resource
    /// and home-Region rules of the group operations are asserted over.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("the test body serialises"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://synthetics.{region}.amazonaws.com{path}"
            ))
            // The SigV4 credential scope is what routes the request to this service and
            // names the Region its resources live in.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/synthetics/aws4_request, \
                     SignedHeaders=host, Signature=0",
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_code,
            body,
        }
    }

    pub(crate) async fn create(&self, body: Value) -> Answer {
        self.call("POST", "/canary", Some(body)).await
    }

    pub(crate) async fn get(&self, name: &str) -> Answer {
        self.call("GET", &format!("/canary/{name}"), None).await
    }

    pub(crate) async fn start(&self, name: &str) -> Answer {
        self.call("POST", &format!("/canary/{name}/start"), Some(json!({})))
            .await
    }

    pub(crate) async fn stop(&self, name: &str) -> Answer {
        self.call("POST", &format!("/canary/{name}/stop"), Some(json!({})))
            .await
    }

    pub(crate) async fn delete(&self, name: &str) -> Answer {
        self.call("DELETE", &format!("/canary/{name}?deleteLambda=true"), None)
            .await
    }

    pub(crate) async fn describe(&self, body: Value) -> Answer {
        self.call("POST", "/canaries", Some(body)).await
    }

    pub(crate) async fn runs(&self, name: &str, body: Value) -> Answer {
        self.call("POST", &format!("/canary/{name}/runs"), Some(body))
            .await
    }
}

/// A minimal, valid `CreateCanary` body for `name`.
pub(crate) fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Code": { "Handler": "index.handler", "S3Bucket": "scripts", "S3Key": "canary.zip" },
        "ArtifactS3Location": "s3://artifacts/canary",
        "ExecutionRoleArn": "arn:aws:iam::123456789012:role/CanaryRole",
        "Schedule": { "Expression": "rate(5 minutes)" },
        "RuntimeVersion": "syn-nodejs-puppeteer-9.0",
    })
}

/// How long a poll waits for a transition the clock owes before giving up. Deliberately far
/// longer than the spans this emulator owes (half a second each), so a loaded machine waits
/// for the CONDITION rather than failing on a budget.
pub(crate) const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll pauses between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Look again until `settled` holds of the answer, or [`POLL_DEADLINE`] passes.
///
/// The ONE wait this target makes on a transition the clock owes: a transition is waited OUT
/// rather than slept for, so the loop asks for the condition and stops as soon as it holds
/// and no suite depends on how fast the host runs.
pub(crate) async fn poll_until<Call, Fut, Settled>(
    case: &str,
    mut call: Call,
    settled: Settled,
) -> Answer
where
    Call: FnMut() -> Fut,
    Fut: std::future::Future<Output = Answer>,
    Settled: Fn(&Answer) -> bool,
{
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let answer = call().await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 while polling, got {} ({:?})",
            answer.status, answer.body
        );
        if settled(&answer) {
            return answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: the condition never held, last answer {:?}",
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `@pattern` the model declares for `shape`, read out of the model this crate ships.
///
/// `pub(crate)` for the same reason as the harness above: every suite that asserts on a
/// constraint refusal needs the model's own expression, and reading it here is what keeps a
/// suite from writing a re-spelled copy of it. What a refusal QUOTES is this expression put
/// through `service_common::validation::unanchored`, the tree's one rendering of that rule,
/// which each suite calls directly.
pub(crate) fn declared_pattern(shape: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-synthetics-synthetics.json"
    );
    let text = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
    let model: Value = serde_json::from_str(&text).expect("the Smithy model is JSON");
    let id = format!("com.amazonaws.synthetics#{shape}");
    model["shapes"][&id]["traits"]["smithy.api#pattern"]
        .as_str()
        .unwrap_or_else(|| panic!("shape {shape} declares no @pattern in the shipped model"))
        .to_string()
}

/// The whole `Message` a refusal with ONE constraint violation carries: the envelope AWS
/// wraps a single clause in, over the echoed value, the member path and the requirement.
///
/// `pub(crate)` for the same reason as the harness above — every suite that asserts on a
/// constraint refusal states this one sentence.
pub(crate) fn one_violation(value: &str, member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement clause a `@pattern` violation states, over the expression the refusing
/// operation's dialect quotes.
///
/// `pub(crate)` beside [`one_violation`] and [`declared_pattern`] for the same reason: the
/// suites that assert on a pattern refusal state this one clause, and which EXPRESSION goes
/// in it is the thing they differ on.
pub(crate) fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The whole slice over one canary: every response is read back out of the store, and the
/// record a create wrote is the record every later read reports.
#[tokio::test]
async fn a_canary_round_trips_through_create_get_start_stop_and_delete() {
    let aws = Synthetics::new("eu-west-2");
    let name = "my-canary";

    let mut body = create_body(name);
    body["Tags"] = json!({ "env": "test" });
    body["SuccessRetentionPeriodInDays"] = json!(7);
    let created = aws.create(body).await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200, got {} ({:?})",
        created.status, created.body
    );

    let id = created.string_at("Canary.Id");
    // `Canary$Id` is modelled as a UUID (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
    assert!(
        is_uuid(&id),
        "case create: expected Canary.Id to be a UUID, got {id:?}"
    );
    assert_eq!(
        created.string_at("Canary.Status.State"),
        "CREATING",
        "case create: expected a freshly created canary to still be provisioning, got {:?}",
        created.at("Canary.Status")
    );
    // Resource ARNs carry the Region the request was signed for.
    let engine_arn = created.string_at("Canary.EngineArn");
    assert!(
        engine_arn.starts_with("arn:aws:lambda:eu-west-2:")
            && engine_arn.ends_with(&format!("function:cwsyn-{name}-{id}")),
        "case create: expected EngineArn to name the signed Region and the canary, got {engine_arn:?}"
    );
    let source_arn = created.string_at("Canary.Code.SourceLocationArn");
    assert!(
        source_arn.starts_with("arn:aws:lambda:eu-west-2:"),
        "case create: expected SourceLocationArn in the signed Region, got {source_arn:?}"
    );

    // The provisioning transition is owed by the clock: a caller acting immediately still
    // sees CREATING, and the conflict errors that state implies.
    let too_early = aws.start(name).await;
    assert_eq!(
        too_early.status, 409,
        "case start-while-creating: expected 409, got {} ({:?})",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code.as_deref(),
        Some("ConflictException"),
        "case start-while-creating: expected ConflictException, got {:?}",
        too_early.error_code
    );

    tokio::time::sleep(PAST_PROVISIONING).await;

    let read = aws.get(name).await;
    assert_eq!(
        read.status, 200,
        "case get: expected 200, got {} ({:?})",
        read.status, read.body
    );
    // The SAME record, not a fresh one built from the request.
    assert_eq!(
        read.string_at("Canary.Id"),
        id,
        "case get: expected the stored Id, got {:?}",
        read.at("Canary.Id")
    );
    for field in [
        "Canary.Name",
        "Canary.Code.Handler",
        "Canary.Schedule.Expression",
        "Canary.RuntimeVersion",
        "Canary.ArtifactS3Location",
        "Canary.ExecutionRoleArn",
        "Canary.EngineArn",
        "Canary.Tags",
        "Canary.Timeline.Created",
        "Canary.Timeline.LastModified",
        "Canary.SuccessRetentionPeriodInDays",
        "Canary.FailureRetentionPeriodInDays",
    ] {
        assert_eq!(
            read.at(field),
            created.at(field),
            "case get: expected {field} to match what CreateCanary stored, got {:?} vs {:?}",
            read.at(field),
            created.at(field)
        );
    }
    assert_eq!(
        read.at("Canary.Tags"),
        &json!({ "env": "test" }),
        "case get: expected the stored tags, got {:?}",
        read.at("Canary.Tags")
    );
    assert_eq!(
        read.at("Canary.SuccessRetentionPeriodInDays"),
        &json!(7),
        "case get: expected the supplied retention period, got {:?}",
        read.at("Canary.SuccessRetentionPeriodInDays")
    );
    // The omitted retention member takes its documented default of 31 days.
    assert_eq!(
        read.at("Canary.FailureRetentionPeriodInDays"),
        &json!(31),
        "case get: expected the documented default of 31 days, got {:?}",
        read.at("Canary.FailureRetentionPeriodInDays")
    );
    assert_eq!(
        read.string_at("Canary.Status.State"),
        "READY",
        "case get-after-provisioning: expected READY, got {:?}",
        read.at("Canary.Status")
    );
    assert!(
        read.at("Canary.Timeline.LastStarted").is_null(),
        "case get-after-provisioning: a canary never started has no LastStarted, got {:?}",
        read.at("Canary.Timeline.LastStarted")
    );

    // Stopping a canary that is not running conflicts.
    let stop_idle = aws.stop(name).await;
    assert_eq!(
        stop_idle.status, 409,
        "case stop-while-ready: expected 409, got {} ({:?})",
        stop_idle.status, stop_idle.body
    );

    let started = aws.start(name).await;
    assert_eq!(
        started.status, 200,
        "case start: expected 200, got {} ({:?})",
        started.status, started.body
    );
    let running = aws.get(name).await;
    assert_eq!(
        running.string_at("Canary.Status.State"),
        "RUNNING",
        "case get-after-start: expected RUNNING, got {:?}",
        running.at("Canary.Status")
    );
    assert!(
        running.at("Canary.Timeline.LastStarted").is_number(),
        "case get-after-start: expected LastStarted to be set, got {:?}",
        running.at("Canary.Timeline.LastStarted")
    );
    // Starting observably changed the record, so its modification instant moved with it.
    assert!(
        running.at("Canary.Timeline.LastModified").as_f64()
            >= created.at("Canary.Timeline.LastModified").as_f64(),
        "case get-after-start: expected LastModified to have advanced past {:?}, got {:?}",
        created.at("Canary.Timeline.LastModified"),
        running.at("Canary.Timeline.LastModified")
    );

    // Starting an already-running canary conflicts, and deleting one does too.
    let restart = aws.start(name).await;
    assert_eq!(
        restart.status, 409,
        "case start-twice: expected 409, got {} ({:?})",
        restart.status, restart.body
    );
    let delete_running = aws.delete(name).await;
    assert_eq!(
        delete_running.status, 409,
        "case delete-while-running: expected 409, got {} ({:?})",
        delete_running.status, delete_running.body
    );

    let stopped = aws.stop(name).await;
    assert_eq!(
        stopped.status, 200,
        "case stop: expected 200, got {} ({:?})",
        stopped.status, stopped.body
    );
    let idle = aws.get(name).await;
    assert_eq!(
        idle.string_at("Canary.Status.State"),
        "STOPPED",
        "case get-after-stop: expected STOPPED, got {:?}",
        idle.at("Canary.Status")
    );
    assert!(
        idle.at("Canary.Timeline.LastStopped").is_number(),
        "case get-after-stop: expected LastStopped to be set, got {:?}",
        idle.at("Canary.Timeline.LastStopped")
    );

    let deleted = aws.delete(name).await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected 200, got {} ({:?})",
        deleted.status, deleted.body
    );
    for (case, answer) in [
        ("get-after-delete", aws.get(name).await),
        ("delete-twice", aws.delete(name).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error_code
        );
    }
    let listed = aws.describe(json!({})).await;
    assert_eq!(
        listed.at("Canaries").as_array().map(Vec::len),
        Some(0),
        "case describe-after-delete: expected no canaries, got {:?}",
        listed.at("Canaries")
    );
}

/// A name already taken is refused, and the canary that holds it is left alone.
#[tokio::test]
async fn a_duplicate_name_is_refused_without_overwriting_the_canary() {
    let aws = Synthetics::new("us-east-1");
    let first = aws.create(create_body("dupe")).await;
    assert_eq!(
        first.status, 200,
        "case first-create: expected 200, got {} ({:?})",
        first.status, first.body
    );
    let id = first.string_at("Canary.Id");

    let mut second_body = create_body("dupe");
    second_body["RuntimeVersion"] = json!("syn-python-selenium-4.0");
    let second = aws.create(second_body).await;
    assert_eq!(
        second.status, 400,
        "case duplicate-create: expected 400, got {} ({:?})",
        second.status, second.body
    );
    assert_eq!(
        second.error_code.as_deref(),
        Some("ValidationException"),
        "case duplicate-create: expected ValidationException, got {:?}",
        second.error_code
    );

    tokio::time::sleep(PAST_PROVISIONING).await;
    let read = aws.get("dupe").await;
    assert_eq!(
        read.string_at("Canary.Id"),
        id,
        "case duplicate-create: expected the original record to survive, got {:?}",
        read.at("Canary.Id")
    );
    assert_eq!(
        read.string_at("Canary.RuntimeVersion"),
        "syn-nodejs-puppeteer-9.0",
        "case duplicate-create: expected the original runtime to survive, got {:?}",
        read.at("Canary.RuntimeVersion")
    );
}

/// `DescribeCanaries` reports live state, in a stable order, and its token resumes exactly
/// where the previous page stopped — over a range of page sizes, including the bounds.
#[tokio::test]
async fn describe_canaries_pages_over_live_state() {
    let aws = Synthetics::new("us-west-2");
    // Deliberately created out of order: the reported order must be the store's, not the
    // creation order.
    let names = ["delta", "alpha", "echo", "charlie", "bravo", "foxtrot"];
    for name in names {
        let created = aws.create(create_body(name)).await;
        assert_eq!(
            created.status, 200,
            "case seed {name}: expected 200, got {} ({:?})",
            created.status, created.body
        );
    }
    let mut sorted: Vec<&str> = names.to_vec();
    sorted.sort_unstable();

    // `MaxResults` is modelled `range 1..20`: both published bounds, a value in between,
    // and the omitted case which takes the documented default of 20.
    for page_size in [Some(1), Some(2), Some(5), Some(20), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(ref next) = token {
                body["NextToken"] = json!(next);
            }
            let page = aws.describe(body).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size:?}: expected 200, got {} ({:?})",
                page.status, page.body
            );
            let canaries = page
                .at("Canaries")
                .as_array()
                .unwrap_or_else(|| panic!("case page size {page_size:?}: Canaries is not an array"))
                .clone();
            if let Some(size) = page_size {
                assert!(
                    canaries.len() <= size,
                    "case page size {page_size:?}: a page returned {} entries",
                    canaries.len()
                );
            }
            for canary in &canaries {
                seen.push(
                    canary["Name"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case page size {page_size:?}: a canary has no Name")
                        })
                        .to_string(),
                );
            }
            token = page.at("NextToken").as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 2,
                "case page size {page_size:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, sorted,
            "case page size {page_size:?}: expected every canary exactly once in name order, got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case page size {page_size:?}: a canary was returned twice, got {seen:?}"
        );
    }

    // `Names` filters, over 1..5 entries (the modelled bounds) and over a name no canary
    // has, which simply matches nothing.
    for (case, filter, expected) in [
        ("one name", vec!["charlie"], vec!["charlie"]),
        (
            "five names",
            vec!["echo", "alpha", "bravo", "charlie", "delta"],
            vec!["alpha", "bravo", "charlie", "delta", "echo"],
        ),
        ("unknown name", vec!["nope"], vec![]),
        (
            "mixed known and unknown",
            vec!["nope", "foxtrot"],
            vec!["foxtrot"],
        ),
    ] {
        let page = aws.describe(json!({ "Names": filter })).await;
        let got: Vec<String> = page
            .at("Canaries")
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: Canaries is not an array"))
            .iter()
            .map(|canary| canary["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got, expected,
            "case {case}: expected {expected:?}, got {got:?}"
        );
    }

    // A deleted canary leaves the listing; the rest keep their order.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let deleted = aws.delete("charlie").await;
    assert_eq!(
        deleted.status, 200,
        "case delete charlie: expected 200, got {} ({:?})",
        deleted.status, deleted.body
    );
    let page = aws.describe(json!({})).await;
    let got: Vec<String> = page
        .at("Canaries")
        .as_array()
        .expect("Canaries is an array")
        .iter()
        .map(|canary| canary["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: Vec<String> = sorted
        .iter()
        .filter(|name| **name != "charlie")
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        got, expected,
        "case describe-after-delete: expected {expected:?}, got {got:?}"
    );

    // A token this operation never issued is a bad request, not an empty page — whether
    // the prefix is missing or the payload could not be a canary name.
    for (case, token) in [
        ("no prefix", "not-a-token"),
        ("name prefix with an illegal canary name", "nm#NotALegalName"),
        ("name prefix with an empty name", "nm#"),
        ("offset prefix with a non-number", "ix#abc"),
        ("offset prefix past the end of the collection", "ix#9999"),
    ] {
        let bogus = aws.describe(json!({ "NextToken": token })).await;
        assert_eq!(
            bogus.status, 400,
            "case bogus token {case}: expected 400, got {} ({:?})",
            bogus.status, bogus.body
        );
        assert_eq!(
            bogus.error_code.as_deref(),
            Some("ValidationException"),
            "case bogus token {case}: expected ValidationException, got {:?}",
            bogus.error_code
        );
    }
}

/// `CanaryCodeInput`'s two prose rules — a handler is required exactly when no blueprint
/// is given, and the script comes either from Amazon S3 or inline, never both — hold over
/// the whole class of code structures.
#[tokio::test]
async fn code_structures_are_accepted_exactly_when_the_model_says_they_are() {
    let aws = Synthetics::new("us-east-1");
    // Rows differ along which source members and which entry-point members are present.
    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "handler with s3 bucket and key",
            json!({ "Handler": "index.handler", "S3Bucket": "b", "S3Key": "k" }),
            200,
        ),
        (
            "handler with s3 bucket, key and version",
            json!({ "Handler": "folder/index.handler", "S3Bucket": "b", "S3Key": "k", "S3Version": "v2" }),
            200,
        ),
        (
            "handler with inline zip",
            json!({ "Handler": "index.handler", "ZipFile": "UEsDBAoAAAAAAA==" }),
            200,
        ),
        (
            "blueprint without handler",
            json!({ "BlueprintTypes": ["multi-checks"], "S3Bucket": "b", "S3Key": "k" }),
            200,
        ),
        (
            "blueprint alone, with no script source",
            json!({ "BlueprintTypes": ["multi-checks"] }),
            200,
        ),
        (
            "no source at all",
            json!({ "Handler": "index.handler" }),
            400,
        ),
        (
            "s3 bucket without key",
            json!({ "Handler": "index.handler", "S3Bucket": "b" }),
            400,
        ),
        (
            "s3 key without bucket",
            json!({ "Handler": "index.handler", "S3Key": "k" }),
            400,
        ),
        (
            "both inline zip and s3",
            json!({ "Handler": "index.handler", "ZipFile": "UEsDBAoAAAAAAA==", "S3Bucket": "b", "S3Key": "k" }),
            400,
        ),
        (
            "neither handler nor blueprint",
            json!({ "S3Bucket": "b", "S3Key": "k" }),
            400,
        ),
        (
            "handler alongside a blueprint",
            json!({ "Handler": "index.handler", "BlueprintTypes": ["multi-checks"], "S3Bucket": "b", "S3Key": "k" }),
            400,
        ),
    ];

    for (index, (case, code, expected)) in cases.into_iter().enumerate() {
        let name = format!("code-case-{index}");
        let mut body = create_body(&name);
        body["Code"] = code;
        let answer = aws.create(body).await;
        assert_eq!(
            answer.status, expected,
            "case {case}: expected {expected}, got {} ({:?})",
            answer.status, answer.body
        );
        if expected == 400 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {:?}",
                answer.error_code
            );
            // A refused create must not leave a record behind.
            let read = aws.get(&name).await;
            assert_eq!(
                read.status, 404,
                "case {case}: a refused create stored a canary anyway, got {} ({:?})",
                read.status, read.body
            );
        }
    }
}

/// `CanaryRunConfigInput$TimeoutInSeconds`: "If you omit this field, the frequency of the
/// canary is used as this value, up to a maximum of 14 minutes." The rule is asserted over
/// a range of schedule expressions — both rate units, the run-once special value, a cron
/// expression, and the 14-minute bound with a value on it and one past it.
#[tokio::test]
async fn an_omitted_run_timeout_follows_the_schedule_frequency() {
    let aws = Synthetics::new("us-east-1");
    // Expression forms and the documented units come from
    // `smithy_query(query="doc",shape="CanaryScheduleInput",member="Expression")`.
    let cases = [
        ("one minute", "rate(1 minute)", 60),
        ("ten minutes", "rate(10 minutes)", 600),
        ("fourteen minutes, the published cap", "rate(14 minutes)", 840),
        ("fifteen minutes, past the cap", "rate(15 minutes)", 840),
        ("one hour", "rate(1 hour)", 840),
        ("run once", "rate(0 minute)", 840),
        ("cron, no single frequency", "cron(0 10 * * ? *)", 840),
    ];

    for (index, (case, expression, expected)) in cases.into_iter().enumerate() {
        let name = format!("schedule-case-{index}");
        let mut body = create_body(&name);
        body["Schedule"] = json!({ "Expression": expression });
        // Half the rows omit `RunConfig` entirely and half supply an empty one: the
        // documented default belongs to the member, so it applies either way.
        if index % 2 == 0 {
            body["RunConfig"] = json!({});
        }
        let answer = aws.create(body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("Canary.RunConfig.TimeoutInSeconds"),
            &json!(expected),
            "case {case}: expected a {expected}s timeout for {expression}, got {:?}",
            answer.at("Canary.RunConfig.TimeoutInSeconds")
        );
        assert_eq!(
            answer.string_at("Canary.Schedule.Expression"),
            expression,
            "case {case}: expected the stored expression, got {:?}",
            answer.at("Canary.Schedule.Expression")
        );
        // The omitted `DurationInSeconds` takes its documented default of 0.
        assert_eq!(
            answer.at("Canary.Schedule.DurationInSeconds"),
            &json!(0),
            "case {case}: expected the documented default of 0, got {:?}",
            answer.at("Canary.Schedule.DurationInSeconds")
        );
    }

    // A supplied timeout is kept as supplied, whatever the schedule says.
    let mut body = create_body("explicit-timeout");
    body["Schedule"] = json!({ "Expression": "rate(1 hour)", "DurationInSeconds": 3600 });
    body["RunConfig"] = json!({ "TimeoutInSeconds": 120 });
    let answer = aws.create(body).await;
    assert_eq!(
        answer.at("Canary.RunConfig.TimeoutInSeconds"),
        &json!(120),
        "case explicit timeout: expected 120, got {:?}",
        answer.at("Canary.RunConfig.TimeoutInSeconds")
    );
    assert_eq!(
        answer.at("Canary.Schedule.DurationInSeconds"),
        &json!(3600),
        "case explicit timeout: expected the supplied duration, got {:?}",
        answer.at("Canary.Schedule.DurationInSeconds")
    );
}

/// `CanaryRunConfigInput$MemoryInMB`: "This value must be a multiple of 64." Asserted on
/// each published bound of the member's `range 960..3008` and on values inside it.
#[tokio::test]
async fn run_config_memory_must_be_a_multiple_of_64() {
    let aws = Synthetics::new("us-east-1");
    let cases = [
        ("the published minimum", 960, 200),
        ("one page above the minimum", 1024, 200),
        ("the published maximum", 3008, 200),
        ("not a multiple of 64", 1000, 400),
        ("one above a multiple of 64", 1025, 400),
    ];

    for (index, (case, memory, expected)) in cases.into_iter().enumerate() {
        let name = format!("memory-case-{index}");
        let mut body = create_body(&name);
        body["RunConfig"] = json!({ "MemoryInMB": memory });
        let answer = aws.create(body).await;
        assert_eq!(
            answer.status, expected,
            "case {case} ({memory} MB): expected {expected}, got {} ({:?})",
            answer.status, answer.body
        );
        if expected == 200 {
            assert_eq!(
                answer.at("Canary.RunConfig.MemoryInMB"),
                &json!(memory),
                "case {case}: expected the supplied memory to be stored, got {:?}",
                answer.at("Canary.RunConfig.MemoryInMB")
            );
        }
    }
}

/// Every operation in the slice answers `ResourceNotFoundException` for a canary that was
/// never created, rather than a hollow success.
#[tokio::test]
async fn an_unknown_canary_is_not_found_on_every_operation() {
    let aws = Synthetics::new("us-east-1");
    for (case, answer) in [
        ("get", aws.get("ghost").await),
        ("start", aws.start("ghost").await),
        ("stop", aws.stop("ghost").await),
        ("delete", aws.delete("ghost").await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error_code
        );
    }
}

/// The run history `StartCanary` opens and `StopCanary` closes, read back through
/// `GetCanaryRuns`.
///
/// This harness mounts Synthetics alone, so the object store a run's artifacts belong to is
/// not there to take them: a run started here therefore ends in the service's own modelled
/// FAILED state with the EXECUTION_FAILURE reason code the model documents for artifacts
/// that could not be saved, and a run cut short by `StopCanary` ends the same way. What a
/// run reports when its artifacts CAN be stored is asserted beside the services that own
/// them (`canary_run_outcomes`).
#[tokio::test]
async fn a_started_run_is_recorded_and_reported_without_a_fabricated_verdict() {
    let aws = Synthetics::new("us-east-1");
    let name = "run-history";
    assert_eq!(
        aws.create(create_body(name)).await.status,
        200,
        "case seed: expected 200"
    );

    // No run until one is started.
    let empty = aws.runs(name, json!({})).await;
    assert_eq!(
        empty.status, 200,
        "case runs-before-start: expected 200, got {} ({:?})",
        empty.status, empty.body
    );
    assert_eq!(
        empty.at("CanaryRuns").as_array().map(Vec::len),
        Some(0),
        "case runs-before-start: expected no runs, got {:?}",
        empty.at("CanaryRuns")
    );

    tokio::time::sleep(PAST_PROVISIONING).await;
    let started_at = std::time::Instant::now();
    assert_eq!(
        aws.start(name).await.status,
        200,
        "case start: expected 200"
    );

    // Immediately after the start the run is open: its outcome is owed by the clock.
    let open = aws.runs(name, json!({})).await;
    let read_within = started_at.elapsed();
    let runs = open
        .at("CanaryRuns")
        .as_array()
        .expect("CanaryRuns is an array")
        .clone();
    assert_eq!(
        runs.len(),
        1,
        "case runs-after-start: expected exactly one run, got {runs:?}"
    );
    let run_id = runs[0]["Id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid(&run_id),
        "case runs-after-start: expected CanaryRun.Id to be a UUID, got {run_id:?}"
    );
    assert_eq!(
        runs[0]["Name"].as_str(),
        Some(name),
        "case runs-after-start: expected the run to name its canary, got {:?}",
        runs[0]["Name"]
    );
    // Guarded on elapsed wall-clock time: the run's outcome comes due 500 ms after the
    // start, so this is only an assertion about the open run while the read provably
    // happened before that. A loaded machine that took longer legitimately sees the
    // settled run, which the post-stop assertions below cover anyway.
    if read_within < RUN_STILL_OPEN_WITHIN {
        assert_eq!(
            runs[0]["Status"]["State"].as_str(),
            Some("RUNNING"),
            "case runs-after-start (read within {read_within:?}): expected RUNNING, got {:?}",
            runs[0]["Status"]
        );
    }
    assert!(
        runs[0]["Timeline"]["Started"].is_number(),
        "case runs-after-start: expected Timeline.Started to be set, got {:?}",
        runs[0]["Timeline"]
    );
    assert_eq!(
        runs[0]["ArtifactS3Location"].as_str(),
        Some("s3://artifacts/canary"),
        "case runs-after-start: expected the canary's artifact location, got {:?}",
        runs[0]["ArtifactS3Location"]
    );

    // Stopping the canary closes the run, and the recorded outcome says why it has no
    // verdict instead of claiming one.
    assert_eq!(aws.stop(name).await.status, 200, "case stop: expected 200");
    let closed = aws.runs(name, json!({})).await;
    let runs = closed
        .at("CanaryRuns")
        .as_array()
        .expect("CanaryRuns is an array")
        .clone();
    assert_eq!(
        runs.len(),
        1,
        "case runs-after-stop: expected the same single run, got {runs:?}"
    );
    assert_eq!(
        runs[0]["Id"].as_str(),
        Some(run_id.as_str()),
        "case runs-after-stop: expected the same run record, got {:?}",
        runs[0]["Id"]
    );
    assert_eq!(
        runs[0]["Status"]["State"].as_str(),
        Some("FAILED"),
        "case runs-after-stop: expected FAILED, got {:?}",
        runs[0]["Status"]
    );
    assert_eq!(
        runs[0]["Status"]["StateReasonCode"].as_str(),
        Some("EXECUTION_FAILURE"),
        "case runs-after-stop: expected EXECUTION_FAILURE, got {:?}",
        runs[0]["Status"]
    );
    assert!(
        runs[0]["Status"]["StateReason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "case runs-after-stop: expected a reason for the failure, got {:?}",
        runs[0]["Status"]
    );
    assert!(
        runs[0]["Timeline"]["Completed"].is_number(),
        "case runs-after-stop: expected Timeline.Completed to be set, got {:?}",
        runs[0]["Timeline"]
    );

    // Several runs page newest-first, and the token resumes exactly where the page ended.
    for round in 0..3 {
        assert_eq!(
            aws.start(name).await.status,
            200,
            "case extra start {round}: expected 200"
        );
        assert_eq!(
            aws.stop(name).await.status,
            200,
            "case extra stop {round}: expected 200"
        );
    }
    for page_size in [1usize, 2, 3, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(ref next) = token {
                body["NextToken"] = json!(next);
            }
            let page = aws.runs(name, body).await;
            assert_eq!(
                page.status, 200,
                "case run page size {page_size}: expected 200, got {} ({:?})",
                page.status, page.body
            );
            let entries = page
                .at("CanaryRuns")
                .as_array()
                .unwrap_or_else(|| panic!("case run page size {page_size}: not an array"))
                .clone();
            assert!(
                entries.len() <= page_size,
                "case run page size {page_size}: a page returned {} entries",
                entries.len()
            );
            for entry in &entries {
                seen.push(entry["Id"].as_str().unwrap_or_default().to_string());
            }
            token = page.at("NextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= 4,
                "case run page size {page_size}: paging did not terminate, got {seen:?}"
            );
        }
        assert_eq!(
            seen.len(),
            4,
            "case run page size {page_size}: expected all four runs, got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case run page size {page_size}: a run was returned twice, got {seen:?}"
        );
        // Newest first: the run opened first is reported last.
        assert_eq!(
            seen.last().map(String::as_str),
            Some(run_id.as_str()),
            "case run page size {page_size}: expected the oldest run last, got {seen:?}"
        );
    }

    // A token this operation never issued is a bad request, not an empty page — the same
    // rule DescribeCanaries applies. A WELL-FORMED token whose run is no longer in the
    // history is the last, empty page instead: retention or the retained-run bound can
    // drop it between two calls.
    for (case, token) in [
        ("no prefix", "not-a-token"),
        ("prefix but not a run id", "rn#not-a-uuid"),
        ("prefix but a truncated run id", "rn#00000000-0000-4000"),
    ] {
        let answer = aws.runs(name, json!({ "NextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case run token {case}: expected 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case run token {case}: expected ValidationException, got {:?}",
            answer.error_code
        );
    }
    let expired = aws
        .runs(
            name,
            json!({ "NextToken": "rn#00000000-0000-4000-8000-000000000000" }),
        )
        .await;
    assert_eq!(
        expired.status, 200,
        "case run token for a dropped run: expected 200, got {} ({:?})",
        expired.status, expired.body
    );
    assert_eq!(
        expired.at("CanaryRuns").as_array().map(Vec::len),
        Some(0),
        "case run token for a dropped run: expected the last, empty page, got {:?}",
        expired.at("CanaryRuns")
    );
    assert!(
        expired.at("NextToken").is_null(),
        "case run token for a dropped run: expected no further token, got {:?}",
        expired.at("NextToken")
    );

    // There are no dry runs to report, and a dry-run id this emulator never minted names
    // nothing.
    let dry = aws
        .runs(name, json!({ "RunType": "DRY_RUN" }))
        .await;
    assert_eq!(
        dry.at("CanaryRuns").as_array().map(Vec::len),
        Some(0),
        "case dry runs: expected no dry runs, got {:?}",
        dry.at("CanaryRuns")
    );
    let unknown_dry_run = aws
        .runs(
            name,
            json!({ "RunType": "DRY_RUN", "DryRunId": "00000000-0000-4000-8000-000000000000" }),
        )
        .await;
    assert_eq!(
        unknown_dry_run.status, 404,
        "case unknown dry run id: expected 404, got {} ({:?})",
        unknown_dry_run.status, unknown_dry_run.body
    );
    // `RunType=CANARY_RUN` with a `DryRunId` is refused, as the model documents.
    let contradictory = aws
        .runs(
            name,
            json!({ "RunType": "CANARY_RUN", "DryRunId": "00000000-0000-4000-8000-000000000000" }),
        )
        .await;
    assert_eq!(
        contradictory.status, 400,
        "case canary-run with dry run id: expected 400, got {} ({:?})",
        contradictory.status, contradictory.body
    );
    // A canary that does not exist has no run history.
    let ghost = aws.runs("ghost", json!({})).await;
    assert_eq!(
        ghost.status, 404,
        "case runs for an unknown canary: expected 404, got {} ({:?})",
        ghost.status, ghost.body
    );
}

/// The store is bounded: neither a create loop nor a start/stop loop can grow it without
/// limit.
#[tokio::test]
async fn the_store_is_bounded_in_canaries_and_in_retained_runs() {
    let aws = Synthetics::new("us-east-1");

    // Up to the quota every create succeeds; past it the modelled quota error is answered
    // and nothing is stored. The ids minted along the way are also the sample that pins
    // the identifier rule: every one has the modelled UUID shape and none repeats.
    let mut minted: Vec<String> = Vec::new();
    for index in 0..MAX_CANARIES {
        let answer = aws.create(create_body(&format!("quota-{index}"))).await;
        assert_eq!(
            answer.status, 200,
            "case create {index} of {MAX_CANARIES}: expected 200, got {} ({:?})",
            answer.status, answer.body
        );
        let id = answer.string_at("Canary.Id");
        assert!(
            is_uuid(&id),
            "case create {index}: expected Canary.Id to have the modelled UUID shape, got {id:?}"
        );
        minted.push(id);
    }
    let unique_ids: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        unique_ids.len(),
        minted.len(),
        "case minted ids: expected {} distinct ids, got {}",
        minted.len(),
        unique_ids.len()
    );
    for case in ["one past the quota", "two past the quota"] {
        let answer = aws.create(create_body("quota-overflow")).await;
        assert_eq!(
            answer.status, 402,
            "case {case}: expected 402, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: expected ServiceQuotaExceededException, got {:?}",
            answer.error_code
        );
        let read = aws.get("quota-overflow").await;
        assert_eq!(
            read.status, 404,
            "case {case}: a refused create stored a canary anyway, got {} ({:?})",
            read.status, read.body
        );
    }
    // The store really holds exactly the quota, counted by paging it.
    let mut stored = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 20 });
        if let Some(ref next) = token {
            body["NextToken"] = json!(next);
        }
        let page = aws.describe(body).await;
        stored += page.at("Canaries").as_array().map_or(0, Vec::len);
        token = page.at("NextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        stored, MAX_CANARIES,
        "case store size: expected exactly {MAX_CANARIES} canaries, got {stored}"
    );

    // Freeing a name lets a create through again, so the bound refuses rather than wedges.
    tokio::time::sleep(PAST_PROVISIONING).await;
    assert_eq!(
        aws.delete("quota-0").await.status,
        200,
        "case free a name: expected 200"
    );
    assert_eq!(
        aws.create(create_body("quota-replacement")).await.status,
        200,
        "case create after freeing a name: expected 200"
    );

    // A start/stop loop past the retained-run cap keeps the newest runs and drops the
    // oldest, so one canary's history cannot grow without limit either.
    let name = "quota-replacement";
    tokio::time::sleep(PAST_PROVISIONING).await;
    let mut first_run_id = None;
    let overflow = 5;
    for round in 0..MAX_RETAINED_RUNS_PER_CANARY + overflow {
        assert_eq!(
            aws.start(name).await.status,
            200,
            "case run {round}: start expected 200"
        );
        if round == 0 {
            let page = aws.runs(name, json!({ "MaxResults": 1 })).await;
            first_run_id = page.at("CanaryRuns")[0]["Id"].as_str().map(str::to_string);
        }
        assert_eq!(
            aws.stop(name).await.status,
            200,
            "case run {round}: stop expected 200"
        );
    }
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 100 });
        if let Some(ref next) = token {
            body["NextToken"] = json!(next);
        }
        let page = aws.runs(name, body).await;
        for run in page.at("CanaryRuns").as_array().unwrap_or(&Vec::new()) {
            ids.push(run["Id"].as_str().unwrap_or_default().to_string());
        }
        token = page.at("NextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        ids.len(),
        MAX_RETAINED_RUNS_PER_CANARY,
        "case retained runs: expected exactly {MAX_RETAINED_RUNS_PER_CANARY} runs after \
         {} start/stop rounds, got {}",
        MAX_RETAINED_RUNS_PER_CANARY + overflow,
        ids.len()
    );
    let first_run_id = first_run_id.expect("the first run was recorded");
    assert!(
        !ids.contains(&first_run_id),
        "case retained runs: expected the oldest run {first_run_id} to have been dropped, \
         got {} ids still holding it",
        ids.len()
    );
    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case retained runs: a run was returned twice, got {} of {} unique",
        unique.len(),
        ids.len()
    );
}

/// A page boundary at a canary name too long to carry in a `Token` (max 252 characters)
/// falls back to the offset form of the token, and still pages without repeat or skip.
#[tokio::test]
async fn a_page_boundary_at_a_very_long_name_still_resumes_exactly() {
    let aws = Synthetics::new("us-east-1");
    // `CanaryName` is `len 1..255, pattern ^[0-9a-z_\-]+$`. Names this long cannot fit the
    // name form of the token (3-character prefix + 250 characters > 252).
    let names: Vec<String> = ["1", "2", "3"]
        .iter()
        .map(|suffix| format!("{}{suffix}", "a".repeat(249)))
        .collect();
    for name in &names {
        assert_eq!(
            name.len(),
            250,
            "case seed: expected a 250-character name, got {}",
            name.len()
        );
        let answer = aws.create(create_body(name)).await;
        assert_eq!(
            answer.status, 200,
            "case seed {name}: expected 200, got {} ({:?})",
            answer.status, answer.body
        );
    }

    for page_size in [1usize, 2] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(ref next) = token {
                assert!(
                    next.len() >= 4 && next.len() <= 252,
                    "case long-name page size {page_size}: the token must satisfy Token's \
                     modelled 4..252 length, got {} characters",
                    next.len()
                );
                body["NextToken"] = json!(next);
            }
            let page = aws.describe(body).await;
            assert_eq!(
                page.status, 200,
                "case long-name page size {page_size}: expected 200, got {} ({:?})",
                page.status, page.body
            );
            for canary in page.at("Canaries").as_array().unwrap_or(&Vec::new()) {
                seen.push(canary["Name"].as_str().unwrap_or_default().to_string());
            }
            token = page.at("NextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case long-name page size {page_size}: expected every canary exactly once in \
             name order, got {} entries",
            seen.len()
        );
    }
}

/// Whether `value` matches the `UUID` pattern the model gives `Canary$Id` and
/// `CanaryRun$Id`: `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
///
/// Written here rather than imported from the crate under test on purpose: an assertion
/// that shared the production predicate would pass vacuously if that predicate were
/// wrong. This reads the modelled pattern group by group.
pub(crate) fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.chars().count() == width
                    && group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
            })
}

