// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Bedrock Agent Runtime **agent memory** slice —
//! `GetAgentMemory` and `DeleteAgentMemory` — and the harness every suite that needs a
//! PRINCIPAL drives the service through.
//!
//! THE RULES UNDER TEST:
//!
//! * an agent alias is a resource of the Bedrock Agent build-time plane, so a memory
//!   operation addressed at an alias that plane holds nothing for answers the modelled
//!   `ResourceNotFoundException` (404), while an alias it does hold is served — with the
//!   memory the store really holds for it, which for an alias that has held no session is
//!   an EMPTY memory and not a not-found;
//! * the authorization decision comes FIRST: a caller whose identity-based policies do not
//!   allow the operation's `bedrock:` action is refused with `AccessDeniedException` (403)
//!   whether or not the alias exists, which is what lets a client tell a permissions
//!   problem from a missing agent. The positive control is the same call made by a
//!   permitted caller: it reaches the service and answers the resource's own 404.
//!
//! The tables below span what those rules generalise over: whether the agent exists,
//! whether the alias is the draft one every agent carries, which operation is called,
//! where the caller's grant comes from (nothing, an unrelated grant, an explicit deny, the
//! action itself) and what a delete names (a memory, a session, both, neither).
//!
//! Nothing is seeded into another service's store by hand except the memory itself: the
//! agents are created through the build-time plane's own `CreateAgent`, the roles through
//! IAM's `CreateRole`/`PutRolePolicy` and the session credential through STS's `AssumeRole`
//! — the same path a client takes. The memory is seeded in this service's own store so a
//! case here turns on the memory alone; the surface that really records a session into an
//! agent's memory is `InvokeAgent`, whose write
//! `tests/all/agent_and_flow_invocations.rs` drives end to end.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state as runtime_state;
use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::agent_memory;

/// The region every request below signs for.
pub(crate) const REGION: &str = "us-west-2";

/// An access key id that names no STS session: STS resolves it to the ambient identity —
/// the account's own published credential — which every operation of this service allows.
pub(crate) const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The wire code and status of the two refusals these rules turn on.
pub(crate) const ACCESS_DENIED: (&str, u16) = ("AccessDeniedException", 403);
pub(crate) const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

/// A well-formed `agentId` the plane never minted (`^[0-9a-zA-Z]+$`, at most 10).
const UNKNOWN_AGENT: &str = "ABCDEFGHIJ";

/// A well-formed `agentAliasId` no agent here carries.
const UNKNOWN_ALIAS: &str = "ZYXWVUTSRQ";

/// One HTTP answer, read the way a client reads it.
pub(crate) struct Reply {
    pub status: u16,
    /// The response's `content-type`, which is how a client tells an event-stream body
    /// from a JSON one.
    pub content_type: String,
    /// Every response header, so a suite can read a member the model binds to one.
    pub headers: ::http::HeaderMap,
    pub body: Bytes,
}

impl Reply {
    /// The body as JSON, or `Value::Null` for an empty one.
    pub fn json(&self, case: &str) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({error})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code restJson1 carries in `__type`, or `""` on a success body.
    pub fn error_code(&self, case: &str) -> String {
        self.json(case)
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error body's `message`.
    pub fn message(&self, case: &str) -> String {
        self.json(case)
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// One response header, or `""` when the response carries none of that name.
    pub fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator these suites drive: one shared engine carrying this service, the Bedrock
/// Agent build-time plane that owns agents, knowledge bases and documents, the Bedrock
/// Runtime data plane that owns the model catalogue a prompt optimization resolves
/// against, S3, which owns the objects a document was ingested from, and the IAM and STS
/// services the principals are made with.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suites sign nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // The build-time plane's agent surface is closed to an account with no prior
        // usage of it, so these suites drive the EXISTING-customer path: the account's
        // prior usage is recorded in that plane's own store, the producer its API does
        // not model, before any `CreateAgent` below.
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::service_state(
            &*engine,
        )
        .expect("the bedrock-agent store is reachable through the shared engine")
        .lock()
        .agents_classic_usage
        .record(engine::EMULATOR_ACCOUNT_ID);
        router.register_service(
            service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One Bedrock call, with `access_key_id` in the SigV4 credential scope.
    pub(crate) async fn bedrock(
        &self,
        access_key_id: &str,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = match body {
            Some(value) => {
                Bytes::from(serde_json::to_vec(&value).expect("the test body serializes"))
            }
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path_and_query)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        Reply {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            headers: response.headers().clone(),
            body: response.into_body(),
        }
    }

    /// One awsQuery call to IAM or STS: `signing_name` picks the service, `version` its
    /// API version.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encoded(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// Create an IAM role, give it `policy` as an inline policy when one is supplied,
    /// assume it through STS, and answer the access key id of the session — the credential
    /// a caller then signs its Bedrock requests with.
    pub(crate) async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let account = engine::EMULATOR_ACCOUNT_ID;
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}"
        );
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "setup: putting the inline policy of {role_name:?} must succeed, got \
                 {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!("arn:aws:iam::{account}:role/{role_name}"),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("setup: AssumeRole must answer an AccessKeyId, got {body}"))
    }

    /// Create an agent through the build-time plane's own `CreateAgent` and answer its
    /// minted `agentId`.
    pub(crate) async fn create_agent(&self, name: &str) -> String {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "PUT",
                "/agents/",
                Some(json!({ "agentName": name })),
            )
            .await;
        assert_eq!(
            reply.status, 202,
            "setup: CreateAgent must answer 202, got {} {:?}",
            reply.status,
            reply.json(name)
        );
        reply.json(name)["agent"]["agentId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: CreateAgent answered no agentId: {:?}", reply.json(name)))
            .to_string()
    }

    /// `GetAgentMemory` over one alias, with the query the model binds.
    async fn get_memory(
        &self,
        key: &str,
        agent_id: &str,
        alias_id: &str,
        memory_id: &str,
        query: &str,
    ) -> Reply {
        self.bedrock(
            key,
            "GET",
            &format!(
                "/agents/{agent_id}/agentAliases/{alias_id}/memories\
                 ?memoryType=SESSION_SUMMARY&memoryId={memory_id}{query}"
            ),
            None,
        )
        .await
    }

    /// `DeleteAgentMemory` over one alias, with the query the case asks for.
    async fn delete_memory(&self, key: &str, agent_id: &str, alias_id: &str, query: &str) -> Reply {
        self.bedrock(
            key,
            "DELETE",
            &format!("/agents/{agent_id}/agentAliases/{alias_id}/memories{query}"),
            None,
        )
        .await
    }

    /// Record `sessions` into the memory `memory_id` of one alias, which is the state the
    /// agent invocation surface leaves behind once it summarises a session.
    fn seed_memory(&self, agent_id: &str, alias_id: &str, memory_id: &str, sessions: &[&str]) {
        let state = runtime_state::service_state(&*self.engine)
            .expect("the Bedrock Agent Runtime store is on this engine");
        let summaries = sessions
            .iter()
            .enumerate()
            .map(|(index, session_id)| agent_memory::SessionSummaryRecord {
                memory_id: memory_id.to_string(),
                session_id: (*session_id).to_string(),
                session_start_time: ::aws_smithy_types::DateTime::from_secs(
                    1_760_000_000 + index as i64 * 60,
                ),
                session_expiry_time: ::aws_smithy_types::DateTime::from_secs(
                    1_760_086_400 + index as i64 * 60,
                ),
                summary_text: format!("the session {session_id} was summarised"),
            })
            .collect();
        state.lock().agent_memories.insert(
            agent_memory::memory_key(agent_id, alias_id, memory_id),
            summaries,
        );
    }

    /// The session ids the memory `memory_id` of one alias still holds, read through
    /// `GetAgentMemory` itself.
    async fn memory_sessions(&self, case: &str, agent_id: &str, memory_id: &str) -> Vec<String> {
        let reply = self
            .get_memory(
                ACCOUNT_KEY,
                agent_id,
                agent_memory::DRAFT_AGENT_ALIAS_ID,
                memory_id,
                "",
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: GetAgentMemory must answer 200, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        reply.json(case)["memoryContents"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: GetAgentMemory answered no memoryContents: {:?}",
                    reply.json(case)
                )
            })
            .iter()
            .map(|entry| {
                entry["sessionSummary"]["sessionId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a memory entry carried no sessionId: {entry:?}")
                    })
                    .to_string()
            })
            .collect()
    }
}

/// One form field, percent-encoded the way an awsQuery body carries it.
fn form_encoded(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>` element of an awsQuery XML response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// An identity-based policy allowing `actions` on every resource.
pub(crate) fn allow(actions: &[&str]) -> String {
    policy("Allow", actions)
}

/// An identity-based policy DENYING `actions` on every resource.
pub(crate) fn deny(actions: &[&str]) -> String {
    policy("Deny", actions)
}

fn policy(effect: &str, actions: &[&str]) -> String {
    let actions = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Action\":[{actions}],\"Resource\":\"*\"}}]}}"
    )
}

/// RULE: a memory operation is served for an alias the Bedrock Agent plane holds, and
/// refused with `ResourceNotFoundException` for one it does not — whichever half of the
/// alias (the agent, or the alias on it) is the one it does not hold.
#[tokio::test]
async fn a_memory_operation_resolves_its_alias_in_the_plane_that_owns_agents() {
    /// Which alias a row addresses, and what the two operations answer for it.
    struct Row {
        case: &'static str,
        /// Whether the agent is created first.
        agent_exists: bool,
        alias: &'static str,
        /// The status `GetAgentMemory` answers, and the one `DeleteAgentMemory` does.
        get_status: u16,
        delete_status: u16,
    }

    let rows = [
        Row {
            case: "an agent that was never created",
            agent_exists: false,
            alias: agent_memory::DRAFT_AGENT_ALIAS_ID,
            get_status: NOT_FOUND.1,
            delete_status: NOT_FOUND.1,
        },
        Row {
            case: "an agent that exists, under an alias that was never created",
            agent_exists: true,
            alias: UNKNOWN_ALIAS,
            get_status: NOT_FOUND.1,
            delete_status: NOT_FOUND.1,
        },
        Row {
            case: "an agent that exists, under the draft alias every agent carries",
            agent_exists: true,
            alias: agent_memory::DRAFT_AGENT_ALIAS_ID,
            // 200 with an empty memory: the alias holds no session, which is not a
            // missing resource. `DeleteAgentMemory` is `@idempotent` and answers 202.
            get_status: 200,
            delete_status: 202,
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let agent_id = if row.agent_exists {
            emulator.create_agent("memory-rule-agent").await
        } else {
            UNKNOWN_AGENT.to_string()
        };

        let read = emulator
            .get_memory(ACCOUNT_KEY, &agent_id, row.alias, "memory-1", "")
            .await;
        assert_eq!(
            read.status,
            row.get_status,
            "case {:?}: expected GetAgentMemory -> {}, got {} {:?}",
            row.case,
            row.get_status,
            read.status,
            read.json(row.case)
        );
        if row.get_status == NOT_FOUND.1 {
            assert_eq!(
                read.error_code(row.case),
                NOT_FOUND.0,
                "case {:?}: expected {}, got {:?}",
                row.case,
                NOT_FOUND.0,
                read.json(row.case)
            );
            assert!(
                read.message(row.case) == agent_memory::AGENT_OR_ALIAS_NOT_FOUND,
                "case {:?}: expected the service's sentence for this error class, got {:?}",
                row.case,
                read.message(row.case)
            );
        } else {
            assert_eq!(
                read.json(row.case)["memoryContents"].as_array().map(Vec::len),
                Some(0),
                "case {:?}: an alias that has held no session has an empty memory, got {:?}",
                row.case,
                read.json(row.case)
            );
            assert!(
                read.json(row.case).get("nextToken").is_none(),
                "case {:?}: a memory that fits one page carries no nextToken, got {:?}",
                row.case,
                read.json(row.case)
            );
        }

        let deleted = emulator
            .delete_memory(ACCOUNT_KEY, &agent_id, row.alias, "")
            .await;
        assert_eq!(
            deleted.status,
            row.delete_status,
            "case {:?}: expected DeleteAgentMemory -> {}, got {} {:?}",
            row.case,
            row.delete_status,
            deleted.status,
            deleted.json(row.case)
        );
    }
}

/// RULE: `GetAgentMemory` reports the sessions the store holds for the alias and memory id
/// the request names, in the order they were recorded, paged on `maxItems`; a `nextToken`
/// resumes exactly after the page it was issued with.
#[tokio::test]
async fn an_agent_memory_is_reported_and_paged_as_the_store_holds_it() {
    let emulator = Emulator::new();
    let agent_id = emulator.create_agent("memory-paging-agent").await;
    let sessions = ["session-1", "session-2", "session-3"];
    emulator.seed_memory(
        &agent_id,
        agent_memory::DRAFT_AGENT_ALIAS_ID,
        "memory-1",
        &sessions,
    );
    // A second memory of the same alias, which no read of the first may report.
    emulator.seed_memory(
        &agent_id,
        agent_memory::DRAFT_AGENT_ALIAS_ID,
        "memory-2",
        &["session-9"],
    );

    assert_eq!(
        emulator
            .memory_sessions("the whole memory", &agent_id, "memory-1")
            .await,
        sessions,
        "the whole memory must be reported in the order it was recorded"
    );
    assert_eq!(
        emulator
            .memory_sessions("another memory of the same alias", &agent_id, "memory-2")
            .await,
        ["session-9"],
        "one memory of an alias must not report another's sessions"
    );

    // Paged: one page per `maxItems` entries, each resumed with the token the one before
    // it handed out, and no token on the last page.
    for page_size in [1_usize, 2, 3] {
        let case = format!("maxItems={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=sessions.len() {
            let query = match &token {
                None => format!("&maxItems={page_size}"),
                Some(token) => format!("&maxItems={page_size}&nextToken={token}"),
            };
            let reply = emulator
                .get_memory(
                    ACCOUNT_KEY,
                    &agent_id,
                    agent_memory::DRAFT_AGENT_ALIAS_ID,
                    "memory-1",
                    &query,
                )
                .await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected 200, got {} {:?}",
                reply.status,
                reply.json(&case)
            );
            let page = reply.json(&case);
            let entries = page["memoryContents"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no memoryContents in {page:?}"))
                .clone();
            assert!(
                entries.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {}",
                entries.len()
            );
            seen.extend(entries.iter().map(|entry| {
                entry["sessionSummary"]["sessionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }));
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the paging must terminate, got a token after {} entries",
            seen.len()
        );
        assert_eq!(
            seen, sessions,
            "case {case:?}: paging must report every session exactly once, in order"
        );
    }
}

/// RULE: `DeleteAgentMemory` removes exactly what the request names — a memory, a session,
/// one session of one memory, or every memory of the alias — and nothing else; the state it
/// leaves is what `GetAgentMemory` then reports.
#[tokio::test]
async fn a_delete_removes_exactly_the_memory_the_request_names() {
    /// One delete, and the sessions each of the alias's two memories still holds after it.
    struct Row {
        case: &'static str,
        /// The query the delete carries.
        query: &'static str,
        first_memory_left: &'static [&'static str],
        second_memory_left: &'static [&'static str],
    }

    let rows = [
        Row {
            case: "memoryId alone empties that one memory",
            query: "?memoryId=memory-1",
            first_memory_left: &[],
            second_memory_left: &["session-1", "session-9"],
        },
        Row {
            case: "sessionId alone removes that session from every memory of the alias",
            query: "?sessionId=session-1",
            first_memory_left: &["session-2"],
            second_memory_left: &["session-9"],
        },
        Row {
            case: "memoryId and sessionId remove that one session from that one memory",
            query: "?memoryId=memory-1&sessionId=session-1",
            first_memory_left: &["session-2"],
            second_memory_left: &["session-1", "session-9"],
        },
        Row {
            case: "neither empties every memory of the alias",
            query: "",
            first_memory_left: &[],
            second_memory_left: &[],
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let agent_id = emulator.create_agent("memory-delete-agent").await;
        let alias = agent_memory::DRAFT_AGENT_ALIAS_ID;
        emulator.seed_memory(&agent_id, alias, "memory-1", &["session-1", "session-2"]);
        emulator.seed_memory(&agent_id, alias, "memory-2", &["session-1", "session-9"]);

        let deleted = emulator
            .delete_memory(ACCOUNT_KEY, &agent_id, alias, row.query)
            .await;
        assert_eq!(
            deleted.status,
            202,
            "case {:?}: expected DeleteAgentMemory -> 202, got {} {:?}",
            row.case,
            deleted.status,
            deleted.json(row.case)
        );
        assert_eq!(
            emulator.memory_sessions(row.case, &agent_id, "memory-1").await,
            row.first_memory_left,
            "case {:?}: memory-1 holds the wrong sessions after the delete",
            row.case
        );
        assert_eq!(
            emulator.memory_sessions(row.case, &agent_id, "memory-2").await,
            row.second_memory_left,
            "case {:?}: memory-2 holds the wrong sessions after the delete",
            row.case
        );

        // `@idempotent`: the same delete again is the same accepted answer, not a 404.
        let again = emulator
            .delete_memory(ACCOUNT_KEY, &agent_id, alias, row.query)
            .await;
        assert_eq!(
            again.status,
            202,
            "case {:?}: a repeated delete must answer 202, got {} {:?}",
            row.case,
            again.status,
            again.json(row.case)
        );
    }
}

/// RULE: a request this service cannot serve as asked is refused with the modelled
/// `ValidationException` (400) naming the member that was wrong, rather than answered with
/// a memory it did not ask for.
#[tokio::test]
async fn a_memory_read_refuses_a_member_it_cannot_serve() {
    let emulator = Emulator::new();
    let agent_id = emulator.create_agent("memory-refusal-agent").await;
    let alias = agent_memory::DRAFT_AGENT_ALIAS_ID;
    emulator.seed_memory(&agent_id, alias, "memory-1", &["session-1", "session-2"]);
    let token = {
        let reply = emulator
            .get_memory(ACCOUNT_KEY, &agent_id, alias, "memory-1", "&maxItems=1")
            .await;
        reply.json("a token of memory-1")["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "setup: a page of 1 over 2 sessions must carry a token, got {:?}",
                    reply.json("a token of memory-1")
                )
            })
            .to_string()
    };

    /// One request that cannot be served as asked, and the member the refusal must name.
    struct Row {
        case: &'static str,
        /// The query the read carries, with `{token}` standing for a token of `memory-1`.
        query: &'static str,
        memory_id: &'static str,
        names: &'static str,
    }

    let rows = [
        Row {
            case: "a token issued for another memory of the same alias",
            query: "&nextToken={token}",
            memory_id: "memory-2",
            names: "nextToken",
        },
        Row {
            case: "a token this service never issued",
            query: "&nextToken=not-a-token-this-service-issued",
            memory_id: "memory-1",
            names: "nextToken",
        },
    ];

    for row in rows {
        let query = row.query.replace("{token}", &token);
        let reply = emulator
            .get_memory(ACCOUNT_KEY, &agent_id, alias, row.memory_id, &query)
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected 400, got {} {:?}",
            row.case,
            reply.status,
            reply.json(row.case)
        );
        assert_eq!(
            reply.error_code(row.case),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            reply.json(row.case)
        );
        assert!(
            reply.message(row.case).contains(row.names),
            "case {:?}: the refusal must name {:?}, got {:?}",
            row.case,
            row.names,
            reply.message(row.case)
        );
    }

    // A `memoryType` outside the one value the model declares is refused by the boundary
    // before the handler runs, and is still this service's `ValidationException`.
    let unsupported = emulator
        .bedrock(
            ACCOUNT_KEY,
            "GET",
            &format!(
                "/agents/{agent_id}/agentAliases/{alias}/memories\
                 ?memoryType=SESSION_TRANSCRIPT&memoryId=memory-1"
            ),
            None,
        )
        .await;
    assert_eq!(
        unsupported.status, 400,
        "case \"an unmodelled memoryType\": expected 400, got {} {:?}",
        unsupported.status,
        unsupported.json("an unmodelled memoryType")
    );
}

/// RULE: the authorization decision is taken before the alias is looked up. A caller whose
/// identity-based policies do not allow the operation's action is refused with
/// `AccessDeniedException` (403) on a call a permitted caller gets the resource's own 404
/// for — which is what proves the first answer was authorization and not a blanket
/// refusal.
#[tokio::test]
async fn a_caller_without_the_action_is_refused_before_the_alias_is_resolved() {
    /// One principal, and what the two memory operations answer it.
    struct Row {
        case: &'static str,
        /// The inline policy the assumed role carries, if any.
        policy: Option<String>,
        expected: (&'static str, u16),
    }

    let rows = [
        Row {
            case: "a principal the account grants nothing",
            policy: None,
            expected: ACCESS_DENIED,
        },
        Row {
            case: "a principal granted an unrelated action",
            policy: Some(allow(&["bedrock:ListSessions"])),
            expected: ACCESS_DENIED,
        },
        Row {
            case: "a principal granted the action and denied it outright",
            policy: Some(deny(&["bedrock:GetAgentMemory", "bedrock:DeleteAgentMemory"])),
            expected: ACCESS_DENIED,
        },
        Row {
            case: "a principal granted exactly the two memory actions",
            policy: Some(allow(&["bedrock:GetAgentMemory", "bedrock:DeleteAgentMemory"])),
            // Allowed through to the resource, which is one no agent exists for.
            expected: NOT_FOUND,
        },
        Row {
            case: "a principal granted every action of the service",
            policy: Some(allow(&["bedrock:*"])),
            expected: NOT_FOUND,
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.into_iter().enumerate() {
        let key = emulator
            .assumed_role_key(&format!("memory-caller-{index}"), row.policy.as_deref())
            .await;
        for (operation, reply) in [
            (
                "GetAgentMemory",
                emulator
                    .get_memory(
                        &key,
                        UNKNOWN_AGENT,
                        agent_memory::DRAFT_AGENT_ALIAS_ID,
                        "memory-1",
                        "",
                    )
                    .await,
            ),
            (
                "DeleteAgentMemory",
                emulator
                    .delete_memory(
                        &key,
                        UNKNOWN_AGENT,
                        agent_memory::DRAFT_AGENT_ALIAS_ID,
                        "",
                    )
                    .await,
            ),
        ] {
            let case = format!("{}, calling {operation}", row.case);
            assert_eq!(
                (reply.error_code(&case).as_str(), reply.status),
                row.expected,
                "case {case:?}: expected {:?}, got {} {:?}",
                row.expected,
                reply.status,
                reply.json(&case)
            );
            if row.expected == ACCESS_DENIED {
                assert!(
                    reply
                        .message(&case)
                        .contains(&format!("bedrock:{operation}")),
                    "case {case:?}: the refusal must name the action it was taken on, got {:?}",
                    reply.message(&case)
                );
            }
        }
    }
}
