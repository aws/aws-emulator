// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `bedrock-agentcore` SigV4 signing name.
//!
//! The AgentCore CONTROL plane (`AmazonBedrockAgentCoreControl`, endpointPrefix
//! `bedrock-agentcore-control`) and the AgentCore DATA plane (`AmazonBedrockAgentCore`,
//! endpointPrefix `bedrock-agentcore`) SHARE one SigV4 signing name,
//! `bedrock-agentcore`, and one `arnNamespace`, while serving two different endpoints.
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! both services see the key `bedrock-agentcore`: a service that claims on the signing
//! name alone claims the other's traffic too, and the router answers `500
//! AmbiguousService` — an error code no AWS model declares, and a 5xx an SDK retries to
//! exhaustion — for BOTH.
//!
//! The rule these suites pin: a request signed for a signing name several emulated
//! services share is claimed by exactly ONE of them — the one whose own model binds a
//! ROUTE (an HTTP method AND a URI) for the request. Unlike every other shared-signing-
//! name pair, no path segment can decide it here: the two models publish the same first
//! path segment for nine resource families (`/memories/…`, `/runtimes/…`, `/browsers/…`,
//! `/code-interpreters/…`, `/identities/…`, `/capacity-providers/…`, `/harnesses/…`,
//! `/payments/…`, `/browser-profiles/…`) and collide at deeper indexes too, which is
//! what AWS answers by dispatching on the endpoint host plus the `restJson1` route.
//!
//! Rows therefore span the dimensions that rule generalises over: which endpoint owns
//! the route, the method (a path both models publish, bound by each for a different
//! method), the depth at which the two models' URIs diverge, a trailing-slash URI, a
//! route neither binds, and a `Host` that agrees with, disagrees with, or says nothing
//! about the target — because on a signed request the credential scope, not the host,
//! resolves the key.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

// Percent-encodes one `@httpLabel` value the way an AWS SDK does, so an ARN's `:` and `/`
// reach the server as part of the label rather than as URI syntax. Shared with the sibling
// AgentCore suites of this crate rather than written again here.
use crate::bedrock_agentcore_data_plane_parent_resources::label;

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTCORE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// How a row names the service that must claim it. `Both` and `Neither` are the two
/// routing failures: `Both` is the `500 AmbiguousService` this issue is about, `Neither`
/// the router's `404 UnknownService`.
const OWNER_CONTROL: &str = "bedrock-agentcore-control";
const OWNER_DATA: &str = "bedrock-agentcore";
const OWNER_BOTH: &str = "both (the router answers 500 AmbiguousService)";
const OWNER_NEITHER: &str = "neither (the router answers 404 UnknownService)";

type Control =
    service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService<
        engine::InMemoryEngine,
    >;
type Data = service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService<
    engine::InMemoryEngine,
>;

/// The two services that share the `bedrock-agentcore` signing name, over one shared
/// engine, exactly as `router/src/lib.rs` mounts them.
fn both_services() -> (Control, Data) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (Control::new(Arc::clone(&engine)), Data::new(engine))
}

/// A router with both of them mounted, so a request that two services claim really is
/// refused as `AmbiguousService`.
fn router_with_both_services() -> MultiServiceRouter {
    let (control, data) = both_services();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(control);
    router.register_service(data);
    router
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, host: &str, uri: &str) -> http::request::Parts {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("the test request is well formed");
    request.into_parts().0
}

/// The service that claims a request, as one of the four `OWNER_*` outcomes.
fn claimant(control: &Control, data: &Data, parts: &http::request::Parts) -> &'static str {
    match (control.can_handle(parts), data.can_handle(parts)) {
        (true, false) => OWNER_CONTROL,
        (false, true) => OWNER_DATA,
        (true, true) => OWNER_BOTH,
        (false, false) => OWNER_NEITHER,
    }
}

/// One answer from the router: the HTTP status and the restJson1 `__type` error code
/// (the empty string on a response that carries none).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, String, Value) {
    let payload = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost:9297")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

#[test]
fn exactly_one_service_sharing_the_agentcore_signing_name_claims_each_request() {
    let (control, data) = both_services();

    // A 32-row table: (case, method, host, uri, the service that must claim it)
    //
    // Every route comes from the two models' `smithy.api#http` bindings, as the crates'
    // `OPERATIONS.md` lists them (`smithy_query(query="digest", …)` on each model).
    let rows: [(&str, &str, &str, &str, &str); 32] = [
        // The control plane's own routes, on hosts that name it, name its sibling, and
        // name nothing at all. The gateway family is the one the issue was filed about.
        (
            "control CreateGateway",
            "POST",
            "bedrock-agentcore-control.us-east-1.amazonaws.com",
            "/gateways/",
            OWNER_CONTROL,
        ),
        (
            "control CreateGateway on the sibling's host",
            "POST",
            "bedrock-agentcore.us-east-1.amazonaws.com",
            "/gateways/",
            OWNER_CONTROL,
        ),
        (
            "control CreateGateway on a bare emulator endpoint",
            "POST",
            "localhost:9297",
            "/gateways/",
            OWNER_CONTROL,
        ),
        ("control ListGateways", "GET", "localhost:9297", "/gateways/", OWNER_CONTROL),
        (
            "control GetGateway",
            "GET",
            "localhost:9297",
            "/gateways/my-gateway-abcdefghij/",
            OWNER_CONTROL,
        ),
        (
            "control CreateGatewayTarget",
            "POST",
            "localhost:9297",
            "/gateways/my-gateway-abcdefghij/targets/",
            OWNER_CONTROL,
        ),
        (
            "control SynchronizeGatewayTargets",
            "PUT",
            "localhost:9297",
            "/gateways/my-gateway-abcdefghij/synchronizeTargets",
            OWNER_CONTROL,
        ),
        // The resource families whose FIRST path segment both models publish: the route
        // decides, at whatever depth the two models diverge.
        ("control CreateAgentRuntime", "PUT", "localhost:9297", "/runtimes/", OWNER_CONTROL),
        (
            "control GetAgentRuntime",
            "GET",
            "localhost:9297",
            "/runtimes/probe-0123456789/",
            OWNER_CONTROL,
        ),
        (
            "control ListAgentRuntimeVersions",
            "POST",
            "localhost:9297",
            "/runtimes/probe-0123456789/versions/",
            OWNER_CONTROL,
        ),
        (
            "data InvokeAgentRuntime, the same first two segments",
            "POST",
            "localhost:9297",
            "/runtimes/probe-0123456789/invocations",
            OWNER_DATA,
        ),
        (
            // `InvokeAgentRuntimeCommand` is modelled but UNWIRED: the scaffold generator
            // declines an operation with a `@streaming` event-stream output
            // (`…-bedrockagentcore-amazonbedrockagentcore/src/ops/SKIPPED_STUBS.md`), so
            // it is in neither service's route table and belongs to neither. That is the
            // router's `404 UnknownService` — not this issue's retryable `500
            // AmbiguousService`, which is what it answered before, and not a claim by the
            // service that does not serve it.
            "data InvokeAgentRuntimeCommand, modelled but unwired",
            "POST",
            "localhost:9297",
            "/runtimes/probe-0123456789/commands",
            OWNER_NEITHER,
        ),
        (
            "data GetAgentCard, five segments deep",
            "GET",
            "localhost:9297",
            "/runtimes/probe-0123456789/invocations/.well-known/agent-card.json",
            OWNER_DATA,
        ),
        ("control CreateMemory", "POST", "localhost:9297", "/memories/create", OWNER_CONTROL),
        (
            "control GetMemory",
            "GET",
            "localhost:9297",
            "/memories/demo-memory-0123456789/details",
            OWNER_CONTROL,
        ),
        (
            "data CreateEvent",
            "POST",
            "localhost:9297",
            "/memories/demo-memory-0123456789/events",
            OWNER_DATA,
        ),
        (
            "data GetMemoryRecord",
            "GET",
            "localhost:9297",
            "/memories/demo-memory-0123456789/memoryRecord/rec-1",
            OWNER_DATA,
        ),
        (
            "data DeleteMemoryRecord, the same depth as a control route",
            "DELETE",
            "localhost:9297",
            "/memories/demo-memory-0123456789/memoryRecords/rec-1",
            OWNER_DATA,
        ),
        ("control CreateBrowser", "POST", "localhost:9297", "/browsers", OWNER_CONTROL),
        ("control GetBrowser", "GET", "localhost:9297", "/browsers/browser-1", OWNER_CONTROL),
        (
            "data StartBrowserSession",
            "PUT",
            "localhost:9297",
            "/browsers/browser-1/sessions/start",
            OWNER_DATA,
        ),
        (
            "control GetCodeInterpreter",
            "GET",
            "localhost:9297",
            "/code-interpreters/ci-1",
            OWNER_CONTROL,
        ),
        (
            // Hand-authored since the data plane's code interpreter tool surface was
            // implemented: its `@streaming` union output keeps it out of the generated
            // routing table, so `src/service.rs` routes it through
            // `Service::override_operation` — and the data plane claims it, which is
            // what puts this route in the data plane's own path space rather than in
            // neither service's.
            "data InvokeCodeInterpreter",
            "POST",
            "localhost:9297",
            "/code-interpreters/ci-1/tools/invoke",
            OWNER_DATA,
        ),
        // One path space where both models bind the SAME depth with DIFFERENT literals,
        // and one where they bind the same shape under different methods.
        (
            "control CreateWorkloadIdentity",
            "POST",
            "localhost:9297",
            "/identities/CreateWorkloadIdentity",
            OWNER_CONTROL,
        ),
        (
            "data GetWorkloadAccessToken",
            "POST",
            "localhost:9297",
            "/identities/GetWorkloadAccessToken",
            OWNER_DATA,
        ),
        (
            "control CreatePaymentManager",
            "POST",
            "localhost:9297",
            "/payments/managers",
            OWNER_CONTROL,
        ),
        (
            "data ProcessPayment",
            "POST",
            "localhost:9297",
            "/payments/processPayment",
            OWNER_DATA,
        ),
        (
            // The harness invocation is HAND-AUTHORED since it was implemented, for the
            // same reason `InvokeCodeInterpreter` above is — but its route binds a
            // `@required` query parameter (`harnessArn`) and a `@required` header
            // (`runtimeSessionId`), and the framework's REST route lookup rejects a
            // candidate route whose required bindings the request does not carry. So this
            // bare probe addresses no route of either model and belongs to neither; the
            // test below this one claims a well-formed harness invocation and shows the
            // data plane taking it.
            "data InvokeHarness without the query and header its route binds",
            "POST",
            "localhost:9297",
            "/harnesses/invoke",
            OWNER_NEITHER,
        ),
        (
            "data RetrieveMemoryRecords",
            "POST",
            "localhost:9297",
            "/memories/demo-memory-0123456789/retrieve",
            OWNER_DATA,
        ),
        (
            "data ListActors",
            "POST",
            "localhost:9297",
            "/memories/demo-memory-0123456789/actors",
            OWNER_DATA,
        ),
        // Neither of the two routing failures may appear for a modelled route; a route
        // NEITHER model binds belongs to neither service, which is the router's
        // `UnknownService` and not an ambiguity.
        (
            "a method neither model binds on a path both publish",
            "PATCH",
            "localhost:9297",
            "/memories/demo-memory-0123456789/events",
            OWNER_NEITHER,
        ),
        (
            "a path neither model binds",
            "POST",
            "localhost:9297",
            "/no-such-resource",
            OWNER_NEITHER,
        ),
    ];

    for (case, method, host, uri, expected) in rows {
        let actual = claimant(&control, &data, &parts(method, host, uri));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} to host {host}): expected it to be claimed by \
             {expected}, got {actual}"
        );
    }
}

/// A WELL-FORMED harness invocation — one carrying the `@required` query parameter and the
/// `@required` session header its route binds — is claimed by the DATA plane, the service
/// whose model binds `POST /harnesses/invoke`, and by it alone.
///
/// Its own row in the table above carries neither binding, which is why that probe belongs
/// to neither service: the framework's route lookup rejects a candidate route whose required
/// bindings are absent, so the two halves together are what says the data plane claims this
/// route exactly when a request addresses it.
#[tokio::test]
async fn a_well_formed_harness_invocation_is_claimed_by_the_data_plane() {
    let (control, data) = both_services();
    let harness_arn =
        "arn:aws:bedrock-agentcore:us-east-1:123456789012:harness/probe_harness-0123456789";
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "/harnesses/invoke?harnessArn={}",
            label(harness_arn)
        ))
        .header("host", "localhost:9297")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        // `SessionId`'s own length floor is 33 characters.
        .header(
            "X-Amzn-Bedrock-AgentCore-Runtime-Session-Id",
            "probe-harness-session-0123456789abc",
        )
        .body(Bytes::new())
        .expect("the test request is well formed");
    let parts = request.into_parts().0;

    let actual = claimant(&control, &data, &parts);
    assert_eq!(
        actual, OWNER_DATA,
        "case \"a well-formed InvokeHarness\": expected it to be claimed by {OWNER_DATA}, got \
         {actual}"
    );
}

#[tokio::test]
async fn every_agentcore_signed_request_is_dispatched_instead_of_refused_as_ambiguous() {
    let router = router_with_both_services();

    // The data plane writes only INSIDE a Memory container the CONTROL plane created, so
    // the container the two data-plane rows below address is created first — through the
    // control plane's own route, over the engine both services share. Its `memoryId` is
    // the one that plane MINTED: an id a request invents belongs to no container.
    let (status, code, memory) = call(
        &router,
        "POST",
        "/memories/create",
        json!({"name": "shared_signing_name", "eventExpiryDuration": 30}),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (202, ""),
        "the fixture memory container must be created, got HTTP {status} {code:?} with \
         {memory:?}"
    );
    let memory_id = memory["memory"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateMemory reported no memory id: {memory:?}"))
        .to_string();

    // An agent runtime of the control plane, so the data-plane invocation rows below
    // address an agent that EXISTS: an invocation resolves the agent and its endpoint
    // against the plane that owns them before it reaches the invocation itself, so the
    // scaffold's own answer is only observable for a runtime that resolves.
    let (status, code, runtime) = call(
        &router,
        "PUT",
        "/runtimes/",
        json!({
            "agentRuntimeName": "shared_signing_name_invoked",
            "agentRuntimeArtifact": {
                "containerConfiguration": {
                    "containerUri": "123456789012.dkr.ecr.us-east-1.amazonaws.com/agent:3"
                }
            },
            "networkConfiguration": {"networkMode": "PUBLIC"},
            "roleArn": "arn:aws:iam::123456789012:role/AgentRuntimeRole"
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (202, ""),
        "the fixture agent runtime must be created, got HTTP {status} {code:?} with {runtime:?}"
    );
    let runtime_arn = runtime["agentRuntimeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAgentRuntime reported no ARN: {runtime:?}"))
        .to_string();

    // (case, method, uri, body, expected status, expected error code)
    //
    // Each row names the answer of the ONE service that owns the route, which is what
    // excludes both routing failures: `AmbiguousService` (500, two claimants) and
    // `UnknownService` (404, none). An operation whose own resource lookup misses answers its
    // modelled `404 ResourceNotFoundException`, and one whose work this emulator cannot do at
    // all — running a container image — answers its modelled `424 RuntimeClientError`; both
    // answers come from the service that owns the route, where the router's refusals do not.
    let rows: [(&str, &str, String, Value, u16, &str); 9] = [
        (
            "CreateGateway, the operation this issue was filed on",
            "POST",
            "/gateways/".to_string(),
            json!({
                "name": "lambda-mcp-gateway",
                "roleArn": "arn:aws:iam::123456789012:role/GatewayRole",
                "protocolType": "MCP",
                "authorizerType": "AWS_IAM"
            }),
            202,
            "",
        ),
        (
            "ListGateways, implemented",
            "GET",
            "/gateways/".to_string(),
            Value::Null,
            200,
            "",
        ),
        (
            "GetGatewayTarget on a gateway that does not exist",
            "GET",
            "/gateways/my-gateway-abcdefghij/targets/abcdefghij/".to_string(),
            Value::Null,
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateAgentRuntime on the control plane, implemented",
            "PUT",
            "/runtimes/".to_string(),
            json!({
                "agentRuntimeName": "shared_signing_name",
                "agentRuntimeArtifact": {
                    "containerConfiguration": {
                        "containerUri": "123456789012.dkr.ecr.us-east-1.amazonaws.com/agent:1"
                    }
                },
                "networkConfiguration": {"networkMode": "PUBLIC"},
                "roleArn": "arn:aws:iam::123456789012:role/AgentRuntimeRole"
            }),
            202,
            "",
        ),
        (
            "ListAgentRuntimes on the control plane, implemented",
            "POST",
            "/runtimes/".to_string(),
            Value::Null,
            200,
            "",
        ),
        (
            "CreateEvent on the data plane, implemented",
            "POST",
            format!("/memories/{memory_id}/events"),
            json!({
                "actorId": "actor-1",
                "sessionId": "session-1",
                "eventTimestamp": 1_767_225_600.0,
                "payload": [{"conversational": {"role": "USER", "content": {"text": "hello"}}}]
            }),
            201,
            "",
        ),
        (
            "ListSessions on the data plane, implemented",
            "POST",
            format!("/memories/{memory_id}/actor/actor-1/sessions"),
            json!({}),
            200,
            "",
        ),
        (
            "InvokeAgentRuntime on the data plane, whose runtime is deployed from a container \
             image this emulator starts none of",
            "POST",
            format!("/runtimes/{}/invocations", label(&runtime_arn)),
            json!({}),
            424,
            "RuntimeClientError",
        ),
        (
            "InvokeAgentRuntime on an agent nothing created",
            "POST",
            "/runtimes/probe-0123456789/invocations".to_string(),
            json!({}),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, &uri, body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the one \
             service that owns {uri}, got {status} {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn an_agent_runtime_created_through_the_shared_signing_name_reads_back() {
    let router = router_with_both_services();

    let (status, code, created) = call(
        &router,
        "PUT",
        "/runtimes/",
        json!({
            "agentRuntimeName": "round_trip",
            "agentRuntimeArtifact": {
                "containerConfiguration": {
                    "containerUri": "123456789012.dkr.ecr.us-east-1.amazonaws.com/agent:2"
                }
            },
            "networkConfiguration": {"networkMode": "PUBLIC"},
            "roleArn": "arn:aws:iam::123456789012:role/AgentRuntimeRole"
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (202, ""),
        "case \"CreateAgentRuntime\": expected HTTP 202, got {status} {code:?} ({created})"
    );

    let arn = created["agentRuntimeArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.starts_with("arn:aws:bedrock-agentcore:us-east-1:"),
        "case \"CreateAgentRuntime\": expected an ARN in the shared bedrock-agentcore \
         namespace, got {arn:?}"
    );

    let id = created["agentRuntimeId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, fetched) =
        call(&router, "GET", &format!("/runtimes/{id}/"), Value::Null).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"GetAgentRuntime after CreateAgentRuntime\": expected HTTP 200, got {status} \
         {code:?} ({fetched})"
    );
    assert_eq!(
        fetched["agentRuntimeArn"], created["agentRuntimeArn"],
        "case \"GetAgentRuntime after CreateAgentRuntime\": the runtime read back must be the \
         one created, got {} for {}",
        fetched["agentRuntimeArn"], created["agentRuntimeArn"]
    );
}
