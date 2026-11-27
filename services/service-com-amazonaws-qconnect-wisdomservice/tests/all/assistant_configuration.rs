// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the configuration an assistant REPORTS: the capability it was
//! provisioned with, the topic it streams through, the default AI agents bound to it and
//! its orchestrator configurations.
//!
//! The rule under test: every configuration member of the stored assistant is reported,
//! out of the record, by every operation that reports the assistant — so the create's own
//! answer, `GetAssistant` and `ListAssistants` agree, and a member whose record holds
//! nothing (the orchestrator list) is reported empty rather than omitted.
//!
//! The tests are TABLES: each row is one create input along an axis the rule generalises
//! over (name form and length, description, tag count, encryption configuration, signed
//! region), because none of those is what decides the reported configuration.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts; nothing reaches into the service's state struct. Amazon SNS
//! is mounted beside this service on the same engine, as the assembled router mounts them,
//! because the topic an assistant streams through is a resource of that service.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a row signs with unless it is exercising the region axis itself.
const REGION: &str = "us-east-1";

/// The `AssistantCapabilityType` an assistant created through this API reports, written
/// out here rather than imported from `state.rs`: the assertion states the value the
/// recorded wire answer carries instead of restating the implementation's constant. `V1`
/// is the legacy Amazon Connect Wisdom capability
/// (`smithy_query(query="shape",shape="AssistantCapabilityType")`).
const CAPABILITY_V2: &str = "V2";

/// The AI agent types a fresh assistant reports a default-agent binding for: every
/// `AIAgentType` the model declares except `ORCHESTRATION`, which has no default agent.
const DEFAULT_AI_AGENT_TYPES: [&str; 8] = [
    "ANSWER_RECOMMENDATION",
    "CASE_SUMMARIZATION",
    "EMAIL_GENERATIVE_ANSWER",
    "EMAIL_OVERVIEW",
    "EMAIL_RESPONSE",
    "MANUAL_SEARCH",
    "NOTE_TAKING",
    "SELF_SERVICE",
];

/// The one type whose binding reports `enabled` — the model's
/// `AIAgentConfigurationData$enabled` doc says disabling "is currently supported only
/// for the `ANSWER_RECOMMENDATION` AI Agent type" — and whose default binding is
/// reported disabled.
const ANSWER_RECOMMENDATION: &str = "ANSWER_RECOMMENDATION";

/// Every `AIAgentType` the model declares
/// (`smithy_query(query="shape",shape="AIAgentType")`), so the test can assert that the
/// one type outside [`DEFAULT_AI_AGENT_TYPES`] is bound for no fresh assistant.
const ALL_AI_AGENT_TYPES: [&str; 9] = [
    "ANSWER_RECOMMENDATION",
    "CASE_SUMMARIZATION",
    "EMAIL_GENERATIVE_ANSWER",
    "EMAIL_OVERVIEW",
    "EMAIL_RESPONSE",
    "MANUAL_SEARCH",
    "NOTE_TAKING",
    "ORCHESTRATION",
    "SELF_SERVICE",
];

/// One wire answer, reduced to what a client reads.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|v| !v.is_null())
    }

    fn string(&self, pointer: &str) -> Option<String> {
        self.field(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// An emulator with this service mounted on a fresh engine, with Amazon SNS beside it —
/// the topic an assistant is integrated with belongs to that service — driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, region: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{region}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn create_assistant(&self, region: &str, body: Value) -> Reply {
        self.send("POST", "/assistants", region, Some(body)).await
    }

    async fn get_assistant(&self, region: &str, assistant_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}"),
            region,
            None,
        )
        .await
    }

    async fn list_assistants(&self, region: &str) -> Reply {
        self.send("GET", "/assistants", region, None).await
    }
}

/// The modelled `Uuid` form: `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width && group.bytes().all(|b| b.is_ascii_hexdigit())
            })
        && value.bytes().all(|b| !b.is_ascii_uppercase())
}

/// The `Uuid` and the `:<QUALIFIER>` of a reported binding id, or `None` when the value
/// is not the modelled `UuidWithQualifier` form
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$`,
/// `smithy_query(query="shape",shape="UuidWithQualifier")`) with the qualifier PRESENT:
/// a binding the service itself made names the agent version that runs for the type.
fn uuid_and_qualifier(value: &str) -> Option<(&str, &str)> {
    let (uuid, qualifier) = value.split_once(':')?;
    let qualified = is_modelled_uuid(uuid)
        && !qualifier.is_empty()
        && qualifier
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'$');
    qualified.then_some((uuid, qualifier))
}

/// One row of the create table: what differs between rows is the create's input, never
/// the configuration the rule says is reported.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    region: &'static str,
    body: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "minimal create",
            region: REGION,
            body: json!({ "name": "minimal", "type": "AGENT" }),
        },
        Row {
            label: "description and three tags",
            region: REGION,
            body: json!({
                "name": "described.assistant_1",
                "type": "AGENT",
                "description": "An assistant, with punctuation. -",
                "tags": { "team": "alpha", "a.b": "beta", "cost-centre": "42" },
            }),
        },
        Row {
            label: "shortest legal name",
            region: "eu-west-3",
            body: json!({ "name": "a", "type": "AGENT" }),
        },
        Row {
            label: "longest legal name, customer managed key",
            region: "ap-southeast-2",
            body: json!({
                "name": "n".repeat(255),
                "type": "AGENT",
                "serverSideEncryptionConfiguration": { "kmsKeyId": "alias/aws/wisdom" },
            }),
        },
    ]
}

/// The ARN of the SNS topic an assistant streams through, written out here rather than
/// taken from the handler's helper, so the test pins the format: the topic is named after
/// the endpoint this service answers for and the assistant it belongs to,
/// `arn:aws:sns:<region>:<account>:wisdom-assistant-<assistantId>`.
/// `tests/all/assistant_chat_streaming_topic.rs` is where that ARN is resolved in Amazon
/// SNS itself.
fn expected_topic_arn(region: &str, assistant_id: &str) -> String {
    format!(
        "arn:aws:sns:{region}:{account}:wisdom-assistant-{assistant_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// Assert the configuration members of one reported assistant, `prefix` being the JSON
/// pointer of the assistant object (`/assistant` for a create or read, an indexed
/// summary for the list).
fn assert_reported_configuration(
    case: &str,
    reply: &Reply,
    prefix: &str,
    region: &str,
    assistant_id: &str,
) {
    let capability = reply.string(&format!("{prefix}/capabilityConfiguration/type"));
    assert_eq!(
        capability.as_deref(),
        Some(CAPABILITY_V2),
        "case {case:?}: expected capabilityConfiguration.type {CAPABILITY_V2}, \
         got {capability:?}"
    );

    let topic = reply.string(&format!("{prefix}/integrationConfiguration/topicIntegrationArn"));
    assert_eq!(
        topic,
        Some(expected_topic_arn(region, assistant_id)),
        "case {case:?}: expected the assistant's own SNS topic ARN, got {topic:?}"
    );

    let orchestrators = reply
        .body
        .pointer(&format!("{prefix}/orchestratorConfigurationList"))
        .cloned();
    assert_eq!(
        orchestrators,
        Some(json!([])),
        "case {case:?}: expected orchestratorConfigurationList reported empty (nothing \
         configures one), got {orchestrators:?}"
    );

    let bound = reply
        .body
        .pointer(&format!("{prefix}/aiAgentConfiguration"))
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no aiAgentConfiguration in {}", reply.body));
    let reported_types: BTreeSet<&str> = bound.keys().map(String::as_str).collect();
    let expected_types: BTreeSet<&str> = DEFAULT_AI_AGENT_TYPES.iter().copied().collect();
    assert_eq!(
        reported_types, expected_types,
        "case {case:?}: expected a binding for every AI agent type with a default agent, \
         got {reported_types:?}"
    );
    for agent_type in ALL_AI_AGENT_TYPES {
        let expected_bound = DEFAULT_AI_AGENT_TYPES.contains(&agent_type);
        assert_eq!(
            bound.contains_key(agent_type),
            expected_bound,
            "case {case:?}/{agent_type}: expected bound={expected_bound}, got {:?}",
            bound.get(agent_type)
        );
    }
    for (agent_type, binding) in bound {
        let id = binding.get("aiAgentId").and_then(Value::as_str);
        assert!(
            id.is_some_and(|id| uuid_and_qualifier(id).is_some()),
            "case {case:?}/{agent_type}: expected a UuidWithQualifier aiAgentId carrying \
             the bound agent version, got {id:?}"
        );
        // `enabled` is reported for the one type whose binding the model lets a caller
        // turn off, and that default binding is reported disabled.
        let expected_enabled = (agent_type == ANSWER_RECOMMENDATION).then_some(json!(false));
        let enabled = binding.get("enabled").cloned();
        assert_eq!(
            enabled, expected_enabled,
            "case {case:?}/{agent_type}: expected enabled={expected_enabled:?}, got {enabled:?}"
        );
    }
}

/// The reported configuration is the same through the create, both reads and the list,
/// for every row — and the AI agent ids are distinct per type and stable across reads.
#[tokio::test]
async fn an_assistants_configuration_is_reported_by_every_read() {
    for row in rows() {
        // One emulator per row, so the list below reports exactly this row's assistant.
        let emulator = Emulator::new();
        let case = row.label;
        let created = emulator.create_assistant(row.region, row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let assistant_id = created
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {case:?}: no assistantId in {}", created.body));

        assert_reported_configuration(case, &created, "/assistant", row.region, &assistant_id);

        let read = emulator.get_assistant(row.region, &assistant_id).await;
        assert_reported_configuration(case, &read, "/assistant", row.region, &assistant_id);
        // Read again: a report must not change what the next caller sees.
        let again = emulator.get_assistant(row.region, &assistant_id).await;
        assert_eq!(
            again.body.pointer("/assistant"),
            read.body.pointer("/assistant"),
            "case {case:?}: two reads of one assistant disagreed"
        );

        let listed = emulator.list_assistants(row.region).await;
        assert_reported_configuration(
            case,
            &listed,
            "/assistantSummaries/0",
            row.region,
            &assistant_id,
        );

        // Every default agent is its own resource, so no two bindings name one id.
        let ids: BTreeSet<String> = read
            .body
            .pointer("/assistant/aiAgentConfiguration")
            .and_then(Value::as_object)
            .map(|bound| {
                bound
                    .values()
                    .filter_map(|binding| binding.get("aiAgentId").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids.len(),
            DEFAULT_AI_AGENT_TYPES.len(),
            "case {case:?}: expected {} distinct aiAgentIds, got {ids:?}",
            DEFAULT_AI_AGENT_TYPES.len()
        );
        // And the ids a later read reports are the ids the create reported.
        let created_ids = created.body.pointer("/assistant/aiAgentConfiguration").cloned();
        let read_ids = read.body.pointer("/assistant/aiAgentConfiguration").cloned();
        assert_eq!(
            created_ids, read_ids,
            "case {case:?}: the create and the read report different bindings"
        );
    }
}

/// Two assistants are provisioned independently in what belongs to the ASSISTANT — the
/// streaming topic — and identically in what belongs to the SERVICE: the built-in AI
/// agents every assistant is bound to are the same agents.
#[tokio::test]
async fn two_assistants_get_their_own_topic_and_the_services_agents() {
    let emulator = Emulator::new();
    let mut topics = BTreeSet::new();
    let mut first_bindings: Option<Value> = None;

    for case in ["first assistant", "second assistant", "third assistant"] {
        let created = emulator
            .create_assistant(
                REGION,
                json!({ "name": format!("own-{case}").replace(' ', "-"), "type": "AGENT" }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let topic = created
            .string("/assistant/integrationConfiguration/topicIntegrationArn")
            .unwrap_or_else(|| panic!("case {case:?}: no topicIntegrationArn in {}", created.body));
        assert!(
            topics.insert(topic.clone()),
            "case {case:?}: topic {topic} was already another assistant's"
        );
        let bindings = created
            .body
            .pointer("/assistant/aiAgentConfiguration")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no aiAgentConfiguration"));
        match &first_bindings {
            None => first_bindings = Some(bindings),
            Some(expected) => assert_eq!(
                &bindings, expected,
                "case {case:?}: expected the same service-owned default agents every \
                 assistant is bound to ({expected:?}), got {bindings:?}"
            ),
        }
    }
}
