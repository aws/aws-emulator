// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONFIGURATION state of an assistant's AI-agent bindings and
//! of a session, as every operation that reports those resources renders it.
//!
//! Two rules are under test:
//!
//! 1. A binding the SERVICE itself made names the agent by its `Uuid` AND the
//!    `:<VERSION>` qualifier of the agent version bound (the modelled
//!    `UuidWithQualifier`, `smithy_query(query="shape",shape="UuidWithQualifier")`), the
//!    same stored value on every later report of that resource; an id a CALLER configured
//!    is reported exactly as given.
//! 2. Every modelled configuration member of a stored session is reported out of the
//!    record by every operation that reports the session: the AI agent bound per type (its
//!    assistant's bindings, overlaid by the session's own), the SNS topic its assistant
//!    streams through, and its orchestrator configurations — reported empty rather than
//!    omitted when it holds none.
//!
//! The tests are TABLES: the assistant rows differ along the create input (name form and
//! length, tag count, description, encryption configuration, signing region), and the
//! session rows along what the create configures for the session itself (no override, a
//! bare-uuid override, a version-qualified override, several overrides, one and two
//! orchestrator entries).
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts; nothing reaches into the service's state struct. Amazon SNS
//! is mounted beside this service on the same engine, as the assembled router mounts them,
//! because the topic a session streams through is a resource of that service.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a row signs with unless it is exercising the region axis itself.
const REGION: &str = "us-east-1";

/// The AI agent types a fresh assistant reports a default-agent binding for: every
/// `AIAgentType` the model declares (`smithy_query(query="shape",shape="AIAgentType")`)
/// except `ORCHESTRATION`, which has no default agent.
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

/// The qualifier that names an AI agent's floating alias rather than one of its published
/// versions, which is what the service's own binding for the answer-recommendation type
/// carries.
const QUALIFIER_LATEST: &str = "$LATEST";

/// The one `AIAgentType` whose service-made binding names the agent's floating alias; the
/// service pins a published version for every other type. Source: the recorded wire
/// answers of `CreateAssistant`, `CreateSession` and `GetSession`.
const FLOATING_DEFAULT_AI_AGENT_TYPE: &str = "ANSWER_RECOMMENDATION";

/// Ids a caller may configure, each matching the modelled `UuidWithQualifier` pattern:
/// a bare uuid, one pinned to a numbered version, and a third bare uuid for the rows that
/// need two distinct agents.
const CALLER_AGENT_BARE: &str = "11111111-2222-3333-4444-555555555555";
const CALLER_AGENT_VERSIONED: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:7";
const CALLER_AGENT_OTHER: &str = "0123abcd-4567-89ef-0123-456789abcdef";

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

    fn object(&self, pointer: &str) -> Option<Map<String, Value>> {
        self.field(pointer).and_then(Value::as_object).cloned()
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
        self.send("GET", &format!("/assistants/{assistant_id}"), region, None)
            .await
    }

    async fn list_assistants(&self, region: &str) -> Reply {
        self.send("GET", "/assistants", region, None).await
    }

    async fn create_session(&self, region: &str, assistant_id: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{assistant_id}/sessions"),
            region,
            Some(body),
        )
        .await
    }

    async fn get_session(&self, region: &str, assistant_id: &str, session_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{session_id}"),
            region,
            None,
        )
        .await
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

/// The `Uuid` and the qualifier of a binding id the SERVICE made, or `None` when the
/// value is not the modelled `UuidWithQualifier` form
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$`,
/// `smithy_query(query="shape",shape="UuidWithQualifier")`) WITH the qualifier present.
fn uuid_and_qualifier(value: &str) -> Option<(&str, &str)> {
    let (uuid, qualifier) = value.split_once(':')?;
    let well_formed = is_modelled_uuid(uuid)
        && !qualifier.is_empty()
        && qualifier
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'$');
    well_formed.then_some((uuid, qualifier))
}

/// The ARN of the SNS topic an assistant streams through, written out here rather than
/// taken from the handler's helper, so the test pins the format: the topic is named after
/// the endpoint this service answers for and the assistant it belongs to,
/// `arn:aws:sns:<region>:<account>:wisdom-assistant-<assistantId>`.
fn expected_topic_arn(region: &str, assistant_id: &str) -> String {
    format!(
        "arn:aws:sns:{region}:{account}:wisdom-assistant-{assistant_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// One row of the assistant table: the create input differs, the rule about the reported
/// bindings does not.
#[derive(Debug, Clone)]
struct AssistantRow {
    label: &'static str,
    region: &'static str,
    body: Value,
}

fn assistant_rows() -> Vec<AssistantRow> {
    vec![
        AssistantRow {
            label: "minimal create",
            region: REGION,
            body: json!({ "name": "agents-minimal", "type": "AGENT" }),
        },
        AssistantRow {
            label: "described, three tags",
            region: REGION,
            body: json!({
                "name": "agents.described_1",
                "type": "AGENT",
                "description": "Bindings, described. -",
                "tags": { "owner": "qconnect-test", "a.b": "beta", "cost-centre": "42" },
            }),
        },
        AssistantRow {
            label: "shortest legal name, another region",
            region: "eu-west-3",
            body: json!({ "name": "a", "type": "AGENT" }),
        },
        AssistantRow {
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

/// Assert every binding of one reported `aiAgentConfiguration` map is a service-made id
/// qualified by the revision of the agent the service bound, and hand back the map so two
/// reports can be compared.
fn assert_service_bindings(case: &str, bound: &Map<String, Value>) {
    let reported: BTreeSet<&str> = bound.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = DEFAULT_AI_AGENT_TYPES.iter().copied().collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected a binding for every AI agent type with a default agent, \
         got {reported:?}"
    );
    let mut uuids: BTreeSet<&str> = BTreeSet::new();
    for (agent_type, binding) in bound {
        let id = binding.get("aiAgentId").and_then(Value::as_str);
        let parsed = id.and_then(uuid_and_qualifier);
        let Some((uuid, qualifier)) = parsed else {
            panic!(
                "case {case:?}/{agent_type}: expected a UuidWithQualifier aiAgentId naming \
                 the bound agent version, got {id:?}"
            );
        };
        assert!(
            uuids.insert(uuid),
            "case {case:?}/{agent_type}: agent {uuid} is already bound to another type"
        );
        // The qualifier of a service-made binding names the revision of the agent it
        // bound: the floating alias for the one type the service binds to the agent
        // itself, and the agent's published version number (the modelled `Version` shape
        // is `range 1..`) for every other type.
        if agent_type == FLOATING_DEFAULT_AI_AGENT_TYPE {
            assert_eq!(
                qualifier, QUALIFIER_LATEST,
                "case {case:?}/{agent_type}: expected the floating qualifier \
                 {QUALIFIER_LATEST}, got {qualifier:?}"
            );
        } else {
            let version = qualifier.parse::<i64>().ok();
            assert!(
                version.is_some_and(|version| version >= 1),
                "case {case:?}/{agent_type}: expected the bound agent's published version \
                 number (at least 1) as the qualifier, got {qualifier:?}"
            );
        }
    }
}

/// Rule 1: the bindings a create reports are qualified by the revision of the agent the
/// service bound, and are the SAME stored values `GetAssistant` and `ListAssistants`
/// report.
#[tokio::test]
async fn service_made_ai_agent_bindings_are_revision_qualified_and_stable() {
    for row in assistant_rows() {
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
        let created_bindings = created
            .object("/assistant/aiAgentConfiguration")
            .unwrap_or_else(|| panic!("case {case:?}: no aiAgentConfiguration in {}", created.body));
        assert_service_bindings(case, &created_bindings);

        let read = emulator.get_assistant(row.region, &assistant_id).await;
        let read_bindings = read
            .object("/assistant/aiAgentConfiguration")
            .unwrap_or_else(|| panic!("case {case:?}: no aiAgentConfiguration in {}", read.body));
        assert_service_bindings(case, &read_bindings);
        assert_eq!(
            read_bindings, created_bindings,
            "case {case:?}: the read reported bindings the create did not: \
             {read_bindings:?} vs {created_bindings:?}"
        );

        let listed = emulator.list_assistants(row.region).await;
        let listed_bindings = listed
            .object("/assistantSummaries/0/aiAgentConfiguration")
            .unwrap_or_else(|| panic!("case {case:?}: no summary bindings in {}", listed.body));
        assert_eq!(
            listed_bindings, created_bindings,
            "case {case:?}: the summary reported bindings the create did not: \
             {listed_bindings:?} vs {created_bindings:?}"
        );
        // The summary reports the tags the create was given, too, since a client selects
        // assistants by them.
        let listed_tags = listed.field("/assistantSummaries/0/tags").cloned();
        let expected_tags = Some(row.body.get("tags").cloned().unwrap_or_else(|| json!({})));
        assert_eq!(
            listed_tags, expected_tags,
            "case {case:?}: expected the summary to report the tags set on create \
             {expected_tags:?}, got {listed_tags:?}"
        );
    }
}

/// One row of the session table: what the create configures for the session itself.
#[derive(Debug, Clone)]
struct SessionRow {
    label: &'static str,
    /// The session's `name`, which the modelled `Name` pattern bounds
    /// (`^[a-zA-Z0-9\s_.,-]+`, len 1..255).
    name: &'static str,
    /// The `aiAgentConfiguration` the create names, as a JSON object (or `None`).
    configured: Option<Value>,
    /// The `orchestratorConfigurationList` the create names (or `None`).
    orchestrators: Option<Value>,
    tags: BTreeMap<String, String>,
}

fn session_rows() -> Vec<SessionRow> {
    vec![
        SessionRow {
            label: "nothing configured: everything inherited",
            name: "inherited-only",
            configured: None,
            orchestrators: None,
            tags: BTreeMap::new(),
        },
        SessionRow {
            label: "one bare-uuid override",
            name: "one-override",
            configured: Some(json!({
                "MANUAL_SEARCH": { "aiAgentId": CALLER_AGENT_BARE }
            })),
            orchestrators: None,
            tags: [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
        },
        SessionRow {
            label: "a version-qualified override and a disabled type",
            name: "Qualified Override_1.,-",
            configured: Some(json!({
                "SELF_SERVICE": { "aiAgentId": CALLER_AGENT_VERSIONED },
                "ANSWER_RECOMMENDATION": { "aiAgentId": CALLER_AGENT_BARE, "enabled": false },
            })),
            orchestrators: Some(json!([
                { "orchestratorUseCase": "Connect.SelfService", "aiAgentId": CALLER_AGENT_BARE }
            ])),
            tags: BTreeMap::new(),
        },
        SessionRow {
            label: "a type with no default agent, two orchestrator entries",
            name: "no-default-agent-type",
            configured: Some(json!({
                "ORCHESTRATION": { "aiAgentId": CALLER_AGENT_VERSIONED },
                "NOTE_TAKING": { "aiAgentId": CALLER_AGENT_OTHER, "enabled": true },
            })),
            orchestrators: Some(json!([
                { "orchestratorUseCase": "Connect.SelfService" },
                {
                    "orchestratorUseCase": "Connect.AgentAssistance",
                    "aiAgentId": CALLER_AGENT_VERSIONED,
                },
            ])),
            tags: [
                ("owner".to_string(), "qconnect-test".to_string()),
                ("a.b".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ]
}

impl SessionRow {
    fn create_body(&self) -> Value {
        let mut body = json!({ "name": self.name });
        if let Some(configured) = &self.configured {
            body["aiAgentConfiguration"] = configured.clone();
        }
        if let Some(orchestrators) = &self.orchestrators {
            body["orchestratorConfigurationList"] = orchestrators.clone();
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }

    /// The bindings this session must report: the assistant's, overlaid per AI agent type
    /// by what this row configured — each configured id reported exactly as given.
    fn expected_bindings(&self, inherited: &Map<String, Value>) -> Map<String, Value> {
        let mut expected = inherited.clone();
        if let Some(configured) = self.configured.as_ref().and_then(Value::as_object) {
            for (agent_type, binding) in configured {
                expected.insert(agent_type.clone(), binding.clone());
            }
        }
        expected
    }
}

/// Assert the configuration members of one reported session.
fn assert_reported_session_configuration(
    case: &str,
    reply: &Reply,
    row: &SessionRow,
    inherited: &Map<String, Value>,
    region: &str,
    assistant_id: &str,
) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );

    let bindings = reply.object("/session/aiAgentConfiguration");
    let expected = row.expected_bindings(inherited);
    assert_eq!(
        bindings.as_ref(),
        Some(&expected),
        "case {case:?}: expected the session's bindings to be the assistant's overlaid \
         with what the create configured ({expected:?}), got {bindings:?}"
    );

    let topic = reply.string("/session/integrationConfiguration/topicIntegrationArn");
    assert_eq!(
        topic,
        Some(expected_topic_arn(region, assistant_id)),
        "case {case:?}: expected the parent assistant's SNS topic ARN, got {topic:?}"
    );

    let orchestrators = reply.body.pointer("/session/orchestratorConfigurationList").cloned();
    let expected_orchestrators = Some(row.orchestrators.clone().unwrap_or_else(|| json!([])));
    assert_eq!(
        orchestrators, expected_orchestrators,
        "case {case:?}: expected orchestratorConfigurationList {expected_orchestrators:?} \
         (reported empty, not omitted, when the session configures none), \
         got {orchestrators:?}"
    );
}

/// Rule 2: the session's configuration members are reported out of the stored record by
/// the create and by every later read, for every row.
#[tokio::test]
async fn a_sessions_configuration_is_reported_by_the_create_and_every_read() {
    let emulator = Emulator::new();
    let created_assistant = emulator
        .create_assistant(
            REGION,
            json!({ "name": "session-configuration-parent", "type": "AGENT" }),
        )
        .await;
    assert_eq!(
        created_assistant.status, 200,
        "expected HTTP 200 creating the parent assistant, got {} with body {}",
        created_assistant.status, created_assistant.body
    );
    let assistant_id = created_assistant
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", created_assistant.body));
    let inherited = created_assistant
        .object("/assistant/aiAgentConfiguration")
        .unwrap_or_else(|| panic!("no aiAgentConfiguration in {}", created_assistant.body));

    for row in session_rows() {
        let case = row.label;
        let created = emulator
            .create_session(REGION, &assistant_id, row.create_body())
            .await;
        assert_reported_session_configuration(
            case,
            &created,
            &row,
            &inherited,
            REGION,
            &assistant_id,
        );
        let session_id = created
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {}", created.body));

        let read = emulator
            .get_session(REGION, &assistant_id, &session_id)
            .await;
        assert_reported_session_configuration(case, &read, &row, &inherited, REGION, &assistant_id);
        assert_eq!(
            read.field("/session"),
            created.field("/session"),
            "case {case:?}: the read reported a session the create did not"
        );
        // A report must not change what the next caller sees.
        let again = emulator
            .get_session(REGION, &assistant_id, &session_id)
            .await;
        assert_eq!(
            again.field("/session"),
            read.field("/session"),
            "case {case:?}: two reads of one session disagreed"
        );
    }
}

/// A session inherits ITS OWN assistant's configuration: two assistants' sessions report
/// different bindings and different integration topics.
#[tokio::test]
async fn a_session_inherits_only_its_own_assistants_configuration() {
    let emulator = Emulator::new();
    let mut topics: BTreeSet<String> = BTreeSet::new();

    for case in ["first parent", "second parent", "third parent"] {
        let assistant = emulator
            .create_assistant(
                REGION,
                json!({ "name": format!("parent-{case}").replace(' ', "-"), "type": "AGENT" }),
            )
            .await;
        let assistant_id = assistant
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {case:?}: no assistantId in {}", assistant.body));
        let bindings = assistant
            .object("/assistant/aiAgentConfiguration")
            .unwrap_or_else(|| panic!("case {case:?}: no aiAgentConfiguration"));

        let session = emulator
            .create_session(REGION, &assistant_id, json!({ "name": "inherited" }))
            .await;
        let session_bindings = session.object("/session/aiAgentConfiguration");
        assert_eq!(
            session_bindings.as_ref(),
            Some(&bindings),
            "case {case:?}: expected the session to report its own assistant's bindings \
             {bindings:?}, got {session_bindings:?}"
        );
        let topic = session
            .string("/session/integrationConfiguration/topicIntegrationArn")
            .unwrap_or_else(|| panic!("case {case:?}: no topicIntegrationArn in {}", session.body));
        assert_eq!(
            topic,
            expected_topic_arn(REGION, &assistant_id),
            "case {case:?}: expected the parent assistant's topic, got {topic:?}"
        );
        assert!(
            topics.insert(topic.clone()),
            "case {case:?}: topic {topic} was already another assistant's"
        );
    }
}

/// A caller-configured id is stored and reported exactly as given — a bare uuid stays
/// bare, a pinned version stays pinned — while the types it does not name keep the
/// assistant's version-qualified bindings.
#[tokio::test]
async fn a_configured_ai_agent_id_is_reported_exactly_as_given() {
    let emulator = Emulator::new();
    let assistant = emulator
        .create_assistant(REGION, json!({ "name": "configured-ids", "type": "AGENT" }))
        .await;
    let assistant_id = assistant
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", assistant.body));
    let inherited = assistant
        .object("/assistant/aiAgentConfiguration")
        .unwrap_or_else(|| panic!("no aiAgentConfiguration in {}", assistant.body));

    // One row per id form the modelled pattern admits: bare, `:<number>`, `:$LATEST`, and
    // a type the assistant has no default agent for.
    for (case, agent_type, configured_id) in [
        ("bare uuid", "MANUAL_SEARCH", CALLER_AGENT_BARE.to_string()),
        (
            "pinned version",
            "EMAIL_RESPONSE",
            CALLER_AGENT_VERSIONED.to_string(),
        ),
        (
            "latest qualifier",
            "ANSWER_RECOMMENDATION",
            format!("{CALLER_AGENT_BARE}:$LATEST"),
        ),
        (
            "type with no default agent",
            "ORCHESTRATION",
            CALLER_AGENT_OTHER.to_string(),
        ),
    ] {
        let created = emulator
            .create_session(
                REGION,
                &assistant_id,
                json!({
                    "name": format!("configured-{}", case.replace(' ', "-")),
                    "aiAgentConfiguration": {
                        agent_type: { "aiAgentId": configured_id }
                    },
                }),
            )
            .await;
        let session_id = created
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {}", created.body));
        let read = emulator
            .get_session(REGION, &assistant_id, &session_id)
            .await;

        for (which, reply) in [("create", &created), ("read", &read)] {
            let reported = reply
                .string(&format!("/session/aiAgentConfiguration/{agent_type}/aiAgentId"));
            assert_eq!(
                reported.as_deref(),
                Some(configured_id.as_str()),
                "case {case:?} ({which}): expected the configured id reported verbatim, \
                 got {reported:?}"
            );
            // Every other type still reports the assistant's own version-qualified
            // binding.
            for (inherited_type, binding) in &inherited {
                if inherited_type == agent_type {
                    continue;
                }
                let expected = binding.get("aiAgentId").and_then(Value::as_str);
                let got = reply.string(&format!(
                    "/session/aiAgentConfiguration/{inherited_type}/aiAgentId"
                ));
                assert_eq!(
                    got.as_deref(),
                    expected,
                    "case {case:?} ({which})/{inherited_type}: expected the assistant's \
                     binding {expected:?}, got {got:?}"
                );
            }
        }
    }
}
