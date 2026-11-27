// SPDX-License-Identifier: CC0-1.0

//! The AgentCore RUNTIME plane's view of an agent runtime: which runtime and which
//! endpoint a data-plane call addresses, and the sessions running in one.
//!
//! An agent runtime, and every endpoint on it, is created through the CONTROL plane
//! (`CreateAgentRuntime`, `CreateAgentRuntimeEndpoint`) — a different service — so this
//! plane creates neither and resolves what a request names against the component that
//! OWNS it, exactly as the browser plane resolves a browser ([`super::browser`]). A
//! request is addressed to an ENDPOINT of a runtime: the `qualifier` names it and a
//! request that names none addresses the endpoint the service manages
//! ([`DEFAULT_RUNTIME_QUALIFIER`]), so "the runtime does not exist" and "that alias does
//! not exist on it" are the same answer to the caller — which is why the sentence below
//! names both the qualifier and the agent.
//!
//! The SESSIONS running in a runtime are this plane's own: a session is established by an
//! invocation of the runtime and stopped by `StopRuntimeSession`, so the records live in
//! this service's store ([`RuntimeSessionRecord`]) and every read of one goes through the
//! lookups here.

use std::collections::BTreeMap;
use std::time::SystemTime;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control_plane;

use super::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState;

/// The endpoint alias a runtime call addresses when it names no `qualifier`: "If you don't
/// provide a value, the DEFAULT alias is used" (source: `smithy_query(query="operation",
/// operation="StopRuntimeSession")`, member `qualifier`) — which is the endpoint the
/// control plane establishes with every runtime, under the same name.
pub const DEFAULT_RUNTIME_QUALIFIER: &str = control_plane::state::DEFAULT_RUNTIME_ENDPOINT;

/// The `resource_type` field of `AgentRuntimeArn`,
/// `arn:aws:bedrock-agentcore:<region>:<account>:runtime/<agentRuntimeId>`, which is what
/// a data-plane request carries as its `agentRuntimeArn` label.
const RUNTIME_ARN_RESOURCE: &str = "runtime";

/// The most runtime sessions this emulator retains, over every runtime: the least recently
/// written record gives way once the store passes it.
///
/// An EMULATOR ceiling, not an AWS quota: a caller names the `runtimeSessionId` every
/// invocation inserts under, so without a bound a caller looping on invocations would grow
/// this process's memory without end.
pub const MAX_RUNTIME_SESSIONS: usize = 1_000;

/// How AWS words a runtime call that resolved no endpoint to address: neither the alias the
/// request asked for nor the agent it asked for it on could be resolved, so the sentence
/// names both — the qualifier the request carried (or the default alias it fell back to)
/// and the agent ARN it addressed.
///
/// One sentence for every operation of the runtime plane, so two of them cannot word the
/// same condition differently; which modelled error carries it is each operation's own
/// choice, because each names its own error variants.
pub fn runtime_endpoint_not_found(qualifier: &str, agent_runtime_arn: &str) -> String {
    format!(
        "No endpoint or agent found with qualifier '{qualifier}' for agent \
         '{agent_runtime_arn}'"
    )
}

/// How this plane words a call addressed to a runtime session that is not running: the
/// session the request named, on the agent it named it for.
///
/// A session exists only because an invocation of the runtime established it, so an id no
/// invocation ever used addresses nothing to stop.
pub fn runtime_session_not_found(runtime_session_id: &str, agent_runtime_arn: &str) -> String {
    format!(
        "No session found with id '{runtime_session_id}' for agent '{agent_runtime_arn}'"
    )
}

/// The sentence a call is refused with when the component that owns agent runtimes cannot
/// be reached, which is this emulator's fault and not the request's.
pub const RUNTIME_STORE_UNAVAILABLE: &str =
    "the shared engine does not carry the AgentCore control plane's state";

/// Why a call addressed to an agent runtime endpoint cannot be served, with the sentence it
/// is refused with. Which modelled error carries the sentence is the operation's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEndpointUnavailable {
    /// Neither the agent nor the alias the request named resolved: the modelled
    /// `ResourceNotFoundException`, worded by [`runtime_endpoint_not_found`].
    NotFound(String),
    /// The component that owns agent runtimes could not be reached: a modelled
    /// `InternalServerException`.
    StoreUnavailable(String),
}

/// The endpoint of an agent runtime a data-plane call addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRuntimeEndpoint {
    /// The `agentRuntimeId` the request's ARN named.
    pub agent_runtime_id: String,
    /// The endpoint the call is addressed to: the `qualifier` it named, else
    /// [`DEFAULT_RUNTIME_QUALIFIER`].
    pub endpoint_name: String,
}

/// Resolve the agent runtime endpoint a data-plane call addresses, read off the component
/// that OWNS agent runtimes.
///
/// `Ok` only for an endpoint that exists on a runtime this account still holds, or for the
/// default alias of the AgentCore Runtime environment the service manages for a HARNESS
/// (the runtime a harness invocation runs its session in): an ARN naming no runtime of this
/// account, a runtime whose deletion the clock has already carried out, and a qualifier
/// naming no endpoint on it are all the one answer [`runtime_endpoint_not_found`] words —
/// AWS tells a caller neither which of the two was missing nor whether the agent belongs to
/// somebody else.
///
/// Reading the owner's store materialises the runtime removals the clock has already made
/// due (`expire_deleted_agent_runtimes`), which were owed at this instant whether or not
/// anybody called; nothing else about the owner's state is changed, so a REPORTING
/// operation of this plane may resolve through this too.
pub fn resolve_runtime_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    agent_runtime_arn: &str,
    qualifier: Option<&str>,
    now: SystemTime,
) -> Result<ResolvedRuntimeEndpoint, RuntimeEndpointUnavailable> {
    let endpoint_name = qualifier
        .filter(|named| !named.is_empty())
        .unwrap_or(DEFAULT_RUNTIME_QUALIFIER)
        .to_string();
    let missing = || {
        RuntimeEndpointUnavailable::NotFound(runtime_endpoint_not_found(
            &endpoint_name,
            agent_runtime_arn,
        ))
    };
    // The `agentRuntimeArn` label addresses one runtime of one account: the id is the
    // resource field of an `AgentRuntimeArn`, and a value that is no such ARN names no
    // runtime at all.
    let Some(agent_runtime_id) =
        ::service_common::arn::resource_id_of_type(agent_runtime_arn, RUNTIME_ARN_RESOURCE)
            .filter(|id| !id.is_empty())
    else {
        return Err(missing());
    };
    let Some(control) = control_plane::state::service_state(engine) else {
        return Err(RuntimeEndpointUnavailable::StoreUnavailable(
            RUNTIME_STORE_UNAVAILABLE.to_string(),
        ));
    };
    let mut store = control.lock();
    store.expire_deleted_agent_runtimes(now);
    let Some(runtime) = store.agent_runtimes.get(agent_runtime_id) else {
        // A harness runs on an AgentCore Runtime environment the SERVICE manages for it
        // (`harness_runtime_environment`), which is a runtime nothing created with
        // `CreateAgentRuntime` and which therefore holds no endpoint record: it is
        // addressed under the default alias alone, and the harness that owns it is what
        // makes it exist.
        let managed = store
            .harnesses
            .values()
            .any(|harness| harness.environment.agent_runtime_arn == agent_runtime_arn);
        if managed && endpoint_name == DEFAULT_RUNTIME_QUALIFIER {
            return Ok(ResolvedRuntimeEndpoint {
                agent_runtime_id: agent_runtime_id.to_string(),
                endpoint_name,
            });
        }
        return Err(missing());
    };
    // A runtime whose ARN names another account's runtime is not this account's, however
    // well-formed the value is: the stored runtime's own ARN is the one that addresses it.
    if runtime.agent_runtime_arn != agent_runtime_arn {
        return Err(missing());
    }
    if !runtime.endpoints.contains_key(&endpoint_name) {
        return Err(missing());
    }
    Ok(ResolvedRuntimeEndpoint {
        agent_runtime_id: agent_runtime_id.to_string(),
        endpoint_name,
    })
}

/// One session running in an agent runtime, as an invocation of that runtime established
/// it and `StopRuntimeSession` stops it.
///
/// The session is keyed by the runtime it runs in and the `runtimeSessionId` the caller
/// invoked under, which is how every call that addresses one names it.
#[derive(Debug, Clone)]
pub struct RuntimeSessionRecord {
    /// The `agentRuntimeId` of the runtime this session runs in.
    pub agent_runtime_id: String,
    /// The endpoint alias the invocation that established the session addressed.
    pub endpoint_name: String,
    /// The `runtimeSessionId` the session is addressed by.
    pub runtime_session_id: String,
    /// When the invocation that established the session arrived.
    pub started_at: SystemTime,
    /// When the session was last written, which is what the eviction order is taken from.
    pub written_at: SystemTime,
    /// Set by the stop that ended the session: a stopped session runs nothing more, and a
    /// second stop of it has nothing left to stop.
    pub stopped_at: Option<SystemTime>,
}

impl RuntimeSessionRecord {
    /// A session established at `now` by an invocation of `endpoint_name` on
    /// `agent_runtime_id`.
    pub fn started(
        agent_runtime_id: &str,
        endpoint_name: &str,
        runtime_session_id: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            agent_runtime_id: agent_runtime_id.to_string(),
            endpoint_name: endpoint_name.to_string(),
            runtime_session_id: runtime_session_id.to_string(),
            started_at: now,
            written_at: now,
            stopped_at: None,
        }
    }

    /// Whether this session is still running, so there is something for a stop to stop.
    pub fn is_running(&self) -> bool {
        self.stopped_at.is_none()
    }
}

/// The key one runtime session is held under: the runtime it runs in and the id it was
/// invoked with, so two runtimes' sessions cannot collide on one id.
fn session_key(agent_runtime_id: &str, runtime_session_id: &str) -> String {
    format!("{agent_runtime_id}/{runtime_session_id}")
}

/// Mint the `runtimeSessionId` an invocation that named none runs under.
///
/// The id has to satisfy BOTH shapes it travels in: the one the answer reports it as
/// (`SessionId`, pattern `^[a-zA-Z0-9][a-zA-Z0-9-_]*$`, length 1..100) and the one a later
/// call addresses the same session by (`SessionType`, length 33..256), so it is exactly 33
/// characters — the instant the session was established in epoch milliseconds and the
/// engine-wide sequence, sixteen lowercase hex digits each, which keeps two sessions
/// established in one millisecond apart.
pub fn mint_runtime_session_id(seq: u64) -> String {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis());
    let millis = u64::try_from(millis).unwrap_or(0);
    format!("{millis:016x}-{seq:016x}")
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// Record that an invocation established `session`, evicting the least recently written
    /// record once the store passes [`MAX_RUNTIME_SESSIONS`].
    ///
    /// The ONE write path for a runtime session: an invocation of a runtime is what
    /// establishes one, so the operation that serves an invocation records it here and
    /// every reader of a session reads these records.
    pub fn insert_runtime_session(&mut self, session: RuntimeSessionRecord) {
        let key = session_key(&session.agent_runtime_id, &session.runtime_session_id);
        self.runtime_sessions.insert(key, session);
        while self.runtime_sessions.len() > MAX_RUNTIME_SESSIONS {
            // The least recently written record gives way, with the key breaking a tie so
            // eviction is deterministic.
            let Some(oldest) = self
                .runtime_sessions
                .iter()
                .min_by(|left, right| {
                    (left.1.written_at, left.0.as_str()).cmp(&(right.1.written_at, right.0.as_str()))
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.runtime_sessions.remove(&oldest);
        }
    }

    /// The session `runtime_session_id` names in `agent_runtime_id`, however it ended.
    pub fn runtime_session(
        &self,
        agent_runtime_id: &str,
        runtime_session_id: &str,
    ) -> Option<&RuntimeSessionRecord> {
        self.runtime_sessions
            .get(&session_key(agent_runtime_id, runtime_session_id))
    }

    /// The same session, to be written by the operation that ends it.
    pub fn runtime_session_mut(
        &mut self,
        agent_runtime_id: &str,
        runtime_session_id: &str,
    ) -> Option<&mut RuntimeSessionRecord> {
        self.runtime_sessions
            .get_mut(&session_key(agent_runtime_id, runtime_session_id))
    }
}

/// Every runtime session this plane holds, keyed by [`session_key`].
pub type RuntimeSessions = BTreeMap<String, RuntimeSessionRecord>;

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the refusal names the qualifier the request addressed and the agent it
    /// addressed it on, and nothing else of the request.
    #[test]
    fn the_refusal_names_the_qualifier_and_the_agent() {
        let agent = control_plane::state::runtime_arn("us-east-1", "demo-1a2b3c4d5e");
        // (case, the qualifier the request carried)
        for (case, qualifier) in [
            ("the default alias", DEFAULT_RUNTIME_QUALIFIER),
            ("an alias the caller named", "prod"),
        ] {
            let stated = runtime_endpoint_not_found(qualifier, &agent);
            let expected = format!(
                "No endpoint or agent found with qualifier '{qualifier}' for agent '{agent}'"
            );
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: a session is addressed by the runtime it runs in and the id it was invoked
    /// with, so one id in two runtimes is two sessions.
    #[test]
    fn a_session_is_addressed_by_its_runtime_and_its_id() {
        let mut store = ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::default();
        let now = SystemTime::now();
        store.insert_runtime_session(RuntimeSessionRecord::started(
            "first-1a2b3c4d5e",
            DEFAULT_RUNTIME_QUALIFIER,
            "session-id-of-at-least-thirty-three-characters",
            now,
        ));

        // (case, the runtime asked about, whether a session is held for it)
        let rows: [(&str, &str, bool); 2] = [
            ("the runtime the session runs in", "first-1a2b3c4d5e", true),
            ("another runtime", "second-9z8y7x6w5v", false),
        ];
        for (case, runtime, expected) in rows {
            let held = store
                .runtime_session(runtime, "session-id-of-at-least-thirty-three-characters")
                .is_some();
            assert_eq!(
                held, expected,
                "case {case:?}: expected held={expected}, got {held}"
            );
        }
    }
}
