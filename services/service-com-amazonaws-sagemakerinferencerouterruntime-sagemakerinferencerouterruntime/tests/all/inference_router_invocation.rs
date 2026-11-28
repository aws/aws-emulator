// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the inference router's two runtime routes.
//!
//! Every case goes over real HTTP through the same `MultiServiceRouter` the production router
//! assembles, with this service mounted on a shared engine, so what the tests exercise is
//! what an SDK client gets. The routers, their routing targets and the destinations each
//! target lists are written through this service's own registration seam
//! (`state::put_router` / `state::put_routing_target`), because the model binds no operation
//! that creates one.
//!
//! The assertions are RULES over TABLES of inputs, never one recorded call:
//!
//! * an invocation resolves the router and the routing target it names in the registry, and a
//!   name the registry does not hold is refused (`ValidationException`, HTTP 400);
//! * which destination takes a request is chosen from the target's own destination list, and
//!   successive requests are DISTRIBUTED over the destinations that can take them, because
//!   the selection reads and advances a cursor held in state;
//! * the reason reported is the one that selection decided, from the vocabulary the output
//!   member documents: the router's own Region first, then one other Region, then two or
//!   more, then a destination tried anyway when none can take the request;
//! * everything the response reports about the destination — its ARN, its production variant,
//!   the body, the content type and the custom attributes — is the CHOSEN destination's own,
//!   so two destinations of one target are answered differently;
//! * a destination that cannot take the request is not chosen while another can, and when
//!   none can the invocation is refused `ModelNotReadyException` (HTTP 429);
//! * an asynchronous submission is recorded under the `InferenceId` it reports before it
//!   answers 202, and the locations it reports are derived from the routing target's own
//!   configured Amazon S3 paths.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::Value;
use service_com_amazonaws_sagemakerinferencerouterruntime_sagemakerinferencerouterruntime::state::{
    select_destination, AsyncSubmissionStatus, Destination, InvocationOutcome, RoutingTarget,
    ASYNC_OUTCOME_DUE_AFTER, DEFAULT_INVOCATION_TIMEOUT_SECONDS, DEFAULT_REQUEST_TTL_SECONDS,
    NO_ASYNC_INFERENCE, REASON_CAPACITY_AVAILABLE, REASON_FAILOVER, REASON_LOCAL_PREFERRED,
    REASON_ONLY_HEALTHY,
};
use service_com_amazonaws_sagemakerinferencerouterruntime_sagemakerinferencerouterruntime::ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState as RouterState;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the routers in these tests are in, and the one every request is signed for.
const REGION: &str = "us-east-1";

/// A second Region, for the destinations a case places outside the router's own — which is
/// what the `only-healthy` and `capacity-available` reasons are about.
const OTHER_REGION: &str = "us-west-2";

/// The account segment every emulator ARN carries, from the vendored constant so an ARN
/// assertion cannot drift from it.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The ARN of the SageMaker endpoint `name` in `region`, as a routing target's destination
/// names the resource it routes to.
fn endpoint_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{name}")
}

/// A SigV4 credential scope naming this service's signing name: it is what routes a request
/// here. The key id and signature are not credentials — this harness turns request
/// authentication off, so nothing is verified.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One HTTP exchange with this data plane.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The error SHAPE the answer names, as restJson1 sets it in `x-amzn-errortype`.
    fn error_code(&self) -> String {
        let named = match self.headers.get("x-amzn-errortype") {
            Some(value) => value.to_str().unwrap_or_default().to_string(),
            None => self
                .json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        named.split(':').next().unwrap_or_default().to_string()
    }

    fn message(&self) -> String {
        let rendered = self.json();
        rendered
            .get("message")
            .or_else(|| rendered.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    fn routed_resource(&self) -> String {
        self.header("x-amzn-sagemaker-invoked-routed-resource-arn")
    }

    fn routing_reason(&self) -> String {
        self.header("x-amzn-sagemaker-invoked-routing-reason")
    }
}

/// One emulator: this service mounted on a shared engine behind the production router.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemakerinferencerouterruntime_sagemakerinferencerouterruntime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Register the router `router` in [`REGION`], holding the routing target `target` over
    /// `destinations`, writing its asynchronous results under `paths`.
    fn given_router(
        &self,
        router: &str,
        target: &str,
        destinations: Vec<Destination>,
        paths: Option<(&str, Option<&str>)>,
    ) -> String {
        let state = self.engine.state::<RouterState>();
        let mut store = state.lock();
        let arn = store.put_router(REGION, router);
        let mut routing_target = RoutingTarget::new(target, destinations);
        if let Some((output_path, failure_path)) = paths {
            routing_target =
                routing_target.writing_to(output_path, failure_path.map(str::to_string));
        }
        assert!(
            store.put_routing_target(router, routing_target),
            "the routing target {target} must be registered on {router}"
        );
        arn
    }

    /// `POST /inference-routers/{router}/invocations` with `headers` and the raw `body`.
    async fn invoke(&self, router: &str, headers: &[(&str, &str)], body: &[u8]) -> Call {
        self.post(
            &format!("/inference-routers/{router}/invocations"),
            headers,
            body,
        )
        .await
    }

    /// `POST /inference-routers/{router}/async-invocations` with `headers`.
    async fn submit(&self, router: &str, headers: &[(&str, &str)]) -> Call {
        self.post(
            &format!("/inference-routers/{router}/async-invocations"),
            headers,
            &[],
        )
        .await
    }

    async fn post(&self, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Call {
        let mut request = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", authorization());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Call {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// The outcomes this service recorded, oldest first.
    fn outcomes(&self) -> Vec<InvocationOutcome> {
        self.engine
            .state::<RouterState>()
            .lock()
            .invocations()
            .map(|invocation| invocation.outcome)
            .collect()
    }

    /// The (destination, reason) pairs this service recorded, oldest first.
    fn recorded_selections(&self) -> Vec<(String, String)> {
        self.engine
            .state::<RouterState>()
            .lock()
            .invocations()
            .map(|invocation| {
                (
                    invocation.routed_resource_arn.clone().unwrap_or_default(),
                    invocation.routing_reason.unwrap_or_default().to_string(),
                )
            })
            .collect()
    }
}

/// The routing-target-name header every invocation carries.
fn routing_target(name: &str) -> (&'static str, &str) {
    ("x-amzn-sagemaker-routing-target-name", name)
}

/// The input-location header every asynchronous submission carries, naming an object as that
/// member's own pattern (`^(https|s3)://([^/]+)/?(.*)$`) admits.
const INPUT_LOCATION: (&str, &str) = ("x-amzn-sagemaker-inputlocation", "s3://bucket/in/one.csv");

/// The payload every real-time invocation in these tests carries.
const PAYLOAD: &[u8] = b"{\"input\":\"hello\"}";

/// One row of the asynchronous-submission table: a case label, the `InferenceId` the request
/// supplies, its `S3OutputPathExtension` and `Filename` where it sends them, and the output
/// location those compose.
type SubmissionCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
);

/// THE RULE, end to end: the router chooses one destination of the routing target and answers
/// with what THAT destination answers, reporting the destination it chose and the reason —
/// and successive invocations are distributed over the destinations, because the selection
/// reads and advances a cursor held in state.
#[tokio::test]
async fn successive_invocations_are_distributed_over_the_targets_destinations() {
    let emulator = Emulator::new();
    let first = endpoint_arn(REGION, "alpha");
    let second = endpoint_arn(REGION, "beta");
    emulator.given_router(
        "fleet",
        "live",
        vec![
            Destination::new(&first).answering(
                br#"{"served":"alpha"}"#.to_vec(),
                Some("application/json".to_string()),
            ),
            Destination::new(&second)
                .in_production_variant("Primary")
                .answering(b"served by beta\n".to_vec(), Some("text/plain".to_string())),
        ],
        None,
    );

    // Four invocations of the SAME request: the rows differ only in the call's ordinal, which
    // is what the distribution rule is about, and each row states the whole answer the
    // destination it lands on gives.
    let expected: [(&str, &str, &str, &str); 4] = [
        (&first, r#"{"served":"alpha"}"#, "application/json", ""),
        (&second, "served by beta\n", "text/plain", "Primary"),
        (&first, r#"{"served":"alpha"}"#, "application/json", ""),
        (&second, "served by beta\n", "text/plain", "Primary"),
    ];
    for (ordinal, (destination, answer, content_type, variant)) in
        expected.iter().copied().enumerate()
    {
        let answered = emulator
            .invoke(
                "fleet",
                &[routing_target("live"), ("content-type", "application/json")],
                PAYLOAD,
            )
            .await;
        assert_eq!(
            answered.status,
            200,
            "case invocation {ordinal}: expected HTTP 200, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.routed_resource(),
            destination,
            "case invocation {ordinal}: expected the router to report destination \
             {destination}, got {:?}",
            answered.routed_resource()
        );
        assert_eq!(
            answered.routing_reason(),
            REASON_LOCAL_PREFERRED,
            "case invocation {ordinal}: a destination in the router's own Region that can \
             take the request is reported as {REASON_LOCAL_PREFERRED}, got {:?}",
            answered.routing_reason()
        );
        assert_eq!(
            answered.text(),
            answer,
            "case invocation {ordinal}: expected the answer destination {destination} \
             serves, got {:?}",
            answered.text()
        );
        assert_eq!(
            answered.header("content-type"),
            content_type,
            "case invocation {ordinal}: expected the content type that destination \
             declares, got {:?}",
            answered.header("content-type")
        );
        assert_eq!(
            answered.header("x-amzn-invoked-production-variant"),
            variant,
            "case invocation {ordinal}: expected the destination's own production variant, \
             got {:?}",
            answered.header("x-amzn-invoked-production-variant")
        );
        // The request's own custom attributes are NEVER reported back: the member carries
        // what the destination's model container returned, and these destinations return none.
        assert_eq!(
            answered.header("x-amzn-sagemaker-custom-attributes"),
            "",
            "case invocation {ordinal}: no custom attributes are claimed for a destination \
             that answers none, got {:?}",
            answered.header("x-amzn-sagemaker-custom-attributes")
        );
    }

    // And the distribution is STATE: this service recorded each selection, in order.
    let recorded = emulator.recorded_selections();
    let expected_record: Vec<(String, String)> = expected
        .iter()
        .map(|(destination, _, _, _)| {
            (
                (*destination).to_string(),
                REASON_LOCAL_PREFERRED.to_string(),
            )
        })
        .collect();
    assert_eq!(
        recorded, expected_record,
        "expected each served invocation to be recorded with the destination it chose, got \
         {recorded:?}"
    );
    assert!(
        emulator
            .outcomes()
            .iter()
            .all(|outcome| *outcome == InvocationOutcome::Served),
        "expected every invocation to be recorded as served, got {:?}",
        emulator.outcomes()
    );
}

/// THE RULE's custom-attributes half: the value reported is the one the destination's model
/// container returned, so a request's own value is never echoed back as the destination's.
#[tokio::test]
async fn the_custom_attributes_reported_are_the_destinations_own() {
    let emulator = Emulator::new();
    let destination = endpoint_arn(REGION, "attributing");
    emulator.given_router(
        "fleet",
        "live",
        vec![
            Destination::new(&destination)
                .answering(b"answered".to_vec(), Some("text/plain".to_string()))
                .with_custom_attributes("trace=destination"),
        ],
        None,
    );

    // The rows differ along what the REQUEST carries: nothing, and a value of its own. Either
    // way the answer carries the destination's.
    for request_attributes in [None, Some("trace=request")] {
        let mut headers = vec![routing_target("live"), ("content-type", "text/plain")];
        if let Some(attributes) = request_attributes {
            headers.push(("x-amzn-sagemaker-custom-attributes", attributes));
        }
        let answered = emulator.invoke("fleet", &headers, PAYLOAD).await;
        assert_eq!(
            answered.status,
            200,
            "case request attributes {request_attributes:?}: expected HTTP 200, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amzn-sagemaker-custom-attributes"),
            "trace=destination",
            "case request attributes {request_attributes:?}: expected the destination's own \
             value, got {:?}",
            answered.header("x-amzn-sagemaker-custom-attributes")
        );
    }
}

/// THE RULE: "The body can't be empty" (`InvokeInferenceRouterInput$Body`) — a payload with
/// nothing in it is no input to infer from, so it is refused with the modelled
/// `ValidationException` (HTTP 400) rather than routed to a destination.
#[tokio::test]
async fn an_invocation_carrying_an_empty_payload_is_refused() {
    let emulator = Emulator::new();
    let destination = endpoint_arn(REGION, "alpha");
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(&destination)
            .answering(b"answered".to_vec(), Some("text/plain".to_string()))],
        None,
    );

    // The rows differ along the payload: empty, one byte, and the ordinary one — the
    // boundary of the rule and either side of it.
    let cases: [(&str, &[u8], u16); 3] = [
        ("an empty payload", b"", 400),
        ("a payload of one byte", b"1", 200),
        ("the ordinary payload", PAYLOAD, 200),
    ];
    for (case, payload, expected) in cases {
        let answered = emulator
            .invoke(
                "fleet",
                &[routing_target("live"), ("content-type", "application/json")],
                payload,
            )
            .await;
        assert_eq!(
            answered.status,
            expected,
            "case {case:?}: expected HTTP {expected}, got {} with {}",
            answered.status,
            answered.text()
        );
        if expected == 400 {
            assert_eq!(
                answered.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled refusal, got {:?} with {}",
                answered.error_code(),
                answered.text()
            );
            assert_eq!(
                emulator.outcomes().last(),
                Some(&InvocationOutcome::EmptyPayload),
                "case {case:?}: expected the refusal recorded as an empty payload, got {:?}",
                emulator.outcomes()
            );
            assert_eq!(
                answered.routed_resource(),
                "",
                "case {case:?}: a payload refused before it is routed names no destination, \
                 got {:?}",
                answered.routed_resource()
            );
        }
    }
}

/// THE RULE: an invocation resolves the router and the routing target it names in this
/// service's own registry; a name the registry does not hold addresses nothing, which is the
/// modelled `ValidationException` (HTTP 400) — on BOTH routes, and never a success.
#[tokio::test]
async fn a_router_or_routing_target_the_registry_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(endpoint_arn(REGION, "alpha"))
            .answering(b"answered".to_vec(), Some("text/plain".to_string()))],
        Some(("s3://bucket/out", None)),
    );

    // The rows differ along WHICH name resolves to nothing, and along its shape — including a
    // name at the 63-character maximum the modelled members declare.
    let long_name = "a".repeat(63);
    let cases: [(&str, &str, &str, InvocationOutcome); 4] = [
        (
            "no such router",
            "ghost",
            "live",
            InvocationOutcome::RouterNotFound,
        ),
        (
            "a router name at the modelled maximum",
            &long_name,
            "live",
            InvocationOutcome::RouterNotFound,
        ),
        (
            "no such routing target on a registered router",
            "fleet",
            "ghost",
            InvocationOutcome::RoutingTargetNotFound,
        ),
        (
            "a routing target name at the modelled maximum",
            "fleet",
            &long_name,
            InvocationOutcome::RoutingTargetNotFound,
        ),
    ];
    for (case, router, target, outcome) in cases {
        let answered = emulator
            .invoke(
                router,
                &[routing_target(target), ("content-type", "application/json")],
                PAYLOAD,
            )
            .await;
        assert_eq!(
            answered.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?} with {}",
            answered.error_code(),
            answered.text()
        );
        let missing = match outcome {
            InvocationOutcome::RouterNotFound => router,
            _ => target,
        };
        assert!(
            answered.message().contains(missing),
            "case {case:?}: the refusal must name {missing}, got {:?}",
            answered.message()
        );

        // The same two names, on the asynchronous route: one registry, one refusal.
        let submitted = emulator
            .submit(router, &[routing_target(target), INPUT_LOCATION])
            .await;
        assert_eq!(
            submitted.status,
            400,
            "case {case:?} (asynchronous): expected HTTP 400, got {} with {}",
            submitted.status,
            submitted.text()
        );
        assert_eq!(
            submitted.error_code(),
            "ValidationException",
            "case {case:?} (asynchronous): expected the modelled refusal, got {:?} with {}",
            submitted.error_code(),
            submitted.text()
        );

        // And both refusals were recorded as what they were.
        let recorded = emulator.outcomes();
        let last_two = recorded
            .iter()
            .rev()
            .take(2)
            .filter(|got| **got == outcome)
            .count();
        assert_eq!(
            last_two,
            2,
            "case {case:?}: expected both refusals recorded as {}, got {recorded:?}",
            outcome.label()
        );
    }
}

/// THE RULE: a destination that cannot take the request is not chosen while another can, and
/// the reason reported is the one the selection decided — from the vocabulary the output
/// member documents.
#[tokio::test]
async fn the_reason_reported_is_the_one_the_selection_decided() {
    let emulator = Emulator::new();
    let local = endpoint_arn(REGION, "local");
    let local_too = endpoint_arn(REGION, "localtoo");
    let remote = endpoint_arn(OTHER_REGION, "remote");
    let remote_too = endpoint_arn(OTHER_REGION, "remotetoo");
    let answered_as = |arn: &str| {
        Destination::new(arn).answering(arn.as_bytes().to_vec(), Some("text/plain".to_string()))
    };

    // (case, the target's destinations, the reason, the destination that must serve it). The
    // rows differ along where the destinations that can take the request are, and how many of
    // them there are.
    let cases: [(&str, Vec<Destination>, &str, &str); 4] = [
        (
            "a destination in the router's own Region",
            vec![answered_as(&local), answered_as(&remote)],
            REASON_LOCAL_PREFERRED,
            &local,
        ),
        (
            "a destination that cannot take the request beside a local one",
            vec![
                answered_as(&local_too).not_ready(),
                answered_as(&local),
                answered_as(&remote),
            ],
            REASON_LOCAL_PREFERRED,
            &local,
        ),
        (
            "exactly one destination that can take it, in another Region",
            vec![answered_as(&local).not_ready(), answered_as(&remote)],
            REASON_ONLY_HEALTHY,
            &remote,
        ),
        (
            "two destinations that can take it, both in another Region",
            vec![
                answered_as(&local).not_ready(),
                answered_as(&remote),
                answered_as(&remote_too),
            ],
            REASON_CAPACITY_AVAILABLE,
            &remote,
        ),
    ];
    for (ordinal, (case, destinations, reason, serving)) in cases.into_iter().enumerate() {
        // One router per row, so each row's selection starts at the head of its own
        // destination list.
        let router = format!("fleet-{ordinal}");
        emulator.given_router(&router, "live", destinations, None);
        let answered = emulator
            .invoke(
                &router,
                &[routing_target("live"), ("content-type", "application/json")],
                PAYLOAD,
            )
            .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.routing_reason(),
            reason,
            "case {case:?}: expected the reason {reason}, got {:?}",
            answered.routing_reason()
        );
        assert_eq!(
            answered.routed_resource(),
            serving,
            "case {case:?}: expected destination {serving} to serve it, got {:?}",
            answered.routed_resource()
        );
        assert_eq!(
            answered.text(),
            serving,
            "case {case:?}: expected the answer of the destination that served it, got {:?}",
            answered.text()
        );
    }
}

/// THE RULE: the selection advances the STORED cursor past the destination it chose, so it is
/// the routing target's own state that distributes requests — which is what makes two
/// successive calls land on two destinations.
#[test]
fn the_selection_advances_the_targets_stored_cursor() {
    let local = endpoint_arn(REGION, "local");
    let remote = endpoint_arn(OTHER_REGION, "remote");
    // The rows differ along the destination list the cursor walks: one destination, two in the
    // router's Region, and one of each where only the remote one can take a request.
    let cases: [(&str, Vec<Destination>, Vec<usize>); 3] = [
        (
            "one destination",
            vec![Destination::new(&local)],
            vec![0, 0, 0],
        ),
        (
            "two destinations in the router's Region",
            vec![Destination::new(&local), Destination::new(&local)],
            vec![0, 1, 0, 1],
        ),
        (
            "only the destination in another Region can take it",
            vec![
                Destination::new(&local).not_ready(),
                Destination::new(&remote),
            ],
            vec![1, 1, 1],
        ),
    ];
    for (case, destinations, expected) in cases {
        let mut target = RoutingTarget::new("live", destinations);
        let chosen: Vec<usize> = expected
            .iter()
            .map(|_| {
                select_destination(&mut target, REGION)
                    .map(|selection| selection.index)
                    .unwrap_or_else(|| panic!("case {case:?}: a destination must be chosen"))
            })
            .collect();
        assert_eq!(
            chosen, expected,
            "case {case:?}: expected the selections to walk the list, got {chosen:?}"
        );
        assert_eq!(
            target.cursor(),
            (chosen.last().copied().unwrap_or_default() + 1) % target.destinations.len(),
            "case {case:?}: the cursor stands past the destination that was chosen, got {}",
            target.cursor()
        );
    }
    // A target that lists no destination has nothing to choose.
    let mut empty = RoutingTarget::new("live", Vec::new());
    assert!(
        select_destination(&mut empty, REGION).is_none(),
        "a target listing no destination chooses none"
    );
}

/// THE RULE: when NO destination of the routing target can take the request, the router tries
/// one anyway — which is the `failover` reason — and the invocation is refused with the
/// modelled `ModelNotReadyException` (HTTP 429) naming the destination that is not ready.
#[tokio::test]
async fn an_invocation_no_destination_can_take_is_refused_as_not_ready() {
    let emulator = Emulator::new();
    let local = endpoint_arn(REGION, "local");
    let remote = endpoint_arn(OTHER_REGION, "remote");
    // The rows differ along where the destinations that cannot take the request are, and how
    // many of them there are.
    let cases: [(&str, Vec<Destination>, &str); 3] = [
        (
            "the one destination cannot take it",
            vec![Destination::new(&local).not_ready()],
            &local,
        ),
        (
            "no destination in either Region can take it",
            vec![
                Destination::new(&local).not_ready(),
                Destination::new(&remote).not_ready(),
            ],
            &local,
        ),
        (
            "the only destination that cannot take it is in another Region",
            vec![Destination::new(&remote).not_ready()],
            &remote,
        ),
    ];
    for (ordinal, (case, destinations, tried)) in cases.into_iter().enumerate() {
        let router = format!("fleet-{ordinal}");
        emulator.given_router(&router, "live", destinations, None);
        let answered = emulator
            .invoke(
                &router,
                &[routing_target("live"), ("content-type", "application/json")],
                PAYLOAD,
            )
            .await;
        assert_eq!(
            answered.status,
            429,
            "case {case:?}: expected HTTP 429, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.error_code(),
            "ModelNotReadyException",
            "case {case:?}: expected the modelled not-ready refusal, got {:?} with {}",
            answered.error_code(),
            answered.text()
        );
        assert!(
            answered.message().contains(tried),
            "case {case:?}: the refusal must name the destination it tried, got {:?}",
            answered.message()
        );
        let recorded = emulator.recorded_selections();
        let (destination, reason) = recorded.last().cloned().unwrap_or_default();
        assert_eq!(
            (destination.as_str(), reason.as_str()),
            (tried, REASON_FAILOVER),
            "case {case:?}: a destination tried although none can take the request is \
             recorded as {REASON_FAILOVER}, got {recorded:?}"
        );
        assert_eq!(
            emulator.outcomes().last(),
            Some(&InvocationOutcome::DestinationNotReady),
            "case {case:?}: expected the refusal recorded as a destination that is not ready, \
             got {:?}",
            emulator.outcomes()
        );
    }
}

/// THE RULE: a routing target with nothing to route to does not answer a request — a target
/// listing no destination at all, and a destination that takes the request and answers
/// nothing, are both the modelled `ServiceUnavailableException` (HTTP 503), never a body this
/// service invented for them.
#[tokio::test]
async fn a_request_nothing_answers_is_refused_as_unavailable() {
    let emulator = Emulator::new();
    let silent = endpoint_arn(REGION, "silent");
    emulator.given_router("empty", "live", Vec::new(), None);
    emulator.given_router("silent", "live", vec![Destination::new(&silent)], None);

    // (case, router, the outcome the service records for it)
    let cases: [(&str, &str, InvocationOutcome); 2] = [
        (
            "a routing target listing no destination",
            "empty",
            InvocationOutcome::NoDestination,
        ),
        (
            "a destination that answers nothing",
            "silent",
            InvocationOutcome::DestinationDidNotRespond,
        ),
    ];
    for (case, router, outcome) in cases {
        let answered = emulator
            .invoke(
                router,
                &[routing_target("live"), ("content-type", "application/json")],
                PAYLOAD,
            )
            .await;
        assert_eq!(
            answered.status,
            503,
            "case {case:?}: expected HTTP 503, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.error_code(),
            "ServiceUnavailableException",
            "case {case:?}: expected the modelled unavailable refusal, got {:?} with {}",
            answered.error_code(),
            answered.text()
        );
        assert_eq!(
            answered.routed_resource(),
            "",
            "case {case:?}: a refusal reports no destination as having served the request, \
             got {:?}",
            answered.routed_resource()
        );
        assert_eq!(
            emulator.outcomes().last(),
            Some(&outcome),
            "case {case:?}: expected the refusal recorded as {}, got {:?}",
            outcome.label(),
            emulator.outcomes()
        );
    }
}

/// THE RULE: an asynchronous submission is RECORDED under the `InferenceId` the response
/// reports — the caller's own, or one this service generated — and the locations it reports
/// are the routing target's configured paths, the request's path extension and the object's
/// name.
#[tokio::test]
async fn an_async_submission_is_recorded_under_the_id_it_reports() {
    let emulator = Emulator::new();
    let destination = endpoint_arn(REGION, "alpha");
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(&destination)],
        Some(("s3://bucket/out/", Some("s3://bucket/failed"))),
    );

    // The rows differ along what the request supplies: the id, the path extension and the
    // object's own name — which is what the reported locations are composed from.
    let cases: [SubmissionCase; 4] = [
        (
            "the caller's own id",
            "req-1",
            None,
            None,
            "s3://bucket/out/req-1.out",
        ),
        (
            "a path extension",
            "req-2",
            Some("tenant/7"),
            None,
            "s3://bucket/out/tenant/7/req-2.out",
        ),
        (
            "an object name",
            "req-3",
            None,
            Some("answer.json"),
            "s3://bucket/out/answer.json",
        ),
        (
            "a path extension and an object name",
            "req-4",
            Some("tenant/7"),
            Some("answer.json"),
            "s3://bucket/out/tenant/7/answer.json",
        ),
    ];
    for (case, inference_id, extension, filename, expected) in cases {
        let mut headers = vec![
            routing_target("live"),
            INPUT_LOCATION,
            ("x-amzn-sagemaker-inference-id", inference_id),
        ];
        if let Some(extension) = extension {
            headers.push(("x-amzn-sagemaker-s3outputpathextension", extension));
        }
        if let Some(filename) = filename {
            headers.push(("x-amzn-sagemaker-filename", filename));
        }
        let answered = emulator.submit("fleet", &headers).await;
        assert_eq!(
            answered.status,
            202,
            "case {case:?}: expected HTTP 202, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.json()["InferenceId"].as_str(),
            Some(inference_id),
            "case {case:?}: expected the id the request supplied, got {}",
            answered.text()
        );
        assert_eq!(
            answered.header("x-amzn-sagemaker-outputlocation"),
            expected,
            "case {case:?}: expected the output location {expected}, got {:?}",
            answered.header("x-amzn-sagemaker-outputlocation")
        );
        assert_eq!(
            answered.header("x-amzn-sagemaker-failurelocation"),
            format!("s3://bucket/failed/{inference_id}-error.out"),
            "case {case:?}: expected the failure object under the target's failure path, got \
             {:?}",
            answered.header("x-amzn-sagemaker-failurelocation")
        );

        // The record exists, under that id, with the destination the selection chose.
        let state = emulator.engine.state::<RouterState>();
        let store = state.lock();
        let stored = store
            .accepted_submission("fleet", "live", inference_id)
            .unwrap_or_else(|| panic!("case {case:?}: the submission must be stored"));
        assert_eq!(
            stored.output_location, expected,
            "case {case:?}: the stored record holds the location that was reported, got {:?}",
            stored.output_location
        );
        assert_eq!(
            stored.routed_resource_arn, destination,
            "case {case:?}: the record names the destination the router sent it to, got {:?}",
            stored.routed_resource_arn
        );
        assert_eq!(
            stored.routing_reason, REASON_LOCAL_PREFERRED,
            "case {case:?}: the record keeps the reason the router chose it, got {:?}",
            stored.routing_reason
        );
        assert_eq!(
            (stored.request_ttl_seconds, stored.invocation_timeout_seconds),
            (
                DEFAULT_REQUEST_TTL_SECONDS,
                DEFAULT_INVOCATION_TIMEOUT_SECONDS
            ),
            "case {case:?}: a submission that asks for neither takes the documented \
             defaults, got {:?}",
            (stored.request_ttl_seconds, stored.invocation_timeout_seconds)
        );
        assert_eq!(
            stored.input_location, INPUT_LOCATION.1,
            "case {case:?}: the record keeps the input the submission named, got {:?}",
            stored.input_location
        );
    }

    // A submission that supplies NO id is accepted under a generated one, which is stored
    // too — and two such submissions never share it.
    let mut generated: Vec<String> = Vec::new();
    for ordinal in 0..2 {
        let answered = emulator
            .submit("fleet", &[routing_target("live"), INPUT_LOCATION])
            .await;
        assert_eq!(
            answered.status,
            202,
            "case generated id {ordinal}: expected HTTP 202, got {} with {}",
            answered.status,
            answered.text()
        );
        let id = answered.json()["InferenceId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case generated id {ordinal}: the response must report the id it was accepted \
             under, got {}",
            answered.text()
        );
        let state = emulator.engine.state::<RouterState>();
        let store = state.lock();
        assert!(
            store.accepted_submission("fleet", "live", &id).is_some(),
            "case generated id {ordinal}: the generated id {id} must name a stored record"
        );
        generated.push(id);
    }
    assert_ne!(
        generated.first(),
        generated.last(),
        "two submissions must not share a generated id, got {generated:?}"
    );

    // A submission that asks for its own time to live and invocation timeout is recorded with
    // them rather than with the defaults — here at the minimum of each member's modelled
    // range (`RequestTTLSeconds` 60..21600, `InvocationTimeoutSeconds` 1..3600).
    let answered = emulator
        .submit(
            "fleet",
            &[
                routing_target("live"),
                INPUT_LOCATION,
                ("x-amzn-sagemaker-inference-id", "req-bounded"),
                ("x-amzn-sagemaker-requestttlseconds", "60"),
                ("x-amzn-sagemaker-invocationtimeoutseconds", "1"),
            ],
        )
        .await;
    assert_eq!(
        answered.status,
        202,
        "a submission at the modelled minimum bounds is accepted, got {} with {}",
        answered.status,
        answered.text()
    );
    let state = emulator.engine.state::<RouterState>();
    let store = state.lock();
    let stored = store
        .accepted_submission("fleet", "live", "req-bounded")
        .expect("the submission must be stored");
    assert_eq!(
        (stored.request_ttl_seconds, stored.invocation_timeout_seconds),
        (60, 1),
        "the record keeps the bounds the submission asked for, got {:?}",
        (stored.request_ttl_seconds, stored.invocation_timeout_seconds)
    );
}

/// THE RULE: a resubmission under an `InferenceId` this service already accepted is the SAME
/// submission — it is answered the locations the record holds and is not accepted twice.
#[tokio::test]
async fn a_resubmission_under_an_accepted_id_is_answered_from_the_record() {
    let emulator = Emulator::new();
    let destination = endpoint_arn(REGION, "alpha");
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(&destination)],
        Some(("s3://bucket/out", Some("s3://bucket/failed"))),
    );
    let headers = vec![
        routing_target("live"),
        INPUT_LOCATION,
        ("x-amzn-sagemaker-inference-id", "req-1"),
    ];
    let first = emulator.submit("fleet", &headers).await;
    // The resubmission names the same id and a DIFFERENT object name: the id names the
    // submission that was already accepted, so what it is answered is that record's.
    let mut resubmitted = headers.clone();
    resubmitted.push(("x-amzn-sagemaker-filename", "other.json"));
    let second = emulator.submit("fleet", &resubmitted).await;
    for (case, answered) in [
        ("the first submission", &first),
        ("the resubmission", &second),
    ] {
        assert_eq!(
            answered.status,
            202,
            "case {case:?}: expected HTTP 202, got {} with {}",
            answered.status,
            answered.text()
        );
    }
    assert_eq!(
        second.json()["InferenceId"],
        first.json()["InferenceId"],
        "the resubmission is accepted under the same id, got {} and {}",
        first.text(),
        second.text()
    );
    assert_eq!(
        second.header("x-amzn-sagemaker-outputlocation"),
        first.header("x-amzn-sagemaker-outputlocation"),
        "the resubmission reports the stored output location, got {:?} and {:?}",
        first.header("x-amzn-sagemaker-outputlocation"),
        second.header("x-amzn-sagemaker-outputlocation")
    );
    assert_eq!(
        second.header("x-amzn-sagemaker-failurelocation"),
        first.header("x-amzn-sagemaker-failurelocation"),
        "the resubmission reports the stored failure location, got {:?} and {:?}",
        first.header("x-amzn-sagemaker-failurelocation"),
        second.header("x-amzn-sagemaker-failurelocation")
    );
    let state = emulator.engine.state::<RouterState>();
    let accepted = state.lock().async_submissions().count();
    assert_eq!(
        accepted, 1,
        "one id names one accepted submission, got {accepted} records"
    );
    assert_eq!(
        emulator.outcomes().last(),
        Some(&InvocationOutcome::AlreadyAccepted),
        "expected the resubmission recorded as one already accepted, got {:?}",
        emulator.outcomes()
    );
}

/// THE RULE: a routing target that specifies no Amazon S3 output path has nowhere to report
/// an asynchronous result, so a submission to it is refused rather than accepted.
#[tokio::test]
async fn an_async_submission_to_a_target_with_no_output_path_is_refused() {
    let emulator = Emulator::new();
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(endpoint_arn(REGION, "alpha"))],
        None,
    );
    let answered = emulator
        .submit("fleet", &[routing_target("live"), INPUT_LOCATION])
        .await;
    assert_eq!(
        answered.status,
        400,
        "expected HTTP 400, got {} with {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.error_code(),
        "ValidationException",
        "expected the modelled refusal, got {:?} with {}",
        answered.error_code(),
        answered.text()
    );
    assert_eq!(
        emulator.outcomes().last(),
        Some(&InvocationOutcome::NoOutputPath),
        "expected the refusal recorded as a target with no output path, got {:?}",
        emulator.outcomes()
    );
    let state = emulator.engine.state::<RouterState>();
    let accepted = state.lock().async_submissions().count();
    assert_eq!(
        accepted, 0,
        "a refused submission is not recorded, got {accepted} records"
    );
}

/// THE RULE's key: an `InferenceId` is the CALLER's identifier for its OWN request, so one id
/// on two different routing targets is two submissions — each recorded, and each reported at
/// the paths its own target specifies — while one id on the same target is one.
#[tokio::test]
async fn one_inference_id_on_two_routing_targets_is_two_submissions() {
    let emulator = Emulator::new();
    let destination = endpoint_arn(REGION, "alpha");
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(&destination)],
        Some(("s3://bucket/fleet-out", Some("s3://bucket/fleet-failed"))),
    );
    emulator.given_router(
        "other",
        "live",
        vec![Destination::new(&destination)],
        Some(("s3://bucket/other-out", Some("s3://bucket/other-failed"))),
    );
    // The rows differ along the router the same id is submitted to; each row's locations are
    // its own target's.
    let cases: [(&str, &str, &str, &str); 2] = [
        (
            "the first router",
            "fleet",
            "s3://bucket/fleet-out/req-1.out",
            "s3://bucket/fleet-failed/req-1-error.out",
        ),
        (
            "the second router",
            "other",
            "s3://bucket/other-out/req-1.out",
            "s3://bucket/other-failed/req-1-error.out",
        ),
    ];
    for (case, router, output, failure) in cases {
        let answered = emulator
            .submit(
                router,
                &[
                    routing_target("live"),
                    INPUT_LOCATION,
                    ("x-amzn-sagemaker-inference-id", "req-1"),
                ],
            )
            .await;
        assert_eq!(
            answered.status,
            202,
            "case {case:?}: expected HTTP 202, got {} with {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amzn-sagemaker-outputlocation"),
            output,
            "case {case:?}: expected the location under its own target's output path, got {:?}",
            answered.header("x-amzn-sagemaker-outputlocation")
        );
        assert_eq!(
            answered.header("x-amzn-sagemaker-failurelocation"),
            failure,
            "case {case:?}: expected the failure object under its own target's failure path, \
             got {:?}",
            answered.header("x-amzn-sagemaker-failurelocation")
        );
        assert_eq!(
            emulator.outcomes().last(),
            Some(&InvocationOutcome::Queued),
            "case {case:?}: expected the submission recorded as accepted, got {:?}",
            emulator.outcomes()
        );
        let state = emulator.engine.state::<RouterState>();
        let store = state.lock();
        let stored = store
            .accepted_submission(router, "live", "req-1")
            .unwrap_or_else(|| panic!("case {case:?}: the submission must be stored"));
        assert_eq!(
            stored.output_location, output,
            "case {case:?}: the record holds its own target's location, got {:?}",
            stored.output_location
        );
    }
    let state = emulator.engine.state::<RouterState>();
    let accepted = state.lock().async_submissions().count();
    assert_eq!(
        accepted, 2,
        "one id on two routing targets names two submissions, got {accepted} records"
    );
}

/// THE RULE: the outcome of an accepted submission is owed by the CLOCK — materialised at the
/// instant it falls due whether or not anybody called again — and no inference was performed,
/// so what falls due is the failure state with the reason naming that.
#[tokio::test]
async fn the_outcome_the_clock_owes_an_accepted_submission_is_materialised() {
    let emulator = Emulator::new();
    emulator.given_router(
        "fleet",
        "live",
        vec![Destination::new(endpoint_arn(REGION, "alpha"))],
        Some(("s3://bucket/out", None)),
    );
    let answered = emulator
        .submit(
            "fleet",
            &[
                routing_target("live"),
                INPUT_LOCATION,
                ("x-amzn-sagemaker-inference-id", "req-1"),
            ],
        )
        .await;
    assert_eq!(
        answered.status,
        202,
        "expected HTTP 202, got {} with {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.header("x-amzn-sagemaker-failurelocation"),
        "",
        "a target specifying no failure path reports no failure location, got {:?}",
        answered.header("x-amzn-sagemaker-failurelocation")
    );
    let state = emulator.engine.state::<RouterState>();
    // Straight after the acceptance the submission is in progress: the span is never zero.
    assert_eq!(
        state
            .lock()
            .accepted_submission("fleet", "live", "req-1")
            .map(|submission| submission.status),
        Some(AsyncSubmissionStatus::InProgress),
        "an accepted submission whose outcome is not due yet is in progress"
    );
    // The clock, driven rather than waited for: the instant the outcome was due.
    let due = SystemTime::now()
        .checked_add(ASYNC_OUTCOME_DUE_AFTER)
        .expect("the due instant is representable");
    let settled = state.lock().settle_async_submissions(due);
    assert_eq!(
        settled, 1,
        "the one submission whose outcome was due is the one that settles, got {settled}"
    );
    let store = state.lock();
    let stored = store
        .accepted_submission("fleet", "live", "req-1")
        .expect("the submission is stored");
    assert_eq!(
        stored.status,
        AsyncSubmissionStatus::Failed,
        "an outcome nothing performed falls due as the failure state, got {:?}",
        stored.status
    );
    assert_eq!(
        stored.failure_reason.as_deref(),
        Some(NO_ASYNC_INFERENCE),
        "the record says why no inference was performed, got {:?}",
        stored.failure_reason
    );
}
