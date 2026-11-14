// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `bedrock`, shared by SIX emulated
//! services.
//!
//! `aws.auth#sigv4` names `bedrock` on every Bedrock-family model while each one serves
//! its own endpoint (`smithy_query(query="service_info")` on the six models):
//!
//! | service | endpoint prefix | protocol |
//! |---|---|---|
//! | Bedrock (control plane) | `bedrock` | restJson1 |
//! | Bedrock Runtime | `bedrock-runtime` | restJson1 |
//! | Bedrock Agent | `bedrock-agent` | restJson1 |
//! | Bedrock Agent Runtime | `bedrock-agent-runtime` | restJson1 |
//! | Bedrock Data Automation | `bedrock-data-automation` | restJson1 |
//! | Bedrock Data Automation Runtime | `bedrock-data-automation-runtime` | awsJson1_1 |
//!
//! Six endpoint prefixes, ONE signing name — so the signing name is explicitly not a
//! unique key across them. The router resolves a request's routing key from the SigV4
//! credential scope first, so all six see the key `bedrock`: a service that claims on the
//! signing name ALONE claims every sibling's traffic too, the router finds six claimants
//! and answers `500 AmbiguousService` for ALL of it. That is a code no AWS model
//! declares, and a 5xx where every modelled failure of these operations but
//! `InternalServerException` is a 4xx, so an SDK retries a request that can never
//! succeed. AWS separates them by endpoint host, which an emulator cannot use: a client
//! configured with one local endpoint sends the same `Host` for all six.
//!
//! Each one therefore also requires the request to address its OWN modelled wire surface
//! — the first URI path segment its model declares for the five `restJson1` members, and
//! the service the `X-Amz-Target` names for the `awsJson1_1` member, whose operations all
//! arrive as `POST /`. Where two of the models publish the SAME first segment — the
//! control plane and Data Automation both bind `POST /tagResource`, `/untagResource` and
//! `/listTagsForResource`; Bedrock Agent and Bedrock Agent Runtime both bind `/agents`,
//! `/flows`, `/knowledgebases` and `/tags` — the segment separates nothing, and
//! `service_common::bedrock` splits those spaces once for both sides: by the whole
//! modelled route where the two bind different routes, by the request's own trailing slash
//! for the ONE route shape whose two templates differ in nothing else
//! (`POST /agents/{agentId}/` against `POST /agents/{sessionId}`), by the TAGGED RESOURCE's
//! own type on the `/tags/{resourceArn}` space — each model enumerates the resource types
//! its own tagging admits, and the two enumerations are disjoint, so the plane that mints
//! the named resource is the claimant — and by a declared owner where nothing in the
//! request tells them apart (a tagging label naming no taggable resource of either plane
//! among them).
//!
//! This suite pins that: the claim table (who claims what), the property that the six
//! models' path spaces overlap ONLY where that rule declares a split, and that a request
//! to each member reaches that member's own service through the whole router — never the
//! router's `AmbiguousService` (500, several claimants) or `UnknownService` (404, none).
//!
//! One test takes the awsJson1_1 member one level further: EVERY operation it binds,
//! dispatched with the family mounted, answering that service's own modelled status and
//! error code. Dispatch is per request rather than per operation, so one unreachable
//! operation would mean all six were — which is exactly the state this suite exists to
//! keep out.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The two refusals the ROUTER itself produces, neither of which is an AWS error code: a
/// request that reaches exactly one service can answer neither.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: the router here runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Every emulated service whose SigV4 signing name is `bedrock`, over one engine, as
/// `core/router/src/lib.rs` mounts them.
fn bedrock_family(engine: &Arc<engine::InMemoryEngine>) -> Vec<Box<dyn Service>> {
    vec![
        Box::new(
            service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
    ]
}

/// The whole family mounted in a router, over the engine the caller keeps a handle on.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// The Smithy shape ids of the family members, as `identity().shape_id()` reports them.
const CONTROL_PLANE: &str = "com.amazonaws.bedrock#AmazonBedrockControlPlaneService";
const RUNTIME: &str = "com.amazonaws.bedrockruntime#AmazonBedrockFrontendService";
const AGENT: &str = "com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda";
const AGENT_RUNTIME: &str = "com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService";
const DATA_AUTOMATION: &str =
    "com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService";
const DATA_AUTOMATION_RUNTIME: &str =
    "com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService";

/// Which mounted services claim a `bedrock`-signed request for `method path`, with
/// `target` as its `X-Amz-Target` when it carries one (how an awsJson1_1 request names
/// the service and operation it is for).
fn claimants(
    family: &[Box<dyn Service>],
    method: &str,
    path: &str,
    target: Option<&str>,
) -> Vec<String> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("authorization", authorization("bedrock"));
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder.body(Bytes::new()).expect("build the request");
    let (parts, _body) = request.into_parts();
    family
        .iter()
        .filter(|service| service.can_handle(&parts))
        .map(|service| service.definition().identity().shape_id().to_string())
        .collect()
}

/// One row: a request under the shared signing name, and every service that may claim it.
struct Row {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    /// The `X-Amz-Target`, for a row addressed in the awsJson1_1 RPC form.
    target: Option<&'static str>,
    claimants: Vec<&'static str>,
}

/// RULE: a request signed under the shared `bedrock` signing name is claimed by the one
/// family member whose modelled wire surface it addresses — each `restJson1` member for
/// its own first URI path segment, the `awsJson1_1` member for a root `POST` whose
/// `X-Amz-Target` names IT, and, where two members publish the same first segment, the
/// one whose own model binds the request's route or the one the family rule declares the
/// owner of that segment.
#[test]
fn each_service_sharing_the_bedrock_signing_name_claims_only_its_own_surface() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let family = bedrock_family(&engine);

    // Rows differ along the wire surface they address: one row per member's own surface,
    // both sides of each path space two members share, near misses on a segment, the
    // root path with and without a target, and a path no member binds.
    let rows = [
        Row {
            case: "the issue's own call: CreateGuardrail",
            method: "POST",
            path: "/guardrails",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "GetGuardrail on the instance path",
            method: "GET",
            path: "/guardrails/abc123def456",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "ListFoundationModels",
            method: "GET",
            path: "/foundation-models",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "GetModelInvocationLoggingConfiguration",
            method: "GET",
            path: "/logging/modelinvocations",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "InvokeModel on the runtime",
            method: "POST",
            path: "/model/anthropic.claude-v2/invoke",
            target: None,
            claimants: vec![RUNTIME],
        },
        Row {
            case: "Converse on the runtime",
            method: "POST",
            path: "/model/anthropic.claude-v2/converse",
            target: None,
            claimants: vec![RUNTIME],
        },
        Row {
            case: "ConverseStream on the runtime",
            method: "POST",
            path: "/model/anthropic.claude-v2/converse-stream",
            target: None,
            claimants: vec![RUNTIME],
        },
        Row {
            // `guardrail` is the runtime's segment, `guardrails` the control plane's:
            // exact segment equality, never a prefix test, keeps them apart. So does the
            // runtime's `model` against the control plane's `model-copy-jobs`,
            // `model-customization`, `model-import-jobs` and `model-invocation-job(s)`.
            case: "ApplyGuardrail on the runtime",
            method: "POST",
            path: "/guardrail/abc123def456/version/1/apply",
            target: None,
            claimants: vec![RUNTIME],
        },
        Row {
            case: "StartAsyncInvoke on the runtime",
            method: "POST",
            path: "/async-invoke",
            target: None,
            claimants: vec![RUNTIME],
        },
        Row {
            case: "CreateKnowledgeBase on the agent plane",
            method: "PUT",
            path: "/knowledgebases/",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "CreateAgent on the agent plane",
            method: "PUT",
            path: "/agents/",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "CreateFlow on the agent plane",
            method: "POST",
            path: "/flows/",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "ListPrompts on the agent plane's own segment",
            method: "GET",
            path: "/prompts/",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            // `knowledgebases` is published by BOTH agent-family models, binding
            // different routes underneath, so the route decides: this one is the
            // runtime's `Retrieve`.
            case: "Retrieve on the agent runtime, inside the shared knowledgebases space",
            method: "POST",
            path: "/knowledgebases/kb-1/retrieve",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            // Its two `@required` query bindings are part of what names the operation,
            // exactly as they are when the router picks one: an SDK always sends them.
            case: "GetAgentMemory on the agent runtime, inside the shared agents space",
            method: "GET",
            path: "/agents/a-1/agentAliases/alias-1/memories\
                   ?memoryType=SESSION_SUMMARY&memoryId=m-1",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            case: "DeleteAgentMemory, the same path under another method",
            method: "DELETE",
            path: "/agents/a-1/agentAliases/alias-1/memories",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            // The one route shape BOTH models bind: the framework's matcher trims a
            // trailing slash, so `POST /agents/{sessionId}` (the runtime's
            // InvokeInlineAgent) and `POST /agents/{agentId}/` (the plane's PrepareAgent)
            // match the same two segments. The spelling a client renders from the model's
            // own URI is what decides.
            case: "InvokeInlineAgent on the agent runtime, with no trailing slash",
            method: "POST",
            path: "/agents/probe-session-1",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            case: "PrepareAgent on the agent plane, the same segments with the slash",
            method: "POST",
            path: "/agents/a-1/",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "GetAgent on the agent plane, inside the same shared space",
            method: "GET",
            path: "/agents/a-1/",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "StartFlowExecution on the agent runtime, inside the shared flows space",
            method: "POST",
            path: "/flows/f-1/aliases/alias-1/executions",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            // The flows space the two models share, one segment shallower: this route is
            // the agent plane's GetFlowAlias.
            case: "GetFlowAlias on the agent plane, inside the shared flows space",
            method: "GET",
            path: "/flows/f-1/aliases/alias-1",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "RetrieveAndGenerate on the agent runtime's own segment",
            method: "POST",
            path: "/retrieveAndGenerate",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            case: "CreateSession on the agent runtime's own segment",
            method: "PUT",
            path: "/sessions/",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            // Inside a space two models share, the claim is the exact modelled route, so
            // a path NEITHER binds reaches nobody and the router answers it rather than
            // picking one of the two. No AWS operation lives on such a path.
            case: "a path inside the shared knowledgebases space that no model binds",
            method: "POST",
            path: "/knowledgebases/kb-1/no-such-operation",
            target: None,
            claimants: vec![],
        },
        Row {
            // The agent pair bind `/tags/{resourceArn}` IDENTICALLY on all three methods,
            // so neither the method nor the URI template tells them apart — the ARN does:
            // each model enumerates the resource types its own tagging admits and the two
            // enumerations are disjoint, so the plane that MINTS the named resource is the
            // claimant. An `agent` is the build-time plane's.
            case: "the tagging path both agent models bind, with an agent ARN",
            method: "GET",
            path: "/tags/arn%3Aaws%3Abedrock%3Aus-east-1%3A%3Aagent%2Fa1",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            case: "the same tagging path as a POST",
            method: "POST",
            path: "/tags/arn%3Aaws%3Abedrock%3Aus-east-1%3A%3Aagent%2Fa1",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            // A knowledge base is the build-time plane's too, and its tags are what a
            // client reads back off a CloudFormation-created knowledge base.
            case: "the tagging path with a knowledge-base ARN",
            method: "GET",
            path: "/tags/arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Aknowledge-base%2F\
                   KB12345678",
            target: None,
            claimants: vec![AGENT],
        },
        Row {
            // A `session` ARN is the data plane's: it is the resource type the
            // `bedrock-agent-runtime` model's own taggable-resource pattern admits, and the
            // one this plane's tagging operations serve.
            case: "the tagging path with a session ARN",
            method: "POST",
            path: "/tags/arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Asession%2F\
                   0cabde93-58b4-4d68-b230-90df328e1acb",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            case: "reading a session's tags",
            method: "GET",
            path: "/tags/arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Asession%2F\
                   0cabde93-58b4-4d68-b230-90df328e1acb",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            case: "removing a session's tags",
            method: "DELETE",
            path: "/tags/arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Asession%2F\
                   0cabde93-58b4-4d68-b230-90df328e1acb?tagKeys=k",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            // A label that names no resource of this family is the declared owner's too,
            // so such a request still reaches exactly one claimant.
            case: "the tagging path with a label that is no bedrock ARN",
            method: "GET",
            path: "/tags/not-an-arn",
            target: None,
            claimants: vec![AGENT_RUNTIME],
        },
        Row {
            case: "CreateBlueprint on data automation",
            method: "PUT",
            path: "/blueprints/",
            target: None,
            claimants: vec![DATA_AUTOMATION],
        },
        Row {
            case: "CreateDataAutomationProject on data automation",
            method: "PUT",
            path: "/data-automation-projects/",
            target: None,
            claimants: vec![DATA_AUTOMATION],
        },
        Row {
            // The control plane and data automation bind `POST /tagResource` identically;
            // the declared owner is the control plane, which every Bedrock tagging flow
            // uses.
            case: "the camelCase tagging path both control planes bind",
            method: "POST",
            path: "/tagResource",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "its untag counterpart",
            method: "POST",
            path: "/untagResource",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "its list counterpart",
            method: "POST",
            path: "/listTagsForResource",
            target: None,
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "the awsJson1_1 member's root post, targeted at it",
            method: "POST",
            path: "/",
            target: Some("AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync"),
            claimants: vec![DATA_AUTOMATION_RUNTIME],
        },
        Row {
            case: "a root post with no X-Amz-Target names no service",
            method: "POST",
            path: "/",
            target: None,
            claimants: vec![],
        },
        Row {
            case: "a root post targeted at a service outside this family",
            method: "POST",
            path: "/",
            target: Some("DynamoDB_20120810.GetItem"),
            claimants: vec![],
        },
        Row {
            // The target names the awsJson1_1 member, which claims only its own
            // protocol's path; the path is the control plane's, which claims it whatever
            // the header says, so exactly one service still serves it.
            case: "a REST member's path carrying the awsJson member's target",
            method: "POST",
            path: "/guardrails",
            target: Some("AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync"),
            claimants: vec![CONTROL_PLANE],
        },
        Row {
            case: "a near miss on a first segment",
            method: "POST",
            path: "/guardrailsextra",
            target: None,
            claimants: vec![],
        },
        Row {
            case: "a first segment in the wrong case",
            method: "POST",
            path: "/Guardrails",
            target: None,
            claimants: vec![],
        },
        Row {
            case: "a path no bedrock model binds",
            method: "POST",
            path: "/not-a-bedrock-path",
            target: None,
            claimants: vec![],
        },
    ];

    for row in rows {
        let actual = claimants(&family, row.method, row.path, row.target);
        assert_eq!(
            actual, row.claimants,
            "case {:?}: expected {:?} to claim {} {} (target {:?}), got {actual:?}",
            row.case, row.claimants, row.method, row.path, row.target
        );
    }
}

/// The first URI path segments that MORE THAN ONE of the six models publishes, as
/// `service_common::bedrock`'s `SHARED_SEGMENTS` declares them. Restated here because a
/// router test target reaches only the crates the router depends on: this assertion is
/// what catches a model whose path space starts overlapping a sibling's somewhere the
/// family rule does not split, which would put two claimants back on that path.
const DECLARED_SHARED_SEGMENTS: &[&str] = &[
    "agents",
    "flows",
    "knowledgebases",
    "listTagsForResource",
    "tagResource",
    "tags",
    "untagResource",
];

/// RULE (the property the claim table rests on): the six models' first-path-segment
/// spaces overlap only where the family rule declares a split, and no REST member
/// publishes an EMPTY space — an empty one claims every path and swallows all five peers.
#[test]
fn the_six_path_spaces_overlap_only_where_the_family_rule_splits_them() {
    // (member, its generated ROUTE_PATH_SEGMENTS, whether its model is RPC)
    let members: [(&str, &[&str], bool); 6] = [
        (
            CONTROL_PLANE,
            service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ROUTE_PATH_SEGMENTS,
            false,
        ),
        (
            RUNTIME,
            service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ROUTE_PATH_SEGMENTS,
            false,
        ),
        (
            AGENT,
            service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::ROUTE_PATH_SEGMENTS,
            false,
        ),
        (
            AGENT_RUNTIME,
            service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::ROUTE_PATH_SEGMENTS,
            false,
        ),
        (
            DATA_AUTOMATION,
            service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::ROUTE_PATH_SEGMENTS,
            false,
        ),
        (
            // awsJson1_1 with no `smithy.api#http` bindings at all, so its generated
            // space is empty by construction and its claim is by the service its
            // `X-Amz-Target` names instead.
            DATA_AUTOMATION_RUNTIME,
            service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::ROUTE_PATH_SEGMENTS,
            true,
        ),
    ];

    for (member, segments, rpc) in members {
        assert_eq!(
            segments.is_empty(),
            rpc,
            "case {member:?}: a REST member must publish a non-empty path space and the \
             RPC member an empty one, got {segments:?}"
        );
    }

    for (index, (member, segments, _)) in members.iter().enumerate() {
        for (peer, peer_segments, _) in members.iter().skip(index + 1) {
            for segment in segments.iter() {
                if !peer_segments.contains(segment) {
                    continue;
                }
                assert!(
                    DECLARED_SHARED_SEGMENTS.contains(segment),
                    "case {member:?} vs {peer:?}: both publish the first segment \
                     {segment:?}, which the family rule does not split — every request \
                     under it has two claimants and is refused 500 AmbiguousService"
                );
            }
        }
    }

    // And the other direction: a segment the rule declares shared must really be
    // published by two or more of the six, or the split narrows a space that has one
    // owner anyway and makes part of it unreachable.
    for segment in DECLARED_SHARED_SEGMENTS {
        let publishers: Vec<&str> = members
            .iter()
            .filter(|(_, segments, _)| segments.contains(segment))
            .map(|(member, _, _)| *member)
            .collect();
        assert!(
            publishers.len() >= 2,
            "case {segment:?}: the family rule splits this segment, so two or more \
             models must publish it, got {publishers:?}"
        );
    }
}

/// One answer from the whole router, reduced to what the rows assert on.
struct Answer {
    status: u16,
    error_code: String,
    content_type: String,
    body: Value,
    raw_body: String,
}

/// Dispatch one `bedrock`-signed request through the whole router.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("content-type", "application/json")
        .header("authorization", authorization("bedrock"));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw_body = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str::<Value>(&raw_body).unwrap_or(Value::Null);
    // restJson1 names the error shape in `x-amzn-errortype`, and the router's own
    // refusals set `__type` in the body; read both so `AmbiguousService` and
    // `UnknownService` are visible whichever way they arrive.
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_code,
        content_type,
        body,
        raw_body,
    }
}

/// One request to dispatch through the whole router: the case it stands for, its method,
/// path, extra headers and body. Every one is signed for `bedrock`, which is the point.
struct Call<'a> {
    case: &'a str,
    method: &'a str,
    path: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: &'a str,
}

/// RULE: with the whole family mounted, a request to any member reaches THAT member's own
/// service, so the answer is that service's own — never the router's `AmbiguousService`
/// refusal (two or more claimants) and never its `UnknownService` one (none), and never a
/// 5xx for a well-formed request on a path its model binds.
#[tokio::test]
async fn every_member_is_reachable_through_the_whole_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Rows differ along the member they address and the wire surface that names it (a
    // path for the five REST members, the RPC `X-Amz-Target` for the awsJson1_1 one),
    // spanning the operations the behavioral checks call first on each.
    let calls: [Call<'_>; 12] = [
        Call {
            case: "CreateGuardrail on the control plane",
            method: "POST",
            path: "/guardrails",
            headers: &[],
            body: r#"{"name":"reachability","blockedInputMessaging":"no","blockedOutputsMessaging":"no"}"#,
        },
        Call {
            case: "ListGuardrails on the control plane",
            method: "GET",
            path: "/guardrails",
            headers: &[],
            body: "",
        },
        Call {
            case: "ListFoundationModels on the control plane",
            method: "GET",
            path: "/foundation-models",
            headers: &[],
            body: "",
        },
        Call {
            case: "GetModelInvocationLoggingConfiguration on the control plane",
            method: "GET",
            path: "/logging/modelinvocations",
            headers: &[],
            body: "",
        },
        Call {
            case: "Converse on the runtime",
            method: "POST",
            path: "/model/anthropic.claude-v2/converse",
            headers: &[],
            body: r#"{"messages":[{"role":"user","content":[{"text":"hello"}]}]}"#,
        },
        Call {
            case: "InvokeModel on the runtime",
            method: "POST",
            path: "/model/anthropic.claude-v2/invoke",
            headers: &[],
            body: r#"{"prompt":"hello"}"#,
        },
        Call {
            case: "CreateKnowledgeBase on the agent plane",
            method: "PUT",
            path: "/knowledgebases/",
            headers: &[],
            body: r#"{"name":"kb"}"#,
        },
        Call {
            case: "ListPrompts on the agent plane",
            method: "GET",
            path: "/prompts/",
            headers: &[],
            body: "",
        },
        Call {
            case: "Retrieve on the agent runtime",
            method: "POST",
            path: "/knowledgebases/kb-1/retrieve",
            headers: &[],
            body: r#"{"retrievalQuery":{"text":"hello"}}"#,
        },
        Call {
            case: "CreateSession on the agent runtime",
            method: "PUT",
            path: "/sessions/",
            headers: &[],
            body: "{}",
        },
        Call {
            case: "ListBlueprints on data automation",
            method: "POST",
            path: "/blueprints/",
            headers: &[],
            body: "{}",
        },
        Call {
            // The awsJson1_1 member: every operation posts to the ROOT path and names the
            // service in `X-Amz-Target`, so no path can claim it and no path claim of its
            // five REST peers may swallow it.
            case: "GetDataAutomationStatus on the data-automation runtime",
            method: "POST",
            path: "/",
            headers: &[(
                "x-amz-target",
                "AmazonBedrockKeystoneRuntimeService.GetDataAutomationStatus",
            )],
            body: r#"{"invocationArn":"arn:aws:bedrock:us-east-1:000000000000:data-automation-invocation/absent"}"#,
        },
    ];

    for call in calls {
        let answer = dispatch(&router, call.method, call.path, call.headers, call.body).await;
        assert!(
            !ROUTER_REFUSALS.contains(&answer.error_code.as_str()),
            "case {:?}: {} {} must reach exactly one of the six services sharing the \
             `bedrock` signing name, got HTTP {} {:?}: {}",
            call.case,
            call.method,
            call.path,
            answer.status,
            answer.error_code,
            answer.raw_body
        );
        assert_ne!(
            answer.status, 500,
            "case {:?}: {} {} must not answer a 5xx the SDK retries, got {:?}: {}",
            call.case, call.method, call.path, answer.error_code, answer.raw_body
        );
    }
}

/// RULE: the member that OWNS the path is the one that answers it — the suite pins WHICH
/// service served the request, not only that exactly one did.
#[tokio::test]
async fn the_service_that_owns_the_path_is_the_one_that_answers() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Bedrock Data Automation's `CreateDataAutomationProject` answers its modelled 201
    // with a `data-automation-project` ARN; no other member of the family has that
    // operation at all.
    let answer = dispatch(
        &router,
        "PUT",
        "/data-automation-projects/",
        &[],
        r#"{"projectName":"routing-owner-check","standardOutputConfiguration":{}}"#,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (201, ""),
        "CreateDataAutomationProject must answer its modelled 201, got {}",
        answer.raw_body
    );
    assert!(
        answer.raw_body.contains(":data-automation-project/"),
        "the 201 must carry a data-automation-project ARN, got {}",
        answer.raw_body
    );

    // The control plane's `ListGuardrails` answers its own listing on a fresh engine;
    // Bedrock Runtime's `/guardrail/…` is a different segment, so only the control plane
    // can produce this.
    let answer = dispatch(&router, "GET", "/guardrails", &[], "").await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (200, ""),
        "ListGuardrails must answer the control plane's own listing, got {}",
        answer.raw_body
    );
}

/// RULE (the complement): narrowing each claim to the member's own wire surface must not
/// leave some member claiming EVERY `bedrock`-signed path. A path none of the six binds is
/// the router's 404 `UnknownService` — one claimant too few, never the 500 that says
/// several claimed it.
#[tokio::test]
async fn a_bedrock_signed_request_on_a_path_no_member_binds_reaches_nobody() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let rows: [Call<'_>; 3] = [
        Call {
            case: "a path no bedrock-family model binds",
            method: "POST",
            path: "/not-a-bedrock-path",
            headers: &[],
            body: "{}",
        },
        Call {
            case: "an X-Amz-Target naming no bedrock-family service, on the root path",
            method: "POST",
            path: "/",
            headers: &[("x-amz-target", "SomeOtherService.SomeOperation")],
            body: "{}",
        },
        Call {
            // Inside a space two of the models share, the claim is the exact modelled
            // route, so a path neither binds reaches nobody rather than both.
            case: "a path inside the shared knowledgebases space that no model binds",
            method: "POST",
            path: "/knowledgebases/kb-1/no-such-operation",
            headers: &[],
            body: "{}",
        },
    ];

    for call in rows {
        let answer = dispatch(&router, call.method, call.path, call.headers, call.body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "UnknownService"),
            "case {:?}: expected the router's 404 UnknownService, got {} {:?} with {}",
            call.case,
            answer.status,
            answer.error_code,
            answer.raw_body
        );
    }
}

/// RULE: a guardrail created through the whole router is readable back through it, and
/// both answers are the control plane's own `restJson1` envelope — the proof that the
/// request reached the Bedrock control plane's handlers rather than a sibling or the
/// router's refusal.
#[tokio::test]
async fn a_guardrail_created_through_the_router_reads_back_through_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Rows differ along the policy set a guardrail carries, which is what CreateGuardrail
    // generalises over; the id, ARN and DRAFT version must come back for each.
    let bodies: [(&str, &str); 3] = [
        (
            "the messaging members alone",
            r#"{"name":"routing-plain","blockedInputMessaging":"blocked in","blockedOutputsMessaging":"blocked out"}"#,
        ),
        (
            "with a denied topic",
            r#"{"name":"routing-topic","blockedInputMessaging":"blocked in","blockedOutputsMessaging":"blocked out","topicPolicyConfig":{"topicsConfig":[{"name":"Legal","definition":"Legal advice of any kind.","type":"DENY"}]}}"#,
        ),
        (
            "with a word filter and a description",
            r#"{"name":"routing-words","description":"a filter","blockedInputMessaging":"blocked in","blockedOutputsMessaging":"blocked out","wordPolicyConfig":{"wordsConfig":[{"text":"forbidden"}]}}"#,
        ),
    ];

    for (case, body) in bodies {
        let created = dispatch(&router, "POST", "/guardrails", &[], body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateGuardrail must answer its modelled 202, got {} {:?}: {}",
            created.status, created.error_code, created.raw_body
        );
        assert!(
            created.content_type.starts_with("application/json"),
            "case {case:?}: a restJson1 response carries application/json, got {:?}",
            created.content_type
        );
        let identifier = created
            .body
            .get("guardrailId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no guardrailId in {}", created.raw_body))
            .to_string();
        assert_eq!(
            created.body.get("version").and_then(Value::as_str),
            Some("DRAFT"),
            "case {case:?}: a created guardrail is its DRAFT version, got {}",
            created.raw_body
        );

        let read = dispatch(&router, "GET", &format!("/guardrails/{identifier}"), &[], "").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetGuardrail on {identifier} must answer 200, got {} {:?}: {}",
            read.status, read.error_code, read.raw_body
        );
        assert_eq!(
            read.body.get("guardrailId").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {case:?}: the read-back guardrail is the created one, got {}",
            read.raw_body
        );
    }
}

/// A legal `dataAutomationProfileArn` (`smithy_query` shape `DataAutomationProfileArn`,
/// whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-east-1:aws:data-automation-profile/us.data-automation-v1";

/// `CreateDataAutomationProject` on the build-time member, answering with the
/// `projectArn` it minted.
///
/// A submission's `dataAutomationConfiguration.dataAutomationProjectArn` is RESOLVED
/// against that member's store, so the project an accepted submission names has to be
/// created through the router first — which is also what makes the accepted row evidence
/// that the two members of the family share one engine.
async fn create_project(router: &MultiServiceRouter, name: &str) -> String {
    let answer = dispatch(
        router,
        "PUT",
        "/data-automation-projects/",
        &[],
        &format!(r#"{{"projectName":"{name}","standardOutputConfiguration":{{}}}}"#),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (201, ""),
        "CreateDataAutomationProject({name:?}) must answer its modelled 201, got {}",
        answer.raw_body
    );
    answer
        .body
        .get("projectArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("CreateDataAutomationProject({name:?}) returned no projectArn: {}", answer.raw_body)
        })
        .to_string()
}

/// A well-formed `InvocationArn` / `TaggableResourceArn` that names nothing this emulator
/// minted.
const ABSENT_INVOCATION_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:data-automation-invocation/absent";

/// EVERY operation of the awsJson1_1 member answers its own modelled contract with the
/// whole family mounted — not only the one operation the row above dispatches.
///
/// The rows differ along the operation and, within an operation, along what the answer
/// depends on (an accepted submission, a submission naming nothing to extract, an ARN
/// that names nothing), so a row passes only if the request reached THAT service's
/// handlers and they computed the answer. Before the claim was narrowed, every one of
/// these was `500 AmbiguousService`, produced before any handler ran.
#[tokio::test]
async fn every_operation_of_the_awsjson_member_answers_its_own_contract() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // The accepted row names a project created through the build-time member of the same
    // family, because the runtime member resolves that reference against its store.
    let project_arn = create_project(&router, "routing-contract-project").await;
    let submission = format!(
        r#"{{"inputConfiguration":{{"s3Uri":"s3://in-bucket/assets/doc.pdf"}},
             "outputConfiguration":{{"s3Uri":"s3://out-bucket/routing/"}},
             "dataAutomationProfileArn":"{PROFILE_ARN}",
             "dataAutomationConfiguration":{{"dataAutomationProjectArn":"{project_arn}"}}}}"#
    );
    let no_extraction_target = format!(
        r#"{{"inputConfiguration":{{"s3Uri":"s3://in-bucket/assets/doc.pdf"}},
             "outputConfiguration":{{"s3Uri":"s3://out-bucket/routing/"}},
             "dataAutomationProfileArn":"{PROFILE_ARN}"}}"#
    );

    // (case, operation, body, expected status, expected error code)
    let rows: [(&str, &str, String, u16, Option<&str>); 7] = [
        (
            "an accepted asynchronous submission",
            "InvokeDataAutomationAsync",
            submission,
            200,
            None,
        ),
        (
            "an asynchronous submission naming nothing to extract",
            "InvokeDataAutomationAsync",
            no_extraction_target.clone(),
            400,
            Some("ValidationException"),
        ),
        (
            "a synchronous submission naming nothing to extract",
            "InvokeDataAutomation",
            no_extraction_target,
            400,
            Some("ValidationException"),
        ),
        (
            "a status read of an invocation that does not exist",
            "GetDataAutomationStatus",
            format!(r#"{{"invocationArn":"{ABSENT_INVOCATION_ARN}"}}"#),
            400,
            Some("ResourceNotFoundException"),
        ),
        (
            "a tag edit on an invocation that does not exist",
            "TagResource",
            format!(
                r#"{{"resourceARN":"{ABSENT_INVOCATION_ARN}",
                     "tags":[{{"key":"env","value":"dev"}}]}}"#
            ),
            400,
            Some("ResourceNotFoundException"),
        ),
        (
            "a tag read of an invocation that does not exist",
            "ListTagsForResource",
            format!(r#"{{"resourceARN":"{ABSENT_INVOCATION_ARN}"}}"#),
            400,
            Some("ResourceNotFoundException"),
        ),
        (
            "a tag removal on an invocation that does not exist",
            "UntagResource",
            format!(r#"{{"resourceARN":"{ABSENT_INVOCATION_ARN}","tagKeys":["env"]}}"#),
            400,
            Some("ResourceNotFoundException"),
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in &rows {
        let target = format!("AmazonBedrockKeystoneRuntimeService.{operation}");
        let answer = dispatch(
            &router,
            "POST",
            "/",
            &[("x-amz-target", target.as_str())],
            body.as_str(),
        )
        .await;
        assert!(
            !ROUTER_REFUSALS.contains(&answer.error_code.as_str()),
            "case {case:?} ({target}): routing failed with {}, got {} with {}",
            answer.error_code,
            answer.status,
            answer.raw_body
        );
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (*expected_status, expected_code.unwrap_or_default()),
            "case {case:?} ({target}): expected HTTP {expected_status} {expected_code:?}, \
             got {} with {}",
            answer.status,
            answer.raw_body
        );
    }
}
