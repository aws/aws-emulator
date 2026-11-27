// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the transcoding-job lifecycle: `CreateQueue`,
//! `GetQueue`, `CreateJob`, `GetJob`, `ListJobs` and `CancelJob`.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from
//! the request's SigV4 scope, the framework deserializes and validates, the handler
//! runs, and the response is serialized — so what is asserted is the bytes a client
//! would see, never an internal call.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header in this service's scope, signed for `region`. It is
/// what the router reads to pick the service, and what the handlers read the ARN region
/// from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
}

/// The region every case signs for unless it names another.
pub(crate) const REGION: &str = "us-west-2";

/// An IAM role ARN of the shape the `Role` member documents.
pub(crate) const ROLE: &str = "arn:aws:iam::123456789012:role/MediaConvertRole";

/// Longer than any case takes to run, so a job stays `SUBMITTED` for the whole of a
/// case that is not about the clock. Pinned rather than raced: the span is state, so no
/// case depends on how fast the machine is.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(3_600);

/// A settle span short enough for the one case that waits past it.
pub(crate) const SHORT_SETTLE: Duration = Duration::from_millis(50);

/// Comfortably past [`SHORT_SETTLE`], so that case always observes the transition.
pub(crate) const PAST_THE_SETTLE: Duration = Duration::from_millis(400);

/// How long a case waits for the CLOCK to run a job before it gives up.
///
/// Generous: it is a deadline for a machine under load, not a span anything is expected to
/// take (a case's own span is the one it pinned, e.g. [`SHORT_SETTLE`]).
pub(crate) const RUN_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll of [`Emulator::await_run`] pauses before it looks again. Only how
/// often the condition is checked, never what a case asserts.
const RUN_POLL_PAUSE: Duration = Duration::from_millis(20);

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
pub(crate) struct Emulator {
    /// The one shared engine behind the router, so a case that needs to SEED state (a
    /// capacity case, say) can reach the store the handlers write to.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

/// One HTTP reply, decoded far enough to assert on.
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// `x-amzn-errortype`, the header restJson1 resolves an error shape from.
    pub(crate) error_type: Option<String>,
    /// The JSON body, or `Value::Null` for an empty one.
    pub(crate) body: Value,
}

impl Reply {
    /// A member of the body, or `Value::Null` when it is absent.
    pub(crate) fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    /// A string member of the body.
    pub(crate) fn string(&self, pointer: &str) -> String {
        self.at(pointer)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The whole `jobTemplate` member, or `Value::Null` when it is absent — what every
    /// job-template operation answers with.
    pub(crate) fn template(&self) -> &Value {
        self.at("/jobTemplate")
    }
}

impl Emulator {
    /// An emulator whose jobs stay `SUBMITTED` for the whole case. Every suite but the
    /// one about the clock wants this.
    pub(crate) fn new() -> Self {
        Self::with_settle_span(HELD_IN_SUBMITTED)
    }

    /// As [`Self::new`], with one more service mounted beside this one on the same router
    /// and built over the SAME engine — for a suite that has to watch what this service's
    /// handlers send to another service over the in-process callback, which is routed by
    /// this very router.
    pub(crate) fn beside<S, F>(build: F) -> Self
    where
        S: smithy_server_runtime::Service,
        F: FnOnce(Arc<engine::InMemoryEngine>) -> S,
    {
        Self::new().mount(build)
    }

    /// Mount one more service beside this one on the same router, built over the SAME
    /// engine, and answer the emulator — so a suite that needs SEVERAL other services (a
    /// bus and the queue one of its rules targets, say) chains the calls.
    pub(crate) fn mount<S, F>(mut self, build: F) -> Self
    where
        S: smithy_server_runtime::Service,
        F: FnOnce(Arc<engine::InMemoryEngine>) -> S,
    {
        let extra = build(Arc::clone(&self.engine));
        // The router has not been cloned or served yet, so the service set is still
        // uniquely held and one more may join it.
        self.router.register_service(extra);
        self
    }

    /// One `awsJson` RPC call against a service mounted BESIDE this one, the way a client
    /// makes it: `POST /` with `X-Amz-Target: <target_prefix>.<operation>` and a
    /// credential scoped to `signing_name`, which is what the router picks the service
    /// from.
    pub(crate) async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let credential = format!(
            "AKIDTESTTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential={credential}"),
            )
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().clone();
        let decoded = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    /// One request to a service mounted BESIDE this one whose protocol is NOT an RPC over
    /// `POST /` — S3's REST, where the bucket and the key are the request's path — the way
    /// a client makes it: `method uri` with a credential scoped to `signing_name`, which
    /// is what the router picks the service from. The reply comes back whole (status,
    /// headers and body), because what such a service reports about a resource — an entity
    /// tag, say — travels in its headers.
    pub(crate) async fn rest(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        body: Vec<u8>,
    ) -> (u16, ::http::HeaderMap, Vec<u8>) {
        let credential = format!(
            "AKIDTESTTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/octet-stream")
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential={credential}"),
            )
            .body(::bytes::Bytes::from(body))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            response.headers().clone(),
            response.body().to_vec(),
        )
    }

    /// An emulator whose jobs are due to leave `SUBMITTED` after `span`.
    pub(crate) fn with_settle_span(span: Duration) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The settle span lives in the service's state, so a case pins it instead of
        // racing the real clock. Nothing on a request path writes it.
        engine
            .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .job_settles_after = span;
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.send_in(REGION, method, uri, body).await
    }

    pub(crate) async fn send_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("mediaconvert.{region}.amazonaws.com"))
            .header("authorization", authorization(region))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_queue(&self, body: Value) -> Reply {
        self.send("POST", "/2017-08-29/queues", Some(body)).await
    }

    pub(crate) async fn get_queue(&self, name: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/queues/{name}"), None)
            .await
    }

    pub(crate) async fn create_job(&self, body: Value) -> Reply {
        self.send("POST", "/2017-08-29/jobs", Some(body)).await
    }

    /// Wait until the CLOCK has run the job `id` to a status nothing further is due from,
    /// and every move it recorded has left the publication queue.
    ///
    /// The condition, not a duration: a job's run is the clock's business (two steps, each
    /// owed the pinned span after the one before it, materialised by the store's own clock
    /// pump), so a case that needs a job that has run looks again until the STORE the
    /// handlers write to says so, or [`RUN_DEADLINE`] passes. The publication queue being
    /// empty is part of the condition, because the event that reports the last step is
    /// drained after the step itself is written. Reading the store makes no MediaConvert
    /// request, so the wait itself cannot be what materialises the run.
    pub(crate) async fn await_run(&self, case: &str, id: &str) {
        let started = std::time::Instant::now();
        loop {
            let status = self.stored_job_status(id);
            let ran = status != "SUBMITTED" && status != "PROGRESSING";
            if ran && self.pending_job_events() == 0 {
                return;
            }
            assert!(
                started.elapsed() < RUN_DEADLINE,
                "case {case:?}: the clock never ran the job {id} to a terminal status with \
                 its moves published; it is {status:?} with {} change(s) still queued",
                self.pending_job_events()
            );
            tokio::time::sleep(RUN_POLL_PAUSE).await;
        }
    }

    /// How many job state changes the store has recorded and nothing has published yet.
    pub(crate) fn pending_job_events(&self) -> usize {
        self.engine
            .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .pending_job_event_count()
    }

    /// The status the STORE holds for the job `id`, or an empty string when it holds no
    /// such job.
    pub(crate) fn stored_job_status(&self, id: &str) -> String {
        self.engine
            .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .jobs
            .get(id)
            .map(|job| job.status.clone())
            .unwrap_or_default()
    }

    pub(crate) async fn get_job(&self, id: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/jobs/{id}"), None)
            .await
    }

    pub(crate) async fn cancel_job(&self, id: &str) -> Reply {
        self.send("DELETE", &format!("/2017-08-29/jobs/{id}"), None)
            .await
    }

    async fn list_jobs(&self, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/2017-08-29/jobs".to_string()
        } else {
            format!("/2017-08-29/jobs?{query}")
        };
        self.send("GET", &uri, None).await
    }

    pub(crate) async fn create_job_template(&self, body: Value) -> Reply {
        self.send("POST", "/2017-08-29/jobTemplates", Some(body))
            .await
    }

    pub(crate) async fn get_job_template(&self, name: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/jobTemplates/{name}"), None)
            .await
    }

    pub(crate) async fn update_job_template(&self, name: &str, body: Value) -> Reply {
        self.send(
            "PUT",
            &format!("/2017-08-29/jobTemplates/{name}"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn delete_job_template(&self, name: &str) -> Reply {
        self.send("DELETE", &format!("/2017-08-29/jobTemplates/{name}"), None)
            .await
    }

    pub(crate) async fn create_preset(&self, body: Value) -> Reply {
        self.send("POST", "/2017-08-29/presets", Some(body)).await
    }

    pub(crate) async fn delete_preset(&self, name: &str) -> Reply {
        self.send("DELETE", &format!("/2017-08-29/presets/{name}"), None)
            .await
    }

    pub(crate) async fn list_presets(&self, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/2017-08-29/presets".to_string()
        } else {
            format!("/2017-08-29/presets?{query}")
        };
        self.send("GET", &uri, None).await
    }

    pub(crate) async fn list_job_templates(&self, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/2017-08-29/jobTemplates".to_string()
        } else {
            format!("/2017-08-29/jobTemplates?{query}")
        };
        self.send("GET", &uri, None).await
    }
}

/// A minimal but structurally real `JobSettings`: one input file and one output group
/// holding one output that states the container it is written in (`Output$Preset`: "You can
/// specify either Preset or Container settings, but not both").
pub(crate) fn settings(input_uri: &str, name_modifier: &str) -> Value {
    json!({
        "inputs": [{ "fileInput": input_uri }],
        "outputGroups": [{
            "name": "File Group",
            "outputs": [{
                "nameModifier": name_modifier,
                "containerSettings": { "container": "MP4" }
            }],
            "outputGroupSettings": {
                "type": "FILE_GROUP_SETTINGS",
                "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
            }
        }]
    })
}

/// `arn:aws:mediaconvert:<region>:<account>:<resource>`, built the way the emulator's
/// own ARN helper builds it, so the expectation is not a second hand-written format.
pub(crate) fn arn(resource: &str) -> String {
    arn_in(REGION, resource)
}

/// As [`arn`], for a region other than the suite's default.
pub(crate) fn arn_in(region: &str, resource: &str) -> String {
    engine::emulator_arn("mediaconvert", region, resource)
}

/// The ARN of the queue `name`, in `region` — the form every queue reference a resource
/// projection reports takes, whichever form the request that stored it was written in.
pub(crate) fn queue_arn_in(region: &str, name: &str) -> String {
    arn_in(region, &format!("queues/{name}"))
}

/// [`queue_arn_in`] for the suite's default region.
pub(crate) fn queue_arn(name: &str) -> String {
    queue_arn_in(REGION, name)
}

/// An Elemental Inference feed ARN of the one form
/// `ElementalInferenceFeedConfigurationSource$FeedArn` accepts:
/// `^arn:aws:elemental-inference:.*:feed/[a-z0-9]{19}$`, from
/// `smithy_query(query="shape",shape="ElementalInferenceFeedConfigurationSource")`.
///
/// `id` is the feed's own identifier; the ARN around it is built by the emulator's own ARN
/// helper, so the expectation is not a second hand-written format. `pub(crate)` because
/// the job-template and job-from-template suites state the same member.
pub(crate) fn feed_arn(id: &str) -> String {
    engine::emulator_arn("elemental-inference", REGION, &format!("feed/{id}"))
}

/// The Elemental Inference settings forms a caller can state, from absent to fully
/// stated — the class `CreateJob$ElementalInferenceSettings`,
/// `CreateJobTemplate$ElementalInferenceSettings` and
/// `UpdateJobTemplate$ElementalInferenceSettings` all range over.
///
/// `Value::Null` is the row that states the member at all; every other row is the value
/// stated, which is also the value every reader of the resource must answer.
pub(crate) fn elemental_inference_forms() -> Vec<(&'static str, Value)> {
    vec![
        ("stated by no member at all", Value::Null),
        ("stated with no member inside it", json!({})),
        (
            "a feed configuration source naming no feed",
            json!({ "feedConfigurationSource": {} }),
        ),
        (
            "a feed configuration source naming a feed",
            json!({
                "feedConfigurationSource": { "feedArn": feed_arn("abcdefghij123456789") }
            }),
        ),
        (
            "a feed configuration source naming a different feed",
            json!({
                "feedConfigurationSource": { "feedArn": feed_arn("0123456789abcdefghi") }
            }),
        ),
    ]
}

/// Feed ARNs outside the pattern `FeedArn` publishes, one broken part each: the
/// identifier one character under and one over the nineteen the pattern fixes, an
/// identifier outside its character set, another service's namespace, and an ARN with no
/// `feed/` segment at all. `pub(crate)` for the same reason
/// [`elemental_inference_forms`] is.
pub(crate) fn malformed_feed_arns() -> Vec<(&'static str, String)> {
    vec![
        ("an identifier one character short", feed_arn("abcdefghij12345678")),
        ("an identifier one character long", feed_arn("abcdefghij1234567890")),
        (
            "an identifier outside the pattern's character set",
            feed_arn("ABCDEFGHIJ123456789"),
        ),
        (
            "another service's namespace",
            engine::emulator_arn("mediaconvert", REGION, "feed/abcdefghij123456789"),
        ),
        (
            "no feed segment",
            engine::emulator_arn("elemental-inference", REGION, "abcdefghij123456789"),
        ),
    ]
}

/// A `hopDestinations` list as a resource projection reports it: an entry that names a
/// queue reports that queue's ARN, and an entry that names none reports no queue.
///
/// The expectation is built from the queue NAME the request's own reference carries, so a
/// suite asserting it spells the projection out rather than borrowing the emulator's.
pub(crate) fn projected_hop_destinations(requested: &Value) -> Value {
    let Some(entries) = requested.as_array() else {
        return requested.clone();
    };
    Value::Array(
        entries
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                if let Some(reference) = entry.pointer("/queue").and_then(Value::as_str) {
                    // A reference is either a bare queue name or the whole queue ARN, so
                    // the name is what is left once that exact prefix is taken off.
                    let name = reference.strip_prefix(&queue_arn("")).unwrap_or(reference);
                    entry["queue"] = json!(queue_arn(name));
                }
                entry
            })
            .collect(),
    )
}

/// Caller-driven growth is bounded: the store accepts queues up to its capacity and
/// then answers the modelled quota error instead of growing further. `CreateJob` guards
/// its own capacity through the same path.
#[tokio::test]
async fn creating_past_the_store_capacity_is_a_quota_refusal() {
    // The capacity the state module publishes, so this test cannot drift from it.
    let capacity = service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_QUEUES;
    let emulator = Emulator::new();

    // The built-in `Default` queue already occupies one slot.
    for index in 1..capacity {
        let reply = emulator
            .create_queue(json!({ "name": format!("bulk-{index}") }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case queue #{index} of {capacity}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
    }

    for name in ["one-too-many", "and-another"] {
        let reply = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            reply.status, 402,
            "case {name:?}: expected HTTP 402 at capacity, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {name:?}: expected ServiceQuotaExceededException, got {:?}",
            reply.error_type
        );
    }

    // A queue already in the store still reads back, so the refusal did not break it.
    let existing = emulator.get_queue("bulk-1").await;
    assert_eq!(
        existing.status, 200,
        "an existing queue must still read back at capacity, got {} with body {}",
        existing.status, existing.body
    );
}

/// Every ARN names the region of the request that read it — not the region of whichever
/// caller happened to touch the store first.
///
/// The table spans the regions a request can be signed for, including the one that
/// created the resources and two that did not, and both resource kinds plus the built-in
/// queue nobody created.
#[tokio::test]
async fn an_arn_names_the_region_of_the_request_that_reads_it() {
    let emulator = Emulator::new();
    let created = emulator
        .send_in(
            "eu-central-1",
            "POST",
            "/2017-08-29/queues",
            Some(json!({ "name": "regional" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created: got {} with body {}",
        created.status, created.body
    );
    let job = emulator
        .send_in(
            "eu-central-1",
            "POST",
            "/2017-08-29/jobs",
            Some(json!({
                "role": ROLE,
                "queue": "regional",
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            })),
        )
        .await;
    assert_eq!(
        job.status, 201,
        "the fixture job must be created: got {} with body {}",
        job.status, job.body
    );
    let id = job.string("/job/id");

    for region in ["eu-central-1", "us-west-2", "ap-southeast-2"] {
        let queue = emulator
            .send_in(region, "GET", "/2017-08-29/queues/regional", None)
            .await;
        assert_eq!(
            queue.string("/queue/arn"),
            arn_in(region, "queues/regional"),
            "case {region:?}: expected the queue ARN in the reading region, got {}",
            queue.body
        );
        let default_queue = emulator
            .send_in(region, "GET", "/2017-08-29/queues/Default", None)
            .await;
        assert_eq!(
            default_queue.string("/queue/arn"),
            arn_in(region, "queues/Default"),
            "case {region:?}: the built-in queue's ARN follows the reading region too, got {}",
            default_queue.body
        );
        let fetched = emulator
            .send_in(region, "GET", &format!("/2017-08-29/jobs/{id}"), None)
            .await;
        assert_eq!(
            fetched.string("/job/arn"),
            arn_in(region, &format!("jobs/{id}")),
            "case {region:?}: expected the job ARN in the reading region, got {}",
            fetched.body
        );
    }
}

/// A request the model's own rules refuse is answered with a code this service's model
/// declares — `BadRequestException` — not with the framework's generic code.
///
/// The table spans the refusal class: an absent required member (either one) and a value
/// outside a declared bound at each end of `Priority`'s range, plus a value outside an
/// enumerated set.
#[tokio::test]
async fn a_request_the_model_refuses_is_a_bad_request() {
    let good_settings = settings("s3://example-bucket/in/a.mp4", "_a");
    let cases: Vec<(&str, Value)> = vec![
        (
            "Role absent",
            json!({ "settings": good_settings.clone() }),
        ),
        ("Settings absent", json!({ "role": ROLE })),
        ("both absent", json!({})),
        (
            "Priority one below the -50 minimum",
            json!({ "role": ROLE, "priority": -51, "settings": good_settings.clone() }),
        ),
        (
            "Priority one above the 50 maximum",
            json!({ "role": ROLE, "priority": 51, "settings": good_settings.clone() }),
        ),
        (
            "StatusUpdateInterval outside the enumerated set",
            json!({
                "role": ROLE,
                "statusUpdateInterval": "SECONDS_7",
                "settings": good_settings.clone()
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (label, request) in &cases {
        let reply = emulator.create_job(request.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
    }

    // And each published bound of Priority is accepted.
    for priority in [-50, 0, 50] {
        let reply = emulator
            .create_job(json!({
                "role": ROLE,
                "priority": priority,
                "settings": good_settings.clone()
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case priority {priority}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/job/priority"),
            &json!(priority),
            "case priority {priority}: expected it stored, got {}",
            reply.body
        );
    }
}

/// `CreateQueue` persists every member it accepts, derives the rest, and `GetQueue`
/// reads back exactly what was stored.
///
/// The table spans the input class: which optional members are present, both
/// `PricingPlan` values, both `QueueStatus` values, and whether tags are given. The
/// `RESERVED` row is the one the issue did not cite.
#[tokio::test]
async fn create_queue_persists_its_input_and_get_queue_reads_it_back() {
    struct Case {
        label: &'static str,
        request: Value,
        pricing_plan: &'static str,
        status: &'static str,
    }

    let cases = vec![
        Case {
            label: "name only",
            request: json!({ "name": "minimal-queue" }),
            pricing_plan: "ON_DEMAND",
            status: "ACTIVE",
        },
        Case {
            label: "paused on-demand queue with a description and a concurrency cap",
            request: json!({
                "name": "paused-queue",
                "description": "holds overnight work",
                "status": "PAUSED",
                "concurrentJobs": 7,
                "maximumConcurrentFeeds": 0
            }),
            pricing_plan: "ON_DEMAND",
            status: "PAUSED",
        },
        Case {
            label: "reserved queue with a reservation plan",
            request: json!({
                "name": "reserved-queue",
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": "AUTO_RENEW",
                    "reservedSlots": 3
                }
            }),
            pricing_plan: "RESERVED",
            status: "ACTIVE",
        },
        Case {
            label: "tagged queue",
            request: json!({
                "name": "tagged-queue",
                "tags": { "team": "video", "cost-centre": "42" }
            }),
            pricing_plan: "ON_DEMAND",
            status: "ACTIVE",
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let label = case.label;
        let name = case.request["name"].as_str().expect("the case names a queue");

        let created = emulator.create_queue(case.request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/queue/name"),
            name,
            "case {label:?}: expected Name {name:?}, got {}",
            created.body
        );
        assert_eq!(
            created.string("/queue/arn"),
            arn(&format!("queues/{name}")),
            "case {label:?}: expected the queues/<Name> ARN, got {}",
            created.body
        );
        assert_eq!(
            created.string("/queue/pricingPlan"),
            case.pricing_plan,
            "case {label:?}: expected PricingPlan {:?}, got {}",
            case.pricing_plan,
            created.body
        );
        assert_eq!(
            created.string("/queue/status"),
            case.status,
            "case {label:?}: expected Status {:?}, got {}",
            case.status,
            created.body
        );
        assert_eq!(
            created.string("/queue/type"),
            "CUSTOM",
            "case {label:?}: a caller-created queue is CUSTOM, got {}",
            created.body
        );
        assert_eq!(
            created.at("/queue/submittedJobsCount"),
            &json!(0),
            "case {label:?}: a new queue holds no jobs, got {}",
            created.body
        );
        assert_eq!(
            created.at("/queue/progressingJobsCount"),
            &json!(0),
            "case {label:?}: a new queue holds no jobs, got {}",
            created.body
        );
        let created_at = created.at("/queue/createdAt").as_i64();
        assert!(
            created_at.is_some_and(|secs| secs > 1_600_000_000),
            "case {label:?}: expected CreatedAt in epoch seconds, got {}",
            created.body
        );
        assert_eq!(
            created.at("/queue/lastUpdated").as_i64(),
            created_at,
            "case {label:?}: an untouched queue's LastUpdated equals its CreatedAt, got {}",
            created.body
        );
        // Every optional member the caller supplied comes back as supplied.
        for member in ["description", "concurrentJobs", "maximumConcurrentFeeds"] {
            if let Some(expected) = case.request.get(member) {
                assert_eq!(
                    created.at(&format!("/queue/{member}")),
                    expected,
                    "case {label:?}: expected {member} {expected}, got {}",
                    created.body
                );
            }
        }
        if let Some(plan) = case.request.get("reservationPlanSettings") {
            for member in ["commitment", "renewalType", "reservedSlots"] {
                assert_eq!(
                    created.at(&format!("/queue/reservationPlan/{member}")),
                    &plan[member],
                    "case {label:?}: expected ReservationPlan.{member} {}, got {}",
                    plan[member],
                    created.body
                );
            }
            assert_eq!(
                created.string("/queue/reservationPlan/status"),
                "ACTIVE",
                "case {label:?}: a just-purchased reservation plan is ACTIVE, got {}",
                created.body
            );
            let purchased = created
                .at("/queue/reservationPlan/purchasedAt")
                .as_i64()
                .expect("PurchasedAt is epoch seconds");
            let expires = created
                .at("/queue/reservationPlan/expiresAt")
                .as_i64()
                .expect("ExpiresAt is epoch seconds");
            assert_eq!(
                expires - purchased,
                365 * 24 * 60 * 60,
                "case {label:?}: a ONE_YEAR commitment expires a year after purchase, got {}",
                created.body
            );
        }

        // Round trip: the reader returns the stored resource, not the request.
        let fetched = emulator.get_queue(name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected GetQueue HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/queue"),
            created.at("/queue"),
            "case {label:?}: GetQueue must return what CreateQueue persisted, got {}",
            fetched.body
        );
    }

    // The built-in queue exists without anybody creating it, and is SYSTEM.
    let default_queue = emulator.get_queue("Default").await;
    assert_eq!(
        default_queue.status, 200,
        "the built-in Default queue must exist: got {} with body {}",
        default_queue.status, default_queue.body
    );
    assert_eq!(
        default_queue.string("/queue/type"),
        "SYSTEM",
        "the built-in Default queue is SYSTEM, got {}",
        default_queue.body
    );
}

/// A second `CreateQueue` for a name already in the store is IDEMPOTENT — the same 201
/// with the queue that is already there, unchanged — while the name of the BUILT-IN queue
/// is a conflict (no `CreateQueue` ever minted it), and an unknown name is not found.
#[tokio::test]
async fn re_creating_a_queue_is_idempotent_and_unknown_queues_are_not_found() {
    let emulator = Emulator::new();
    for name in ["alpha", "a-queue-with-a-much-longer-name"] {
        let first = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            first.status, 201,
            "case {name:?}: expected the first CreateQueue to succeed, got {} with body {}",
            first.status, first.body
        );
        let again = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            again.status, 201,
            "case {name:?}: re-creating an existing queue must succeed, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.at("/queue"),
            first.at("/queue"),
            "case {name:?}: the repeat must return the stored queue unchanged, got {}",
            again.body
        );
    }

    // The built-in queue is not a resource any create produced, so its name conflicts.
    let system = emulator.create_queue(json!({ "name": "Default" })).await;
    assert_eq!(
        system.status, 409,
        "case \"Default\": re-creating the built-in queue must conflict, got {} with body {}",
        system.status, system.body
    );
    assert_eq!(
        system.error_type.as_deref(),
        Some("ConflictException"),
        "case \"Default\": expected ConflictException, got {:?}",
        system.error_type
    );

    for missing in ["nope", "also-missing"] {
        let reply = emulator.get_queue(missing).await;
        assert_eq!(
            reply.status, 404,
            "case {missing:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {missing:?}: expected NotFoundException, got {:?}",
            reply.error_type
        );
    }
}

/// `ReservationPlanSettings` is "Required for reserved queues and not applicable to
/// on-demand queues", so each pairing of `PricingPlan` with the member's presence is
/// accepted or refused accordingly — and a refused pairing carries the sentence AWS words
/// for it, which is fixed text with nothing of the request in it.
#[tokio::test]
async fn a_reservation_plan_is_required_exactly_for_reserved_queues() {
    let plan = json!({
        "commitment": "ONE_YEAR",
        "renewalType": "EXPIRE",
        "reservedSlots": 1
    });
    /// The sentence a reserved queue submitted without reservation plan settings is refused
    /// with, as AWS words it.
    const RESERVED_WITHOUT_A_PLAN: &str =
        "You must provide reservation plan settings when creating a reserved queue.";
    /// One pairing of `PricingPlan` with the member's presence, and what the create must
    /// answer for it.
    struct Pairing {
        label: &'static str,
        /// The `PricingPlan` the request states, or `None` for a request stating none.
        pricing_plan: Option<&'static str>,
        /// Whether the request carries `ReservationPlanSettings`.
        with_plan: bool,
        /// The HTTP status the create must answer.
        expected: u16,
        /// The message a refusal must state, where the pairing has one.
        message: Option<&'static str>,
    }
    let cases: &[Pairing] = &[
        Pairing {
            label: "reserved without a plan",
            pricing_plan: Some("RESERVED"),
            with_plan: false,
            expected: 400,
            message: Some(RESERVED_WITHOUT_A_PLAN),
        },
        Pairing {
            label: "reserved with a plan",
            pricing_plan: Some("RESERVED"),
            with_plan: true,
            expected: 201,
            message: None,
        },
        Pairing {
            label: "on-demand with a plan",
            pricing_plan: Some("ON_DEMAND"),
            with_plan: true,
            expected: 400,
            message: None,
        },
        Pairing {
            label: "on-demand without a plan",
            pricing_plan: Some("ON_DEMAND"),
            with_plan: false,
            expected: 201,
            message: None,
        },
        Pairing {
            label: "defaulted plan, no settings",
            pricing_plan: None,
            with_plan: false,
            expected: 201,
            message: None,
        },
        Pairing {
            label: "defaulted plan with settings",
            pricing_plan: None,
            with_plan: true,
            expected: 400,
            message: None,
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let mut request = json!({ "name": format!("queue-{index}") });
        if let Some(pricing_plan) = case.pricing_plan {
            request["pricingPlan"] = json!(pricing_plan);
        }
        if case.with_plan {
            request["reservationPlanSettings"] = plan.clone();
        }
        let reply = emulator.create_queue(request).await;
        assert_eq!(
            reply.status, case.expected,
            "case {label:?}: expected HTTP {}, got {} with body {}",
            case.expected, reply.status, reply.body
        );
        if case.expected == 400 {
            assert_eq!(
                reply.error_type.as_deref(),
                Some("BadRequestException"),
                "case {label:?}: expected BadRequestException, got {:?}",
                reply.error_type
            );
        }
        if let Some(message) = case.message {
            assert_eq!(
                reply.body.get("message").and_then(Value::as_str),
                Some(message),
                "case {label:?}: expected the refusal to state {message:?}, got {}",
                reply.body
            );
        }
    }
}

/// `CreateJob` stores the job and `GetJob` hands back exactly what was stored.
///
/// The table spans what a caller can vary: the queue (absent, by name, by ARN), both
/// ends of the `Priority` range, user metadata, the status-update interval, the billing
/// tags source, and differing `Settings`.
#[tokio::test]
async fn create_job_persists_its_input_and_get_job_reads_it_back() {
    struct Case {
        label: &'static str,
        request: Value,
        queue: &'static str,
    }

    let emulator = Emulator::new();
    let created_queue = emulator.create_queue(json!({ "name": "test-queue" })).await;
    assert_eq!(
        created_queue.status, 201,
        "the fixture queue must be created: got {} with body {}",
        created_queue.status, created_queue.body
    );
    // The `jobTemplate` row below references this template: a `CreateJob` reference must
    // resolve to a stored resource, so the template the job names is created first.
    let created_template = emulator
        .create_job_template(json!({
            "name": "my-template",
            // A graph a job can be built from: `JobTemplateSettings` needs an output
            // group, and each of its outputs needs the container it is written in, so the
            // fixture states the output groups this suite's own job fixture builds.
            "settings": {
                "inputs": [{ "timecodeSource": "ZEROBASED" }],
                "outputGroups": settings("s3://example-bucket/in/ignored.mp4", "_t")["outputGroups"]
                    .clone()
            }
        }))
        .await;
    assert_eq!(
        created_template.status, 201,
        "the fixture job template must be created: got {} with body {}",
        created_template.status, created_template.body
    );

    let cases = vec![
        Case {
            label: "no queue, so the default queue",
            request: json!({
                "role": ROLE,
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            }),
            queue: "Default",
        },
        Case {
            label: "queue by name, lowest priority, user metadata",
            request: json!({
                "role": ROLE,
                "queue": "test-queue",
                "priority": -50,
                "userMetadata": { "k": "v", "batch": "nightly" },
                "settings": settings("s3://example-bucket/in/b.mov", "_b")
            }),
            queue: "test-queue",
        },
        Case {
            label: "queue by ARN, highest priority, status updates and billing source",
            request: json!({
                "role": ROLE,
                "queue": arn("queues/test-queue"),
                "priority": 50,
                "statusUpdateInterval": "SECONDS_120",
                "billingTagsSource": "JOB",
                "settings": settings("s3://example-bucket/in/c.mkv", "_c")
            }),
            queue: "test-queue",
        },
        Case {
            label: "job template name and an acceleration setting",
            request: json!({
                "role": ROLE,
                "jobTemplate": "my-template",
                "accelerationSettings": { "mode": "DISABLED" },
                "settings": settings("s3://example-bucket/in/d.ts", "_d")
            }),
            queue: "Default",
        },
    ];

    let mut seen_ids = BTreeMap::new();
    for case in &cases {
        let label = case.label;
        let created = emulator.create_job(case.request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );

        let id = created.string("/job/id");
        assert!(
            !id.is_empty(),
            "case {label:?}: CreateJob must mint an Id, got {}",
            created.body
        );
        assert!(
            seen_ids.insert(id.clone(), label).is_none(),
            "case {label:?}: Id {id:?} was already minted for {:?}",
            seen_ids.get(&id)
        );
        assert_eq!(
            created.string("/job/arn"),
            arn(&format!("jobs/{id}")),
            "case {label:?}: expected the jobs/<Id> ARN, got {}",
            created.body
        );
        assert_eq!(
            created.string("/job/status"),
            "SUBMITTED",
            "case {label:?}: a new job is SUBMITTED, got {}",
            created.body
        );
        assert_eq!(
            created.string("/job/queue"),
            arn(&format!("queues/{}", case.queue)),
            "case {label:?}: expected the ARN of queue {:?}, got {}",
            case.queue,
            created.body
        );
        assert_eq!(
            created.string("/job/role"),
            ROLE,
            "case {label:?}: Role must be stored as submitted, got {}",
            created.body
        );
        assert_eq!(
            created.at("/job/settings"),
            &case.request["settings"],
            "case {label:?}: Settings must be stored as submitted, got {}",
            created.body
        );
        assert!(
            created
                .at("/job/createdAt")
                .as_i64()
                .is_some_and(|secs| secs > 1_600_000_000),
            "case {label:?}: expected CreatedAt in epoch seconds, got {}",
            created.body
        );
        assert_eq!(
            created.at("/job/timing/submitTime").as_i64(),
            created.at("/job/createdAt").as_i64(),
            "case {label:?}: SubmitTime is when the job was created, got {}",
            created.body
        );
        for member in [
            "priority",
            "userMetadata",
            "statusUpdateInterval",
            "billingTagsSource",
            "jobTemplate",
            "accelerationSettings",
        ] {
            if let Some(expected) = case.request.get(member) {
                assert_eq!(
                    created.at(&format!("/job/{member}")),
                    expected,
                    "case {label:?}: expected {member} {expected}, got {}",
                    created.body
                );
            }
        }

        // Round trip: GetJob returns the stored job, byte for byte.
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected GetJob HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/job"),
            created.at("/job"),
            "case {label:?}: GetJob must return what CreateJob persisted, got {}",
            fetched.body
        );
    }

    for missing in ["doesnotexist", "1700000000000-zzzzzz"] {
        let reply = emulator.get_job(missing).await;
        assert_eq!(
            reply.status, 404,
            "case {missing:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {missing:?}: expected NotFoundException, got {:?}",
            reply.error_type
        );
    }
}

/// A `Queue` that names no stored queue is not found, whichever form it is given in.
#[tokio::test]
async fn create_job_into_an_unknown_queue_is_not_found() {
    let emulator = Emulator::new();
    for reference in ["no-such-queue", &arn("queues/no-such-queue")] {
        let reply = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": reference,
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            }))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {reference:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {reference:?}: expected NotFoundException, got {:?}",
            reply.error_type
        );
    }
}

/// Repeating a `ClientRequestToken` returns the job that token first created, and a
/// different token creates a different job — for any token value.
#[tokio::test]
async fn a_repeated_client_request_token_returns_the_same_job() {
    let emulator = Emulator::new();
    let tokens = ["t", "9c1e5f70-0000-4000-8000-000000000001", &"x".repeat(64)];

    let mut ids = Vec::new();
    for token in tokens {
        let request = json!({
            "role": ROLE,
            "clientRequestToken": token,
            "settings": settings("s3://example-bucket/in/a.mp4", "_a")
        });
        let first = emulator.create_job(request.clone()).await;
        assert_eq!(
            first.status, 201,
            "case {token:?}: expected HTTP 201, got {} with body {}",
            first.status, first.body
        );
        let repeat = emulator.create_job(request).await;
        assert_eq!(
            repeat.at("/job"),
            first.at("/job"),
            "case {token:?}: the same token must return the same job, got {}",
            repeat.body
        );
        ids.push(first.string("/job/id"));
    }

    let listed = emulator.list_jobs("").await;
    let count = listed.at("/jobs").as_array().map_or(0, Vec::len);
    assert_eq!(
        count,
        tokens.len(),
        "expected one job per distinct token ({}), got {count} in {}",
        tokens.len(),
        listed.body
    );
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        tokens.len(),
        "expected a distinct Id per distinct token, got {ids:?}"
    );
}

/// `ListJobs` reads the store: it honours the `queue` and `status` filters, orders by
/// creation time in the requested direction (newest first by default), pages at
/// `maxResults`, and sets `NextToken` only while a further page remains.
#[tokio::test]
async fn list_jobs_filters_orders_and_pages_over_the_store() {
    let emulator = Emulator::new();
    for name in ["queue-a", "queue-b"] {
        let reply = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            reply.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            reply.status, reply.body
        );
    }

    // Six jobs, created in this order, across three queues.
    let plan = [
        ("queue-a", "_0"),
        ("queue-b", "_1"),
        ("queue-a", "_2"),
        ("Default", "_3"),
        ("queue-a", "_4"),
        ("queue-b", "_5"),
    ];
    let mut created_order = Vec::new();
    for (queue, modifier) in plan {
        let reply = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue,
                "settings": settings("s3://example-bucket/in/a.mp4", modifier)
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "fixture job {modifier:?} must be created: got {} with body {}",
            reply.status, reply.body
        );
        created_order.push((queue, reply.string("/job/id")));
    }

    let ids_in = |queue: Option<&str>, descending: bool| -> Vec<String> {
        let mut ids: Vec<String> = created_order
            .iter()
            .filter(|(job_queue, _)| queue.is_none_or(|want| *job_queue == want))
            .map(|(_, id)| id.clone())
            .collect();
        if descending {
            ids.reverse();
        }
        ids
    };

    // (label, query string, expected ids)
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter, default order is newest first",
            String::new(),
            ids_in(None, true),
        ),
        (
            "explicit DESCENDING",
            "order=DESCENDING".to_string(),
            ids_in(None, true),
        ),
        (
            "ASCENDING is oldest first",
            "order=ASCENDING".to_string(),
            ids_in(None, false),
        ),
        (
            "filtered to one queue",
            "queue=queue-a&order=ASCENDING".to_string(),
            ids_in(Some("queue-a"), false),
        ),
        (
            "filtered to another queue",
            "queue=queue-b&order=ASCENDING".to_string(),
            ids_in(Some("queue-b"), false),
        ),
        (
            "filtered to the default queue",
            "queue=Default".to_string(),
            ids_in(Some("Default"), true),
        ),
        (
            // "Provide a queue name to get back only jobs from that queue": this reader's
            // member offers the name form alone, so an ARN narrows to the queue literally
            // named by that whole value, which no queue is here.
            // `tests/all/list_jobs_queue_filter.rs` is the suite for that rule.
            "queue given as an ARN, a form this member does not offer",
            format!("queue={}&order=ASCENDING", arn("queues/queue-a")),
            Vec::new(),
        ),
        (
            "status SUBMITTED matches every job",
            "status=SUBMITTED&order=ASCENDING".to_string(),
            ids_in(None, false),
        ),
        (
            "status CANCELED matches none yet",
            "status=CANCELED".to_string(),
            Vec::new(),
        ),
        (
            "queue and status together",
            "queue=queue-a&status=SUBMITTED&order=ASCENDING".to_string(),
            ids_in(Some("queue-a"), false),
        ),
        (
            "an empty queue lists nothing",
            "queue=no-such-queue".to_string(),
            Vec::new(),
        ),
    ];

    for (label, query, expected) in &cases {
        let reply = emulator.list_jobs(query).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        let got: Vec<String> = reply
            .at("/jobs")
            .as_array()
            .map(|jobs| {
                jobs.iter()
                    .map(|job| job["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            &got, expected,
            "case {label:?}: expected ids {expected:?}, got {got:?} in {}",
            reply.body
        );
        assert_eq!(
            reply.at("/nextToken"),
            &Value::Null,
            "case {label:?}: a single complete page carries no NextToken, got {}",
            reply.body
        );
    }

    // Paging: every page size from 1 to one past the result count walks the same
    // sequence the unpaged call returns, and NextToken stops exactly at the end.
    let expected_sequence = ids_in(None, false);
    for page_size in 1..=expected_sequence.len() + 1 {
        let label = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("order=ASCENDING&maxResults={page_size}");
            if let Some(token) = token.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let reply = emulator.list_jobs(&query).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let page: Vec<String> = reply
                .at("/jobs")
                .as_array()
                .map(|jobs| {
                    jobs.iter()
                        .map(|job| job["id"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {label:?}: a page must not exceed maxResults, got {} ids",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected_sequence.len() + 1,
                "case {label:?}: paging did not terminate after {pages} pages"
            );
            match reply.at("/nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected_sequence,
            "case {label:?}: paging must walk every job exactly once, got {walked:?}"
        );
    }

    // A token this service never issued is a bad request, not an empty page.
    let bogus = emulator.list_jobs("nextToken=not-a-token").await;
    assert_eq!(
        bogus.status, 400,
        "an unrecognised nextToken must be refused, got {} with body {}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_type.as_deref(),
        Some("BadRequestException"),
        "expected BadRequestException, got {:?}",
        bogus.error_type
    );
}

/// A `nextToken` keeps working when the job it anchors on stops matching the caller's
/// filters between pages: the next page resumes at the first job past the anchor key,
/// rather than failing because the anchor left the result set.
#[tokio::test]
async fn a_next_token_survives_the_anchor_job_leaving_the_result_set() {
    const JOBS: usize = 4;

    // Each row removes a different anchor from the filtered set between the two pages.
    for anchor in 0..JOBS - 1 {
        let emulator = Emulator::new();
        let mut fresh = Vec::new();
        for index in 0..JOBS {
            let reply = emulator
                .create_job(json!({
                    "role": ROLE,
                    "settings": settings("s3://example-bucket/in/a.mp4", &format!("_{index}"))
                }))
                .await;
            assert_eq!(
                reply.status, 201,
                "case anchor {anchor}: fixture job {index} must be created, got {} with body {}",
                reply.status, reply.body
            );
            fresh.push(reply.string("/job/id"));
        }

        let first = emulator
            .list_jobs(&format!(
                "status=SUBMITTED&order=ASCENDING&maxResults={}",
                anchor + 1
            ))
            .await;
        let token = first.string("/nextToken");
        assert!(
            !token.is_empty(),
            "case anchor {anchor}: a further page must exist, got {}",
            first.body
        );

        // The anchor is the last job of the page just returned; cancelling it drops it
        // out of the `status=SUBMITTED` filter the next page is taken under.
        let cancelled = emulator.cancel_job(&fresh[anchor]).await;
        assert_eq!(
            cancelled.status, 202,
            "case anchor {anchor}: expected HTTP 202, got {} with body {}",
            cancelled.status, cancelled.body
        );

        let next = emulator
            .list_jobs(&format!(
                "status=SUBMITTED&order=ASCENDING&maxResults=1&nextToken={token}"
            ))
            .await;
        assert_eq!(
            next.status, 200,
            "case anchor {anchor}: the token must still be usable, got {} with body {}",
            next.status, next.body
        );
        assert_eq!(
            next.at("/jobs/0/id").as_str(),
            Some(fresh[anchor + 1].as_str()),
            "case anchor {anchor}: expected the job after the anchor, got {}",
            next.body
        );
    }
}

/// `CancelJob` moves the stored job to `CANCELED`, which every later read observes, and
/// cancelling an already-`CANCELED` job again succeeds and changes nothing.
#[tokio::test]
async fn cancel_job_moves_the_stored_job_to_canceled() {
    let emulator = Emulator::new();
    let reply = emulator.create_queue(json!({ "name": "test-queue" })).await;
    assert_eq!(
        reply.status, 201,
        "the fixture queue must be created: got {} with body {}",
        reply.status, reply.body
    );

    // Each row is a job in a different queue, so cancellation is not keyed to one of them.
    for (label, queue) in [("default queue", "Default"), ("custom queue", "test-queue")] {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue,
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("/job/id");

        let cancelled = emulator.cancel_job(&id).await;
        assert_eq!(
            cancelled.status, 202,
            "case {label:?}: expected HTTP 202, got {} with body {}",
            cancelled.status, cancelled.body
        );
        assert_eq!(
            cancelled.body,
            json!({}),
            "case {label:?}: CancelJob has no output members, got {}",
            cancelled.body
        );

        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.string("/job/status"),
            "CANCELED",
            "case {label:?}: GetJob must observe the cancellation, got {}",
            fetched.body
        );
        let listed = emulator
            .list_jobs(&format!("queue={queue}&status=CANCELED"))
            .await;
        let listed_ids: Vec<String> = listed
            .at("/jobs")
            .as_array()
            .map(|jobs| {
                jobs.iter()
                    .map(|job| job["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed_ids,
            vec![id.clone()],
            "case {label:?}: ListJobs must observe the cancellation, got {}",
            listed.body
        );

        let again = emulator.cancel_job(&id).await;
        assert_eq!(
            again.status, 202,
            "case {label:?}: cancelling a CANCELED job again must succeed, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_type, None,
            "case {label:?}: expected no error on the repeat, got {:?}",
            again.error_type
        );
        assert_eq!(
            again.body,
            json!({}),
            "case {label:?}: the repeat cancel has no output members, got {}",
            again.body
        );
        let after = emulator.get_job(&id).await;
        assert_eq!(
            after.at("/job"),
            fetched.at("/job"),
            "case {label:?}: the repeat cancel must change nothing, got {} after {}",
            after.body,
            fetched.body
        );
        let after = emulator.get_job(&id).await;
        assert_eq!(
            after.string("/job/status"),
            "CANCELED",
            "case {label:?}: the repeat must leave the job CANCELED, got {}",
            after.body
        );
        assert_eq!(
            after.at("/job/timing/finishTime"),
            fetched.at("/job/timing/finishTime"),
            "case {label:?}: the repeat must not move the finish time, got {}",
            after.body
        );
    }

    for missing in ["doesnotexist", "1700000000000-zzzzzz"] {
        let reply = emulator.cancel_job(missing).await;
        assert_eq!(
            reply.status, 404,
            "case {missing:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {missing:?}: expected NotFoundException, got {:?}",
            reply.error_type
        );
    }
}

/// A queue's job counts are counted off the stored jobs, so they move as the jobs do
/// instead of being a second copy that can drift.
#[tokio::test]
async fn queue_job_counts_are_derived_from_the_stored_jobs() {
    let emulator = Emulator::new();
    let reply = emulator.create_queue(json!({ "name": "counted" })).await;
    assert_eq!(
        reply.status, 201,
        "the fixture queue must be created: got {} with body {}",
        reply.status, reply.body
    );

    let mut ids = Vec::new();
    for expected in 1..=4 {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": "counted",
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {expected} jobs: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        ids.push(created.string("/job/id"));

        let queue = emulator.get_queue("counted").await;
        assert_eq!(
            queue.at("/queue/submittedJobsCount"),
            &json!(expected),
            "case {expected} jobs: expected SubmittedJobsCount {expected}, got {}",
            queue.body
        );
        assert_eq!(
            queue.at("/queue/progressingJobsCount"),
            &json!(0),
            "case {expected} jobs: nothing is PROGRESSING, got {}",
            queue.body
        );
        // A job in one queue is never counted against another.
        let default_queue = emulator.get_queue("Default").await;
        assert_eq!(
            default_queue.at("/queue/submittedJobsCount"),
            &json!(0),
            "case {expected} jobs: the Default queue holds none of them, got {}",
            default_queue.body
        );
    }

    // Cancelling drops the job out of the SUBMITTED count.
    for (cancelled, id) in ids.iter().enumerate() {
        let reply = emulator.cancel_job(id).await;
        assert_eq!(
            reply.status, 202,
            "case cancel #{cancelled}: expected HTTP 202, got {} with body {}",
            reply.status, reply.body
        );
        let queue = emulator.get_queue("counted").await;
        let expected = ids.len() - cancelled - 1;
        assert_eq!(
            queue.at("/queue/submittedJobsCount"),
            &json!(expected),
            "case cancel #{cancelled}: expected SubmittedJobsCount {expected}, got {}",
            queue.body
        );
    }
}

/// `SUBMITTED` is not a terminal status, so the clock owes a submitted job the steps of its
/// run. Because this emulator has no transcoding engine, the run ends in the service's own
/// modelled failure status with a reason that says so — and a terminal job can no longer be
/// cancelled.
#[tokio::test]
async fn a_submitted_job_leaves_submitted_once_the_clock_makes_it_due() {
    // The span the service ships with is the WHOLE run — from the submit to the status the
    // run ends in — and it is non-zero and inside the window this project allows for a
    // clock-owed transition, so a client that submits a job and then waits for its terminal
    // status waits a deliberate value rather than a multiple of one.
    let shipped = engine::InMemoryEngine::new()
        .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_settles_after;
    assert!(
        shipped >= Duration::from_secs(2) && shipped <= Duration::from_secs(5),
        "the shipped run must reach a terminal status in a non-zero 2-5s, got {shipped:?}"
    );
    // And each of the two steps that run is — out of `SUBMITTED`, then on to the status the
    // run ends in — is half of it, so a caller acting straight after CreateJob observes
    // `SUBMITTED` and one acting while the job runs observes `PROGRESSING`, each with a
    // window to act inside.
    let step = engine::InMemoryEngine::new()
        .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_run_step();
    assert_eq!(
        step * 2,
        shipped,
        "the two steps of a run must divide the shipped span between them, got {step:?} of \
         {shipped:?}"
    );
    assert!(
        step >= Duration::from_secs(1),
        "each step of a run must leave a caller a window to act inside, got {step:?}"
    );

    let emulator = Emulator::with_settle_span(SHORT_SETTLE);
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": settings("s3://example-bucket/in/a.mp4", "_a")
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");
    assert_eq!(
        created.string("/job/status"),
        "SUBMITTED",
        "a caller acting immediately after CreateJob still sees SUBMITTED, got {}",
        created.body
    );

    // The due instants are stored, never slept for by a handler: this case waits for the
    // CONDITION — the clock having run the job — rather than for a span.
    emulator
        .await_run("a submitted job leaves SUBMITTED on the clock", &id)
        .await;

    let settled = emulator.get_job(&id).await;
    assert_eq!(
        settled.string("/job/status"),
        "ERROR",
        "once due, the job must leave SUBMITTED, got {}",
        settled.body
    );
    let reason = settled.string("/job/errorMessage");
    assert!(
        reason.contains("transcoding engine"),
        "the failure must say why nothing transcoded, got {reason:?}"
    );
    assert_eq!(
        settled.at("/job/createdAt").as_i64(),
        created.at("/job/createdAt").as_i64(),
        "settling must not rewrite CreatedAt, got {}",
        settled.body
    );
    assert!(
        settled.at("/job/timing/finishTime").as_i64().is_some(),
        "a finished job carries a FinishTime, got {}",
        settled.body
    );

    // The status is reported identically by every reader, and is stable across reads.
    let again = emulator.get_job(&id).await;
    assert_eq!(
        again.at("/job"),
        settled.at("/job"),
        "a second read must report the same job, got {}",
        again.body
    );
    let listed = emulator.list_jobs("status=ERROR").await;
    let listed_ids: Vec<String> = listed
        .at("/jobs")
        .as_array()
        .map(|jobs| {
            jobs.iter()
                .map(|job| job["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed_ids,
        vec![id.clone()],
        "ListJobs must observe the same status, got {}",
        listed.body
    );

    let cancelled = emulator.cancel_job(&id).await;
    assert_eq!(
        cancelled.status, 409,
        "a job that has left SUBMITTED can no longer be cancelled, got {} with body {}",
        cancelled.status, cancelled.body
    );
}

/// The whole workflow the issue describes, in one pass: create a queue, submit a job
/// into it, read it back, list it under both filters, cancel it, and see CANCELED.
#[tokio::test]
async fn the_documented_job_workflow_runs_end_to_end() {
    let emulator = Emulator::new();

    let queue = emulator
        .create_queue(json!({ "name": "workflow-queue" }))
        .await;
    assert_eq!(
        queue.status, 201,
        "CreateQueue: expected HTTP 201, got {} with body {}",
        queue.status, queue.body
    );

    let job_settings = settings("s3://example-bucket/in/workflow.mp4", "_out");
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": "workflow-queue",
            "userMetadata": { "k": "v" },
            "settings": job_settings
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateJob: expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");

    let fetched = emulator.get_job(&id).await;
    assert_eq!(
        fetched.at("/job/settings"),
        &job_settings,
        "GetJob: Settings must match what was submitted, got {}",
        fetched.body
    );
    assert_eq!(
        fetched.at("/job/userMetadata"),
        &json!({ "k": "v" }),
        "GetJob: UserMetadata must match what was submitted, got {}",
        fetched.body
    );

    let listed = emulator
        .list_jobs("queue=workflow-queue&status=SUBMITTED&maxResults=1")
        .await;
    assert_eq!(
        listed.at("/jobs").as_array().map(Vec::len),
        Some(1),
        "ListJobs: expected the one job, got {}",
        listed.body
    );
    assert_eq!(
        listed.at("/nextToken"),
        &Value::Null,
        "ListJobs: no further page exists, so no NextToken, got {}",
        listed.body
    );

    let cancelled = emulator.cancel_job(&id).await;
    assert_eq!(
        cancelled.status, 202,
        "CancelJob: expected HTTP 202, got {} with body {}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        emulator.get_job(&id).await.string("/job/status"),
        "CANCELED",
        "GetJob after CancelJob must report CANCELED"
    );
}

/// A job whose input is the video this service GENERATES — `Input$VideoGenerator`,
/// "MediaConvert creates a video input with black frames… if you do not have a video input"
/// — runs just as one with an input file does: the frames are CODED, the output file is
/// written into the destination its group names, the job reaches `COMPLETE`, and each output
/// reports the duration and frame size of the file that was written.
///
/// The rows span what the rule generalises over: whether the generator states a frame size
/// or takes the default resolution its member documents, whether the output states both
/// dimensions, one of them or neither, and the frame rate the output is written at — the
/// duration is a whole number of THAT rate's frames.
#[tokio::test]
async fn a_job_whose_input_is_generated_video_runs_to_complete_and_reports_what_it_produced() {
    use super::media_probe::{create_bucket, BUCKET};

    /// One row: the generator the job's input states, the output's video description, and
    /// what the produced file must report.
    struct Case {
        case: &'static str,
        generator: Value,
        video: Option<Value>,
        /// The destination prefix and base name this row's output group writes to, and the
        /// `NameModifier` its one output carries.
        destination: &'static str,
        name_modifier: &'static str,
        duration_ms: i64,
        frame: (i64, i64),
        /// The key, inside the probe suite's bucket, the run must have written at.
        written: &'static str,
    }
    let cases = vec![
        Case {
            case: "a generated input written at 30 frames a second",
            generator: json!({ "duration": 1_000 }),
            video: Some(json!({
                "width": 320,
                "height": 240,
                "codecSettings": {
                    "codec": "H_264",
                    "h264Settings": {
                        "framerateNumerator": 30,
                        "framerateDenominator": 1
                    }
                }
            })),
            destination: "generated-a/movie",
            name_modifier: "_a",
            // 1 000ms of whole 30fps frames is 31 of them, which run 1 033ms.
            duration_ms: 1_033,
            frame: (320, 240),
            written: "generated-a/movie_a.mp4",
        },
        Case {
            case: "a generated input stating its own frame size, at the default rate",
            generator: json!({ "duration": 300, "width": 160, "height": 120 }),
            video: None,
            destination: "generated-b/movie",
            name_modifier: "_b",
            // The default generator frame rate is 29.97, so 300ms is 9 whole frames, which
            // run 300ms at that rate (9 x 1001 units of a 30 000 timescale).
            duration_ms: 300,
            frame: (160, 120),
            written: "generated-b/movie_b.mp4",
        },
        Case {
            case: "an output that states only its height, scaled from the generated frame",
            generator: json!({
                "duration": 200,
                "framerateNumerator": 25,
                "framerateDenominator": 1
            }),
            video: Some(json!({ "height": 120 })),
            destination: "generated-c/movie",
            name_modifier: "_c",
            // 200ms at 25fps is 6 frames, which run 240ms.
            duration_ms: 240,
            // The default generated resolution is 540x360, scaled evenly to a height of 120.
            frame: (180, 120),
            written: "generated-c/movie_c.mp4",
        },
        Case {
            case: "a generated input at a frame size large enough to be coded in slices",
            generator: json!({ "duration": 500 }),
            video: Some(json!({
                "width": 1_280,
                "height": 720,
                "codecSettings": {
                    "codec": "H_264",
                    "h264Settings": {
                        "framerateNumerator": 30,
                        "framerateDenominator": 1
                    }
                }
            })),
            destination: "generated-d/movie",
            name_modifier: "_d",
            // 500ms of whole 30fps frames is 16 of them, which run 533ms — one sample per
            // frame however the codec slices the picture.
            duration_ms: 533,
            frame: (1_280, 720),
            written: "generated-d/movie_d.mp4",
        },
    ];

    let emulator = Emulator::with_settle_span(SHORT_SETTLE)
        .mount(service_com_amazonaws_s3_amazons3::EmulatedService::new);
    create_bucket(&emulator).await;

    let mut ids = Vec::new();
    for case in &cases {
        let mut output = json!({
            "nameModifier": case.name_modifier,
            "containerSettings": { "container": "MP4" }
        });
        if let Some(video) = &case.video {
            output["videoDescription"] = video.clone();
        }
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "settings": {
                    "inputs": [{ "videoGenerator": case.generator }],
                    "outputGroups": [{
                        "name": "File Group",
                        "outputs": [output],
                        "outputGroupSettings": {
                            "type": "FILE_GROUP_SETTINGS",
                            "fileGroupSettings": {
                                "destination": format!("s3://{BUCKET}/{}", case.destination)
                            }
                        }
                    }]
                }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateJob must succeed, got {} with body {}",
            case.case, created.status, created.body
        );
        ids.push(created.string("/job/id"));
    }

    for (case, id) in cases.iter().zip(&ids) {
        // The CONDITION, not a span: each row reads the job again until its run has ended.
        let started = std::time::Instant::now();
        let read = loop {
            let read = emulator.get_job(id).await;
            let status = read.string("/job/status");
            if matches!(status.as_str(), "COMPLETE" | "ERROR" | "CANCELED") {
                break read;
            }
            assert!(
                started.elapsed() < RUN_DEADLINE,
                "case {:?}: the clock never ran the job to a terminal status; it is \
                 {status:?}",
                case.case
            );
            tokio::time::sleep(RUN_POLL_PAUSE).await;
        };
        assert_eq!(
            read.string("/job/status"),
            "COMPLETE",
            "case {:?}: a job whose input this service generates must reach COMPLETE, got {}",
            case.case,
            read.body
        );
        let output = read
            .at("/job/outputGroupDetails")
            .pointer("/0/outputDetails/0")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            output.pointer("/durationInMs").and_then(Value::as_i64),
            Some(case.duration_ms),
            "case {:?}: expected the produced file's duration of {}ms, got {output}",
            case.case,
            case.duration_ms
        );
        assert_eq!(
            output
                .pointer("/videoDetails/widthInPx")
                .and_then(Value::as_i64),
            Some(case.frame.0),
            "case {:?}: expected a width of {}px, got {output}",
            case.case,
            case.frame.0
        );
        assert_eq!(
            output
                .pointer("/videoDetails/heightInPx")
                .and_then(Value::as_i64),
            Some(case.frame.1),
            "case {:?}: expected a height of {}px, got {output}",
            case.case,
            case.frame.1
        );

        // And the file itself: a job that reports COMPLETE has written its output, so it is
        // read back out of the S3 that owns it and must be an ISO base media file carrying
        // coded video, under the media type of its container and carrying the id of the job
        // that produced it.
        let (status, headers, body) = emulator
            .rest(
                "s3",
                "GET",
                &format!("/{BUCKET}/{}", case.written),
                Vec::new(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: the output {:?} the job reported must be in the bucket, got {status} {}",
            case.case,
            case.written,
            String::from_utf8_lossy(&body)
        );
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(
            header("content-type"),
            "video/mp4",
            "case {:?}: the output is stored under the media type of its container",
            case.case
        );
        assert_eq!(
            header("x-amz-meta-mediaconvert-jobid"),
            *id,
            "case {:?}: the output records the id of the job that produced it",
            case.case
        );
        assert_eq!(
            body.get(4..8),
            Some(b"ftyp".as_slice()),
            "case {:?}: the output must be an ISO base media file, got {} bytes starting {:?}",
            case.case,
            body.len(),
            body.get(..8)
        );
        assert!(
            body.windows(4).any(|window| window == b"avc1"),
            "case {:?}: the output must carry a coded AVC video track, got {} bytes",
            case.case,
            body.len()
        );
    }
}

/// One row of the run table: the media a job is given, the settings it is submitted with,
/// what its `OutputGroupDetails` must report, and the object keys its run must have written.
struct RunCase {
    case: &'static str,
    /// The key this row's input media is written at, inside the probe suite's bucket.
    key: String,
    /// The input file's own bytes, muxed by this crate's test muxer.
    media: Vec<u8>,
    /// The `Settings` the job is submitted with.
    settings: Value,
    /// How long the input runs, in milliseconds, from the tracks it was muxed with: what
    /// every output of this row must report as its own duration, since the run carries the
    /// input's samples into the output.
    duration_ms: i64,
    /// One entry per output group, each holding one `(width, height)` per output — `None`
    /// for an output whose produced file carries no video at all.
    frames: Vec<Vec<Option<(i64, i64)>>>,
    /// The keys, inside the same bucket, the run must have written its outputs at.
    written: Vec<String>,
}

/// How far a produced file's reported duration may differ from the input's.
///
/// A container states its duration in a timescale, so the value is exact to that
/// timescale's resolution rather than to the millisecond; it is NOT a tolerance on time
/// passing, which nothing here waits for.
const DURATION_TOLERANCE_MS: i64 = 60;

/// A job whose settings describe a transcode this service can perform RUNS: it leaves
/// `SUBMITTED`, settles on `COMPLETE`, has written one output file per output into the
/// destination its group names, and reports one `outputGroupDetails` entry per output group
/// whose `outputDetails` carry the duration and the frame size of the file that output
/// produced.
///
/// The rows span what the rule generalises over: how many output groups a job has and how
/// many outputs each of them has, whether an output asks for a frame size of its own or
/// takes the input's, which tracks the input carries (video and audio, video alone, audio
/// alone), how long the input runs, and whether the destination names a place or a base
/// name.
///
/// The media is REAL: each row's input is muxed by the sibling `media_probe` suite's ISO
/// base media writer, written into a bucket of an S3 mounted beside MediaConvert on one
/// engine, and the produced outputs are read back out of that same S3 — so what is asserted
/// is a file the run really wrote.
#[tokio::test]
async fn a_processable_job_runs_to_complete_and_reports_what_it_produced() {
    use super::media_probe::{
        create_bucket, iso_bmff, put_object, s3_uri, AudioTrack, VideoTrack, BUCKET,
    };

    /// A video track of `frames` samples at 30 fps, in the geometry given.
    fn video(width: u16, height: u16, frames: u32) -> VideoTrack {
        VideoTrack {
            width,
            height,
            timescale: 30_000,
            frames,
            sample_duration: 1_000,
        }
    }

    /// An AAC track of `frames` frames at 48 kHz, stereo.
    fn audio(frames: u32) -> AudioTrack {
        AudioTrack {
            sample_rate: ::mp4::SampleFreqIndex::Freq48000,
            channels: ::mp4::ChannelConfig::Stereo,
            frames,
            samples_per_frame: 1_024,
        }
    }

    /// One output group writing to `destination`, holding the outputs given.
    fn group(destination: &str, outputs: Value) -> Value {
        json!({
            "name": "File Group",
            "outputs": outputs,
            "outputGroupSettings": {
                "type": "FILE_GROUP_SETTINGS",
                "fileGroupSettings": { "destination": destination }
            }
        })
    }

    /// One output in the MP4 container, with the name modifier given and, when it asks for
    /// one, its own frame size.
    fn output(name_modifier: &str, frame: Option<(i64, i64)>) -> Value {
        let mut output = json!({
            "nameModifier": name_modifier,
            "containerSettings": { "container": "MP4" }
        });
        if let Some((width, height)) = frame {
            output["videoDescription"] = json!({ "width": width, "height": height });
        }
        output
    }

    let bucket_uri = |prefix: &str| format!("s3://{BUCKET}/{prefix}");
    let cases = vec![
        RunCase {
            case: "one output taking the input's own frame size",
            key: "run/plain.mp4".to_string(),
            media: iso_bmff(b"isom", Some(video(1280, 720, 60)), Some(audio(94))),
            settings: json!({
                "inputs": [{ "fileInput": s3_uri("run/plain.mp4") }],
                "outputGroups": [group(&bucket_uri("plain-out/"), json!([output("_out", None)]))]
            }),
            // 94 AAC frames of 1024 samples at 48 kHz is the longer of the two tracks.
            duration_ms: 2_005,
            frames: vec![vec![Some((1280, 720))]],
            written: vec!["plain-out/plain_out.mp4".to_string()],
        },
        RunCase {
            case: "an output that asks for a frame size of its own",
            key: "run/sized.mp4".to_string(),
            media: iso_bmff(b"isom", Some(video(1920, 1080, 30)), None),
            settings: json!({
                "inputs": [{ "fileInput": s3_uri("run/sized.mp4") }],
                "outputGroups": [group(
                    &bucket_uri("sized-out/"),
                    json!([output("_small", Some((640, 360)))]),
                )]
            }),
            // 30 video samples of 1000 units at a 30 000 timescale is one second.
            duration_ms: 1_000,
            frames: vec![vec![Some((640, 360))]],
            written: vec!["sized-out/sized_small.mp4".to_string()],
        },
        RunCase {
            case: "two outputs in one group, one of them resized",
            key: "run/two.mp4".to_string(),
            media: iso_bmff(b"isom", Some(video(1280, 720, 90)), None),
            settings: json!({
                "inputs": [{ "fileInput": s3_uri("run/two.mp4") }],
                "outputGroups": [group(
                    &bucket_uri("two-out/"),
                    json!([output("_hd", None), output("_sd", Some((854, 480)))]),
                )]
            }),
            duration_ms: 3_000,
            frames: vec![vec![Some((1280, 720)), Some((854, 480))]],
            written: vec![
                "two-out/two_hd.mp4".to_string(),
                "two-out/two_sd.mp4".to_string(),
            ],
        },
        RunCase {
            case: "two output groups, each with its own destination",
            key: "run/groups.mp4".to_string(),
            media: iso_bmff(b"isom", Some(video(640, 480, 60)), None),
            settings: json!({
                "inputs": [{ "fileInput": s3_uri("run/groups.mp4") }],
                "outputGroups": [
                    group(&bucket_uri("groups-a/"), json!([output("_a", None)])),
                    group(&bucket_uri("groups-b/"), json!([output("_b", Some((320, 240)))])),
                ]
            }),
            duration_ms: 2_000,
            frames: vec![vec![Some((640, 480))], vec![Some((320, 240))]],
            written: vec![
                "groups-a/groups_a.mp4".to_string(),
                "groups-b/groups_b.mp4".to_string(),
            ],
        },
        RunCase {
            case: "an input carrying audio alone, so the output has no video stream",
            key: "run/audio.mp4".to_string(),
            media: iso_bmff(b"isom", None, Some(audio(47))),
            settings: json!({
                "inputs": [{ "fileInput": s3_uri("run/audio.mp4") }],
                "outputGroups": [group(&bucket_uri("audio-out/"), json!([output("_a", None)]))]
            }),
            // 47 frames of 1024 samples at 48 kHz.
            duration_ms: 1_002,
            frames: vec![vec![None]],
            written: vec!["audio-out/audio_a.mp4".to_string()],
        },
        RunCase {
            case: "a destination that names a base name rather than a place",
            key: "run/named.mp4".to_string(),
            media: iso_bmff(b"isom", Some(video(1280, 720, 15)), None),
            settings: json!({
                "inputs": [{ "fileInput": s3_uri("run/named.mp4") }],
                "outputGroups": [group(&bucket_uri("named-out/movie"), json!([output("_out", None)]))]
            }),
            duration_ms: 500,
            frames: vec![vec![Some((1280, 720))]],
            written: vec!["named-out/movie_out.mp4".to_string()],
        },
    ];

    let emulator = Emulator::with_settle_span(SHORT_SETTLE)
        .mount(service_com_amazonaws_s3_amazons3::EmulatedService::new);
    create_bucket(&emulator).await;

    // Every input is written and every job submitted first, so the jobs run beside one
    // another rather than one row at a time.
    let mut ids = Vec::new();
    for case in &cases {
        put_object(&emulator, &case.key, &case.media).await;
        let created = emulator
            .create_job(json!({ "role": ROLE, "settings": case.settings }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateJob must succeed, got {} with body {}",
            case.case, created.status, created.body
        );
        assert_eq!(
            created.string("/job/status"),
            "SUBMITTED",
            "case {:?}: a job a caller has just submitted is SUBMITTED, got {}",
            case.case,
            created.body
        );
        ids.push(created.string("/job/id"));
    }

    for (case, id) in cases.iter().zip(&ids) {
        // The CONDITION, not a span: the due instants are stored and never slept for by a
        // handler, so each row reads the job again until its run has ended.
        let started = std::time::Instant::now();
        let read = loop {
            let read = emulator.get_job(id).await;
            let status = read.string("/job/status");
            if matches!(status.as_str(), "COMPLETE" | "ERROR" | "CANCELED") {
                break read;
            }
            assert!(
                started.elapsed() < RUN_DEADLINE,
                "case {:?}: the clock never ran the job to a terminal status; it is \
                 {status:?}",
                case.case
            );
            tokio::time::sleep(RUN_POLL_PAUSE).await;
        };
        assert_eq!(
            read.string("/job/status"),
            "COMPLETE",
            "case {:?}: a job whose transcode this service can perform must reach COMPLETE, \
             got {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.at("/job/errorCode"),
            &Value::Null,
            "case {:?}: a job that completed failed at nothing, so it carries no ErrorCode, \
             got {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.at("/job/errorMessage"),
            &Value::Null,
            "case {:?}: a job that completed carries no ErrorMessage, got {}",
            case.case,
            read.body
        );
        assert!(
            read.at("/job/timing/finishTime").is_number(),
            "case {:?}: a job that has run reports when it finished, got {}",
            case.case,
            read.body
        );
        assert!(
            read.at("/job/warnings").is_array(),
            "case {:?}: the warnings container is always reported, got {}",
            case.case,
            read.body
        );

        // One details entry per output group, one output entry per output, each carrying
        // what the produced file measures.
        let groups = read
            .at("/job/outputGroupDetails")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            groups.len(),
            case.frames.len(),
            "case {:?}: expected one outputGroupDetails entry per output group, got {}",
            case.case,
            read.body
        );
        for (index, (group, expected)) in groups.iter().zip(&case.frames).enumerate() {
            let outputs = group
                .pointer("/outputDetails")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                outputs.len(),
                expected.len(),
                "case {:?} group {index}: expected one outputDetails entry per output, got \
                 {group}",
                case.case
            );
            for (position, (output, frame)) in outputs.iter().zip(expected).enumerate() {
                let duration = output
                    .pointer("/durationInMs")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                assert!(
                    (duration - case.duration_ms).abs() <= DURATION_TOLERANCE_MS,
                    "case {:?} group {index} output {position}: expected the produced \
                     file's duration of about {}ms, got {duration}ms in {output}",
                    case.case,
                    case.duration_ms
                );
                let reported = output.pointer("/videoDetails").map(|video| {
                    (
                        video
                            .pointer("/widthInPx")
                            .and_then(Value::as_i64)
                            .unwrap_or_default(),
                        video
                            .pointer("/heightInPx")
                            .and_then(Value::as_i64)
                            .unwrap_or_default(),
                    )
                });
                assert_eq!(
                    reported, *frame,
                    "case {:?} group {index} output {position}: expected the produced \
                     file's frame size {frame:?}, got {reported:?} in {output}",
                    case.case
                );
            }
        }

        // And the files themselves: a job that reports COMPLETE has written its outputs,
        // so each one is read back out of the S3 that OWNS it, and what comes back is an
        // ISO base media file.
        for key in &case.written {
            let (status, _, body) = emulator
                .rest("s3", "GET", &format!("/{BUCKET}/{key}"), Vec::new())
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: the output {key:?} the job reported must be in the bucket, got \
                 {status} {}",
                case.case,
                String::from_utf8_lossy(&body)
            );
            assert_eq!(
                body.get(4..8),
                Some(b"ftyp".as_slice()),
                "case {:?}: the output {key:?} must be an ISO base media file, got {} bytes \
                 starting {:?}",
                case.case,
                body.len(),
                body.get(..8)
            );
        }
    }
}

/// The status the end of a job's run reaches is decided by what that run ESTABLISHED, so the
/// finish is applied only once the run has been MADE: while the run is still owed to the job
/// and nothing has established what it produced, the clock owes the finish again a step later
/// rather than resolving it as a failure. A job whose transcode nothing has attempted yet has
/// not failed at it.
///
/// The driver that makes those runs makes a bounded number of them per pass, so a job
/// submitted beside others is routinely settled before its own run has been claimed — which
/// must not report it in `ERROR`.
///
/// The rows span what the rule generalises over: HOW MANY runs the store is owed at the same
/// instant — one job alone, a handful, and more than twice as many as any one pass of the
/// driver makes, so they cover both the job whose run is made first and the jobs whose runs
/// are left for a later pass — and WHEN the finish comes due relative to those runs, a span
/// the driver comfortably fits inside and one so short that every job's finish is already due
/// at the instant its run is owed. Both spans are pinned STATE, not a race: whatever the
/// finish is owed at, it lands on the run's result and never ahead of it.
///
/// Nothing here polls the service while it waits: each row waits on the STORE the handlers
/// write to ([`Emulator::await_run`]), so the only thing that can make these runs is the
/// store's own clock pump — exactly the client that pump exists for, one that submits a job
/// and then makes no further MediaConvert call. The media is REAL, muxed by the `media_probe`
/// suite's ISO base media writer into a bucket of an S3 mounted beside MediaConvert on one
/// engine, and every produced file is read back out of that same S3.
#[tokio::test]
async fn every_job_owed_a_run_at_once_waits_for_its_own_run() {
    use super::media_probe::{create_bucket, iso_bmff, put_object, s3_uri, VideoTrack, BUCKET};

    /// The input every job of a row reads: one small video track, so a row costs the number
    /// of jobs it submits rather than the size of what each of them carries.
    fn media() -> Vec<u8> {
        iso_bmff(
            b"isom",
            Some(VideoTrack {
                width: 320,
                height: 240,
                timescale: 30_000,
                frames: 10,
                sample_duration: 1_000,
            }),
            None,
        )
    }

    /// A settle span so short that a job's finish is due at the very instant the job begins,
    /// which is the instant its run starts being owed: the run cannot have been made yet, so
    /// every row pinned to this span settles a finish over a result that is not in.
    const FINISH_DUE_AT_ONCE: Duration = Duration::from_millis(2);

    // (case, how many jobs are submitted together before any of them is read, the span the
    // whole run is owed in)
    let rows: [(&str, usize, Duration); 6] = [
        (
            "one job, whose run is the first the driver makes",
            1,
            SHORT_SETTLE,
        ),
        ("a handful of jobs submitted together", 5, SHORT_SETTLE),
        (
            "more jobs at once than one pass of the driver runs, twice over",
            9,
            SHORT_SETTLE,
        ),
        (
            "one job whose finish is due the instant its run is owed",
            1,
            FINISH_DUE_AT_ONCE,
        ),
        (
            "a handful of jobs whose finishes are due the instant their runs are owed",
            5,
            FINISH_DUE_AT_ONCE,
        ),
        (
            "more jobs than one pass runs, every finish due the instant its run is owed",
            9,
            FINISH_DUE_AT_ONCE,
        ),
    ];

    for (case, count, span) in rows {
        // Each row gets its own emulator, so the jobs it counts are the only runs that store
        // is owed.
        let emulator = Emulator::with_settle_span(span)
            .mount(service_com_amazonaws_s3_amazons3::EmulatedService::new);
        create_bucket(&emulator).await;

        // Every input is written and every job submitted BEFORE any of them is read, so the
        // store comes to owe `count` runs at the same instant.
        let mut jobs = Vec::new();
        for index in 0..count {
            let key = format!("many/{index}.mp4");
            put_object(&emulator, &key, &media()).await;
            let created = emulator
                .create_job(json!({
                    "role": ROLE,
                    "settings": {
                        "inputs": [{ "fileInput": s3_uri(&key) }],
                        "outputGroups": [{
                            "name": "File Group",
                            "outputs": [{
                                "nameModifier": "_out",
                                "containerSettings": { "container": "MP4" }
                            }],
                            "outputGroupSettings": {
                                "type": "FILE_GROUP_SETTINGS",
                                "fileGroupSettings": {
                                    "destination": format!("s3://{BUCKET}/many-out-{index}/")
                                }
                            }
                        }]
                    }
                }))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?} job {index}: CreateJob must succeed, got {} with body {}",
                created.status, created.body
            );
            jobs.push(created.string("/job/id"));
        }

        for (index, id) in jobs.iter().enumerate() {
            // The CONDITION, not a span, and read off the store rather than over the wire:
            // the due instants are stored and never slept for by a handler, and reading the
            // store makes no MediaConvert request, so the wait itself cannot be what makes
            // the run.
            emulator.await_run(&format!("{case}, job {index}"), id).await;
            let read = emulator.get_job(id).await;
            assert_eq!(
                read.string("/job/status"),
                "COMPLETE",
                "case {case:?} job {index}: a job whose transcode this service can perform \
                 must reach COMPLETE however many runs the store was owed at once, got {}",
                read.body
            );
            assert_eq!(
                read.at("/job/errorMessage"),
                &Value::Null,
                "case {case:?} job {index}: a job that completed failed at nothing, so it \
                 carries no ErrorMessage, got {}",
                read.body
            );
            assert_eq!(
                read.at("/job/errorCode"),
                &Value::Null,
                "case {case:?} job {index}: a job that completed carries no ErrorCode, got \
                 {}",
                read.body
            );
            // And the status is backed by the file the run really wrote, read back out of
            // the S3 that owns it.
            let key = format!("many-out-{index}/{index}_out.mp4");
            let (status, _, body) = emulator
                .rest("s3", "GET", &format!("/{BUCKET}/{key}"), Vec::new())
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} job {index}: the completed job's output {key:?} must be in \
                 the destination it named, got {status} {}",
                String::from_utf8_lossy(&body)
            );
            assert_eq!(
                body.get(4..8),
                Some(b"ftyp".as_slice()),
                "case {case:?} job {index}: the output {key:?} must be an ISO base media \
                 file, got {} bytes starting {:?}",
                body.len(),
                body.get(..8)
            );
        }
    }
}

/// A job's finish is never resolved over a run that has not been made.
///
/// RULE: `JobStatus` is what a job's run REACHED and `Timing$FinishTime` is "the time ...
/// that the transcoding job finished", so the end of a job's run is owed only once something
/// has established what that run produced: a job whose transcode nothing has attempted yet
/// has not finished, and its finish is owed again a step later. WHICH status it then reaches
/// is decided by that job's own run and by nothing else — least of all by which driver of
/// the store's clock happened to be alive at the instant somebody read it.
///
/// One pass of the driver makes a bounded number of runs, so a store owed more runs at once
/// than one pass makes always holds jobs whose finish comes due with no result in. This case
/// drives exactly such a store: first the settle a reporting door makes, with no pump
/// running and no run made, which must put every finish off rather than resolve it; and then
/// reads over the wire about ANOTHER job — a drain covers the whole already-due prefix, so
/// one read settles every job the clock owed and not only the one it is about — which must
/// leave every job reaching `COMPLETE` with the file its run wrote.
///
/// The rows span what the rule generalises over: how many runs the store is owed at the same
/// instant, from one, through the bound one pass makes, to well past it.
///
/// The jobs are stored through the store's own submit path (`accept_job`, which arms the
/// clock exactly as `CreateJob` does) rather than over the wire, and only once this store's
/// clock pump has stood down: that is what makes their finishes come due with NO pump
/// running, which is the state a pump that ended abnormally leaves behind and the one a
/// machine under load reaches. The media is REAL, as in the case above.
#[tokio::test]
async fn a_finish_that_comes_due_before_its_run_waits_for_the_run() {
    use service_com_amazonaws_mediaconvert_mediaconvert::state::AcceptOutcome;
    use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState as Store;

    use super::media_probe::{create_bucket, iso_bmff, put_object, s3_uri, VideoTrack, BUCKET};

    /// A settle span so short that a job's finish is due at the very instant it begins,
    /// which is the instant its run starts being owed: the run cannot have been made yet, so
    /// every job below settles a finish over a result that is not in.
    const FINISH_DUE_AT_ONCE: Duration = Duration::from_millis(2);

    /// The input every job of a row reads: one small video track, so a row costs the number
    /// of jobs it stores rather than the size of what each of them carries.
    fn media() -> Vec<u8> {
        iso_bmff(
            b"isom",
            Some(VideoTrack {
                width: 320,
                height: 240,
                timescale: 30_000,
                frames: 10,
                sample_duration: 1_000,
            }),
            None,
        )
    }

    /// Whether a pump is driving this store's clock right now.
    fn pump_is_running(emulator: &Emulator) -> bool {
        emulator.engine.state::<Store>().lock().clock_pump_is_running()
    }

    /// What the store holds for the job `id`: its status and the reason it carries, so a row
    /// that finds the wrong status can report what the store said about it.
    fn stored(emulator: &Emulator, id: &str) -> (String, Option<String>) {
        emulator
            .engine
            .state::<Store>()
            .lock()
            .jobs
            .get(id)
            .map(|job| (job.status.clone(), job.error_message.clone()))
            .unwrap_or_default()
    }

    /// Where every job of a row writes its output, and the key the produced file lands
    /// under: the destination names a PLACE, so the output is named after the input file.
    const DESTINATION: &str = "finish-waits-out";
    const INPUT_KEY: &str = "finish-waits/input.mp4";

    // (case, how many runs the store is owed at the same instant)
    let rows: [(&str, usize); 4] = [
        ("one run owed", 1),
        ("as many runs as one pass of the driver makes", 4),
        ("two more runs than one pass makes", 6),
        ("more than twice what one pass makes", 9),
    ];

    for (case, count) in rows {
        let emulator = Emulator::with_settle_span(FINISH_DUE_AT_ONCE)
            .mount(service_com_amazonaws_s3_amazons3::EmulatedService::new);
        create_bucket(&emulator).await;

        // One job submitted over the wire, so there is a stored record to submit `count`
        // more like it — and so this store's clock pump starts, runs that job and stands
        // down again.
        put_object(&emulator, INPUT_KEY, &media()).await;
        let seeded = emulator
            .create_job(json!({
                "role": ROLE,
                "settings": {
                    "inputs": [{ "fileInput": s3_uri(INPUT_KEY) }],
                    "outputGroups": [{
                        "name": "File Group",
                        "outputs": [{
                            "nameModifier": "_out",
                            "containerSettings": { "container": "MP4" }
                        }],
                        "outputGroupSettings": {
                            "type": "FILE_GROUP_SETTINGS",
                            "fileGroupSettings": {
                                "destination": format!("s3://{BUCKET}/{DESTINATION}/")
                            }
                        }
                    }]
                }
            }))
            .await;
        assert_eq!(
            seeded.status, 201,
            "case {case:?}: CreateJob must succeed, got {} with {}",
            seeded.status, seeded.body
        );
        let seeded_id = seeded.string("/job/id");
        emulator.await_run(case, &seeded_id).await;

        // The CONDITION the rows need: a store NO pump is driving. A pump stands down inside
        // the critical section that observes this store owing nothing, so whether one is
        // running is owned by the store and read from it.
        let started = std::time::Instant::now();
        while pump_is_running(&emulator) {
            assert!(
                started.elapsed() < RUN_DEADLINE,
                "case {case:?}: the clock pump never stood down, so the jobs below would not \
                 settle a finish with no pump running"
            );
            tokio::time::sleep(RUN_POLL_PAUSE).await;
        }

        // `count` jobs stored through the store's own submit path, which arms each one's
        // exit from `SUBMITTED` exactly as `CreateJob` does. Nothing starts a pump for them,
        // so their finishes come due with no driver having made their runs.
        let mut ids = Vec::new();
        {
            let state = emulator.engine.state::<Store>();
            let mut guard = state.lock();
            let record = guard
                .jobs
                .get(&seeded_id)
                .cloned()
                .expect("the job submitted above is in the store");
            for index in 0..count {
                let mut submitted = record.clone();
                // A newly submitted job: nothing has run it and nothing is recorded about
                // it, which is how `CreateJob` stores one.
                submitted.status = "SUBMITTED".to_string();
                submitted.run = None;
                submitted.run_claimed_at = None;
                submitted.client_request_token = None;
                submitted.timing.start_time = None;
                submitted.timing.finish_time = None;
                submitted.error_code = None;
                submitted.error_message = None;
                match guard.accept_job(submitted, std::time::SystemTime::now()) {
                    AcceptOutcome::Stored(id) => ids.push(id),
                    other => panic!("case {case:?}: job {index} was not stored: {other:?}"),
                }
            }
        }

        // The write a reporting door makes: materialise what the clock had already made due
        // at this instant. No pump is running and no run has been made, so this is the
        // settle that must put each finish off rather than resolve it over a result that is
        // not in.
        let started = std::time::Instant::now();
        loop {
            emulator
                .engine
                .state::<Store>()
                .lock()
                .settle(std::time::SystemTime::now());
            let statuses: Vec<(String, Option<String>)> =
                ids.iter().map(|id| stored(&emulator, id)).collect();
            for (index, (status, reason)) in statuses.iter().enumerate() {
                assert_ne!(
                    status, "ERROR",
                    "case {case:?}: job {index} was failed while its run was still to come, \
                     with {reason:?}"
                );
            }
            if statuses.iter().all(|(status, _)| status != "SUBMITTED") {
                assert!(
                    statuses.iter().all(|(status, _)| status == "PROGRESSING"),
                    "case {case:?}: a job that has begun and whose run is still to come is \
                     PROGRESSING, got {statuses:?}"
                );
                break;
            }
            assert!(
                started.elapsed() < RUN_DEADLINE,
                "case {case:?}: the clock never let every job begin; they are {statuses:?}"
            );
            tokio::time::sleep(RUN_POLL_PAUSE).await;
        }

        // And every one of them still runs to COMPLETE: reads about the job that ALREADY ran
        // settle the store exactly as the loop above did, and the runs those reads leave to
        // the clock are made rather than lost.
        let started = std::time::Instant::now();
        loop {
            let read = emulator.get_job(&seeded_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetJob must answer 200, got {} with {}",
                read.status, read.body
            );
            let statuses: Vec<(String, Option<String>)> =
                ids.iter().map(|id| stored(&emulator, id)).collect();
            for (index, (status, reason)) in statuses.iter().enumerate() {
                assert_ne!(
                    status, "ERROR",
                    "case {case:?}: job {index} was failed while its run was still to come, \
                     with {reason:?}"
                );
            }
            if statuses.iter().all(|(status, _)| status == "COMPLETE") {
                break;
            }
            assert!(
                started.elapsed() < RUN_DEADLINE,
                "case {case:?}: the clock never ran every job to COMPLETE; they are \
                 {statuses:?}"
            );
            tokio::time::sleep(RUN_POLL_PAUSE).await;
        }

        // Every one of them reports a run that produced what its settings describe ...
        for (index, id) in ids.iter().enumerate() {
            let read = emulator.get_job(id).await;
            assert_eq!(
                read.string("/job/status"),
                "COMPLETE",
                "case {case:?} job {index}: a job whose transcode this service can perform \
                 must reach COMPLETE, got {}",
                read.body
            );
            assert_eq!(
                read.at("/job/errorCode"),
                &Value::Null,
                "case {case:?} job {index}: a job that completed failed at nothing, so it \
                 carries no ErrorCode, got {}",
                read.body
            );
            assert_eq!(
                read.at("/job/errorMessage"),
                &Value::Null,
                "case {case:?} job {index}: a job that completed carries no ErrorMessage, \
                 got {}",
                read.body
            );
            assert!(
                read.at("/job/timing/finishTime").is_number(),
                "case {case:?} job {index}: a job that has run reports when it finished, got \
                 {}",
                read.body
            );
        }

        // ... and the status is backed by the file those runs really wrote, read back out of
        // the S3 that owns it.
        let object = format!("{DESTINATION}/input_out.mp4");
        let (status, _, body) = emulator
            .rest("s3", "GET", &format!("/{BUCKET}/{object}"), Vec::new())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the completed jobs' output {object:?} must be in the destination \
             they named, got {status} {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            body.get(4..8),
            Some(b"ftyp".as_slice()),
            "case {case:?}: the output {object:?} must be an ISO base media file, got {} \
             bytes starting {:?}",
            body.len(),
            body.get(..8)
        );
    }
}

/// `Job$ElementalInferenceSettings` — "The Elemental Inference settings used in this job"
/// — is a member the service applies no value of its own for: `CreateJob` persists the
/// settings the request states, every reader of the job answers exactly those, and a
/// request that states none leaves the member absent rather than filling one in.
///
/// The table spans the forms the member takes ([`elemental_inference_forms`]): absent, an
/// empty structure, a source naming no feed, and a source naming each of two different
/// feeds.
#[tokio::test]
async fn the_elemental_inference_settings_a_job_states_are_stored_and_read_back() {
    let emulator = Emulator::new();
    for (index, (case, stated)) in elemental_inference_forms().into_iter().enumerate() {
        let mut request = json!({
            "role": ROLE,
            "settings": settings(&format!("s3://example-bucket/in/inference-{index}.mp4"), "_i")
        });
        if !stated.is_null() {
            request["elementalInferenceSettings"] = stated.clone();
        }

        let created = emulator.create_job(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("/job/elementalInferenceSettings"),
            &stated,
            "case {case:?}: expected the stated settings {stated} back, got {}",
            created.body
        );

        let id = created.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJob HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/job/elementalInferenceSettings"),
            &stated,
            "case {case:?}: GetJob must answer what CreateJob persisted, expected {stated}, \
             got {}",
            fetched.body
        );
    }
}

/// A feed ARN outside the pattern `ElementalInferenceFeedConfigurationSource$FeedArn`
/// publishes is the modelled `BadRequestException`, and the refused submission stores no
/// job — while the same request carrying a feed ARN inside that pattern is accepted.
#[tokio::test]
async fn a_feed_arn_outside_its_published_pattern_is_a_bad_request() {
    let emulator = Emulator::new();
    let stored = || {
        emulator
            .engine
            .state::<service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .jobs
            .len()
    };

    for (case, arn) in malformed_feed_arns() {
        let before = stored();
        let reply = emulator
            .create_job(json!({
                "role": ROLE,
                "elementalInferenceSettings": { "feedConfigurationSource": { "feedArn": arn } },
                "settings": settings("s3://example-bucket/in/malformed.mp4", "_m")
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            stored(),
            before,
            "case {case:?}: a refused submission must store no job, got {} stored where \
             {before} were",
            stored()
        );
    }

    // The published form — an identifier of exactly nineteen characters from the pattern's
    // own set — is accepted, so the refusals above are the value and not the member.
    let accepted = emulator
        .create_job(json!({
            "role": ROLE,
            "elementalInferenceSettings": {
                "feedConfigurationSource": { "feedArn": feed_arn("abcdefghij123456789") }
            },
            "settings": settings("s3://example-bucket/in/accepted.mp4", "_a")
        }))
        .await;
    assert_eq!(
        accepted.status, 201,
        "a feed ARN inside the published pattern must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
}

