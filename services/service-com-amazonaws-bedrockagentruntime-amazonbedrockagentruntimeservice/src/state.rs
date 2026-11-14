// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice service.
//!
//! The persistent resource this service owns is the **session**: a session holds
//! invocations, and an invocation holds invocation steps. Every session-scoped
//! operation (`GetSession`, `UpdateSession`, `EndSession`, `DeleteSession`,
//! `CreateInvocation`, `PutInvocationStep`, `GetInvocationStep`,
//! `ListInvocations`, `ListInvocationSteps`) resolves its `sessionIdentifier`
//! through the ONE helper [`session_id_from_identifier`], so they all agree on what
//! a bare UUID and a session ARN mean.
//!
//! The refusals that lookup and the session state machine produce are worded here too —
//! [`session_not_found`], [`taggable_resource_not_found`], [`session_in_terminal_state`]
//! and [`active_session_not_deletable`] — for the same reason: the service states one
//! sentence per rule, not one per operation, so a client can match the rule. How a refusal
//! the FRAMEWORK raises before a handler runs is worded (a value outside a `@pattern`, say)
//! belongs to `crate::service`'s `override_refusal` instead.
//!
//! Beside the store, this module carries the one AUTHORIZATION decision every
//! session-scoped operation makes before it answers ([`authorize`]): who the caller is,
//! and whether that caller's identity-based permissions cover `bedrock:<Operation>` on
//! the session. It lives here, not in a handler, because all of them must decide it the
//! same way.
//!
//! The service's own store is the session; the KNOWLEDGE BASE a retrieval reads is
//! owned by the Bedrock Agent build-time plane and the S3 object store, so what a
//! retrieval may report from them lives in two submodules of its own rather than in a
//! handler:
//!
//! * [`knowledge_base_retrieval`] — which chunks of which ingested source a knowledge
//!   base holds, and where each one came from;
//! * [`retrieval_filter`] — which of those chunks a request's `filter` admits;
//! * [`generated_answer`] — the response a knowledge-base query is answered with, composed
//!   of the passages retrieved for it, and the model it is generated against;
//! * [`knowledge_base_session`] — the conversational session such a query is served in,
//!   which IS this service's own store: the turns one caller has taken against a knowledge
//!   base, under a `sessionId` only this service mints.
//!
//! The MEMORY of an agent alias is this plane's own ([`agent_memory`]): the summaries of
//! the sessions an alias held live in the store below, while the agent and the alias they
//! belong to are the build-time plane's, so that submodule is where both memory
//! operations resolve an alias and word the refusal for one nothing holds.

/// See the module docs: what an agent alias's memory holds, and how the two memory
/// operations resolve the alias they address.
pub mod agent_memory;
/// See the module docs: the agent alias an invocation reaches, what its response stream
/// says, and the summary it leaves in that alias's memory.
pub mod agent_invocation;
/// See the module docs: where the content of one ingested document lives.
pub mod document_content;
/// See the module docs: the flow an alias routes to, and the path a request's input
/// travels through that flow's own definition.
pub mod flow_invocation;
/// See the module docs: the response a knowledge-base query is answered with, and the
/// model it is generated against.
pub mod generated_answer;
/// See the module docs: what a knowledge-base retrieval may report.
pub mod knowledge_base_retrieval;
/// See the module docs: the conversational session a knowledge-base query is served in.
pub mod knowledge_base_session;
/// See the module docs: which chunks a retrieval's `filter` admits.
pub mod retrieval_filter;
/// See the module docs: the model a prompt optimization is asked to optimize for.
pub mod target_model;

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::aws_smithy_types::DateTime;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_state;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::ops::types::{
    AccessDeniedException, ConflictException, InvocationStepPayload, ResourceNotFoundException,
};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region every generated emulator defaults to (see
/// `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `bedrock` ARN namespace this service's resources live in
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`), which is
/// NOT the `bedrock-agent-runtime` endpoint prefix.
pub const ARN_NAMESPACE: &str = "bedrock";

/// Resource guards, NOT AWS quotas.
///
/// The session store lives in this process's memory, has no TTL and is released
/// only by `DeleteSession`, so an unbounded number of `CreateSession` /
/// `CreateInvocation` / `PutInvocationStep` calls would grow it until the emulator
/// is starved. These are the declared maxima that stop that: past one of them the
/// writing operation answers its modelled `ServiceQuotaExceededException` (HTTP 400,
/// "The number of requests exceeds the service quota") instead of allocating. They
/// are the emulator's own limits, deliberately not presented as the real service's
/// published quotas, and each is far above any plausible test or demo workload.
pub const MAX_SESSIONS: usize = 1_000;

/// Resource guard: the most invocations one session holds. See [`MAX_SESSIONS`].
pub const MAX_INVOCATIONS_PER_SESSION: usize = 1_000;

/// Resource guard: the most steps one invocation holds. See [`MAX_SESSIONS`].
pub const MAX_STEPS_PER_INVOCATION: usize = 1_000;

/// Resource guard: the most session summaries one agent memory holds. See
/// [`MAX_SESSIONS`]; an agent invocation that would exceed it answers its modelled
/// `ServiceQuotaExceededException` instead of growing the memory.
pub const MAX_SESSION_SUMMARIES_PER_MEMORY: usize = 1_000;

/// Resource guard: the most retrievers ONE agentic retrieval reads. See [`MAX_SESSIONS`].
///
/// The modelled `AgenticRetrievers` carries a length FLOOR and no ceiling, and each
/// retriever of a request is a separate pass over a vector index — so without this the
/// request itself would choose how many times that work is done. Past it the retrieval
/// answers its modelled `ServiceQuotaExceededException` instead of running them.
pub const MAX_RETRIEVERS_PER_RETRIEVAL: usize = 25;

/// Resource guard: the most tags one session holds, taken from the model's own bound on
/// a single tagging request (`smithy_query(query="operation",operation="TagResource")` ->
/// `tags` is `TagsMap`, `length {min: 1, max: 200}`). `TagResource` merges into a stored
/// map, so without a ceiling repeated calls with fresh keys would grow one session's map
/// without bound; past it the write answers its modelled `ServiceQuotaExceededException`.
/// An emulator limit, not an AWS published quota.
pub const MAX_TAGS_PER_SESSION: usize = 200;

/// The `maxResults` bounds this service's session listings declare
/// (`smithy_query(query="operation",operation="ListInvocationSteps,ListInvocations")`
/// -> `maxResults` has `range {min: 1, max: 1000}` and `default: 10` in both), named
/// here so the model-derived page bounds have one home.
pub const MIN_LIST_PAGE: i32 = 1;

/// See [`MIN_LIST_PAGE`].
pub const MAX_LIST_PAGE: i32 = 1_000;

/// See [`MIN_LIST_PAGE`].
pub const DEFAULT_LIST_PAGE: i32 = 10;

/// A session's lifecycle status: the three values the model's `SessionStatus` enum
/// declares (`smithy_query(query="shape",shape="SessionStatus")` -> ACTIVE,
/// EXPIRED, ENDED).
///
/// `Expired` is modelled and understood here, but no code path mints it: the model
/// and the service docs state no session lifetime, so there is no instant at which
/// the clock owes that transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Expired,
    Ended,
}

impl SessionStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Expired => "EXPIRED",
            Self::Ended => "ENDED",
        }
    }
}

/// One stored invocation step: the identity triple, the caller's step time, and the
/// `InvocationStepPayload` union stored WHOLE so `GetInvocationStep` hands back
/// exactly what `PutInvocationStep` was given.
#[derive(Debug)]
pub struct InvocationStepRecord {
    pub invocation_step_id: String,
    pub invocation_id: String,
    pub session_id: String,
    pub invocation_step_time: DateTime,
    pub payload: InvocationStepPayload,
    /// Position in this service's global store order. Listings sort on it, so a
    /// page boundary is stable even though the maps are keyed by UUID.
    pub order: u64,
}

/// One stored invocation, holding the steps written under it.
#[derive(Debug)]
pub struct Invocation {
    pub invocation_id: String,
    pub session_id: String,
    pub description: Option<String>,
    pub created_at: DateTime,
    pub steps: BTreeMap<String, InvocationStepRecord>,
    /// Store order, as for [`InvocationStepRecord::order`].
    pub order: u64,
}

/// One stored session.
#[derive(Debug)]
pub struct Session {
    pub session_id: String,
    pub session_arn: String,
    pub status: SessionStatus,
    pub created_at: DateTime,
    pub last_updated_at: DateTime,
    /// Exactly what the writer supplied: `None` when the request carried no
    /// `sessionMetadata` member at all, `Some(map)` (possibly empty) when it did, so
    /// `GetSession` can report an absent member and a stored empty map as the two
    /// different answers they are.
    pub session_metadata: Option<BTreeMap<String, String>>,
    pub encryption_key_arn: Option<String>,
    /// The session's tags: what `CreateSession` was given, merged by `TagResource` and
    /// pruned by `UntagResource`, and what `ListTagsForResource` reports. A session ARN
    /// is the ONLY resource this service's `TaggableResourcesArn` admits
    /// (`smithy_query(query="operation",operation="TagResource")` -> `resourceArn`), so
    /// this map is the whole tag plane.
    pub tags: BTreeMap<String, String>,
    pub invocations: BTreeMap<String, Invocation>,
    /// Store order, as for [`InvocationStepRecord::order`].
    pub order: u64,
}

/// Per-service state for com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState {
    /// Every live session, keyed by its bare `sessionId` UUID.
    pub sessions: BTreeMap<String, Session>,
    /// The memory of every agent alias this plane has recorded a session into, keyed by
    /// the alias and the memory id ([`agent_memory::MemoryKey`]). `GetAgentMemory` reports
    /// what it holds and `DeleteAgentMemory` removes from it; a key it holds nothing for
    /// is an alias that has held no session, which is an empty memory rather than a
    /// missing resource. See [`agent_memory`] for what writes a summary.
    pub agent_memories: BTreeMap<agent_memory::MemoryKey, Vec<agent_memory::SessionSummaryRecord>>,
    /// Every conversational session a `RetrieveAndGenerate` has opened, keyed by the
    /// `sessionId` this service minted for it. See [`knowledge_base_session`] for what a
    /// session holds and why a caller can only continue one it was handed.
    pub knowledge_base_sessions: BTreeMap<String, knowledge_base_session::KnowledgeBaseSession>,
    /// Monotonic counter handing out [`InvocationStepRecord::order`] values.
    pub order_counter: u64,
    /// The secret the addresses this service hands out for the content of an ingested
    /// document are authenticated under ([`document_content`]).
    ///
    /// Drawn from the system's randomness when this store is created and never revealed:
    /// an address `GetDocumentContent` minted carries a tag only this store could have
    /// produced, so the content plane serves a document only to the address the read that
    /// authorized it handed out. Never reported, never logged.
    pub document_address_key: ::service_common::page_tokens::PageTokenKey,
}

/// The next store-order position, taken from
/// [`ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState::order_counter`]
/// directly. Never reused, so listing order is insertion order for the lifetime of
/// the process.
///
/// It takes the counter field rather than the whole state, because a handler writing
/// through a `&mut` borrow of one session cannot hold a second borrow of the struct;
/// one function serves every caller either way.
pub fn next_order(counter: &mut u64) -> u64 {
    *counter += 1;
    *counter
}

/// Resolve a `sessionIdentifier` to the bare `sessionId` it names.
///
/// The model's `SessionIdentifier` accepts EITHER a session ARN
/// (`arn:…:bedrock:…:session/<uuid>`) or the bare `<uuid>`
/// (`smithy_query(query="operation",operation="GetSession")` -> the member's
/// pattern), so every session-scoped operation routes its label through here and
/// they cannot disagree.
///
/// Only an ARN this emulator could itself have minted names a session here: the
/// `bedrock` ARN namespace and the canonical emulator account. An ARN for another
/// service or another account is returned unchanged, so the lookup misses and the
/// caller answers `ResourceNotFoundException` — the real service would not resolve
/// another account's resource either. The region segment is NOT checked: one
/// emulator process serves every region from one store, and the region in a minted
/// ARN is whichever one the creating caller signed for.
pub fn session_id_from_identifier(identifier: &str) -> &str {
    let Some((prefix, session_id)) = identifier.rsplit_once(":session/") else {
        // Not an ARN at all: the bare session id.
        return identifier;
    };
    let fields: Vec<&str> = prefix.split(':').collect();
    match fields.as_slice() {
        ["arn", _partition, namespace, _region, account]
            if *namespace == ARN_NAMESPACE && *account == ::engine::EMULATOR_ACCOUNT_ID =>
        {
            session_id
        }
        _ => identifier,
    }
}

/// The modelled `ResourceNotFoundException` (HTTP 404) every session-scoped operation
/// answers when its `sessionIdentifier` names no stored session.
///
/// ONE constructor for all of them: the refusal is worded from the REQUEST — the fixed
/// template `Session with ID <sessionIdentifier> not found`, interpolating the identifier
/// the caller submitted and nothing else — and the service states it identically whichever
/// operation did the lookup (`GetSession`, `UpdateSession`, `EndSession`, `DeleteSession`,
/// `CreateInvocation`, `PutInvocationStep`, `GetInvocationStep`, `ListInvocationSteps`), so
/// a client can match one rule rather than one sentence per operation. No trailing period:
/// the template is the sentence, not prose.
pub fn session_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Session with ID {identifier} not found")),
    }
}

/// The refusal the TAG plane answers when its `resourceArn` names no stored resource.
///
/// The tagging operations are addressed by a `TaggableResourcesArn`, whose pattern admits
/// a session ARN and nothing else
/// (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
/// -> `resourceArn`), so the refusal names that one resource KIND in words and carries no
/// identifier at all: a CONSTANT sentence, trailing period included, interpolating neither
/// the ARN nor any part of it. It is worded from the resource kind rather than from the
/// request because the ARN was already resolved — what the caller is told is that no
/// session answers to it, not which ARN was submitted.
pub const TAGGABLE_RESOURCE_NOT_FOUND: &str = "Session resource not found.";

/// The modelled `ResourceNotFoundException` (HTTP 404) carrying
/// [`TAGGABLE_RESOURCE_NOT_FOUND`]: ONE constructor for every tagging operation
/// (`TagResource`, `UntagResource`, `ListTagsForResource`), so the tag plane states one
/// sentence whichever of them did the lookup.
pub fn taggable_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TAGGABLE_RESOURCE_NOT_FOUND.to_string()),
    }
}

/// The modelled `ConflictException` (HTTP 409) a WRITE to a session answers when that
/// session is no longer `ACTIVE`.
///
/// `ENDED` and `EXPIRED` are both terminal — a session in either accepts no further write
/// (`smithy_query(query="doc",shape="EndSession")`) — and the refusal is the ONE
/// terminal-state sentence for every write that hits one (`CreateInvocation`,
/// `PutInvocationStep`, `UpdateSession`), interpolating the id of the session named and
/// naming no particular status: a client matches one rule, not one sentence per operation.
pub fn session_in_terminal_state(session_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Session with id {session_id} is in a terminal state and cannot be updated anymore."
        )),
    }
}

/// The refusal `DeleteSession` answers while the session it names is still `ACTIVE`: a
/// session must be ended before it can be deleted
/// (`smithy_query(query="doc",shape="DeleteSession")`).
///
/// A CONSTANT sentence, interpolating nothing — the refusal states the precondition, not
/// which session was addressed.
pub const ACTIVE_SESSION_NOT_DELETABLE: &str = "Cannot delete session in ACTIVE state";

/// The modelled `ConflictException` (HTTP 409) carrying [`ACTIVE_SESSION_NOT_DELETABLE`].
pub fn active_session_not_deletable() -> ConflictException {
    ConflictException {
        message: Some(ACTIVE_SESSION_NOT_DELETABLE.to_string()),
    }
}

/// A freshly minted version-4 UUID in the canonical lowercase hyphenated form every
/// `Uuid`-shaped member of this model requires (`sessionId`, `invocationId`,
/// `invocationStepId`).
///
/// The generator is the audited `uuid` one, reached through a dependency this crate
/// already has rather than a hand-written one: `emulator_telemetry`'s UUID newtypes
/// are `uuid::Uuid::new_v4().as_hyphenated()` behind `random()` (see
/// `vendor/emulator-telemetry/src/types.rs`), which is how the greengrass service
/// crate mints its ids too. Only the id is taken — nothing here emits telemetry.
///
/// A resource id is not a secret and never a credential: created credentials come only
/// from the emulator's credential store.
pub fn new_uuid() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// The current wall-clock instant, as the model's `DateTimestamp` carries it.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// The longest region segment this model admits inside a `bedrock` ARN: its own
/// `AWSResourceARN` bounds that segment to `[a-z0-9-]{1,20}`
/// (`smithy_query(query="shape",shape="AWSResourceARN")`), and no AWS region name is
/// longer. A scope region past it cannot be part of a well-formed ARN, so it is not
/// stamped into one.
pub const MAX_REGION_SEGMENT: usize = 20;

/// The region to stamp into an ARN this request mints: the one the caller's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope value that is not region-shaped (`SessionArn`'s own `[a-z0-9-]+`
/// segment) falls back too, so a minted `sessionArn` always satisfies the pattern
/// the model declares for it.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_SEGMENT
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The scope a pagination cursor is bound to: the listing that issued it (the
/// operation name), the session being listed, and the `invocationIdentifier` filter
/// when one is supplied.
///
/// A cursor is only meaningful for the listing that issued it, so the scope goes
/// INTO the token: a token minted while listing one session (or one invocation, or a
/// different kind of item) is then refused on another rather than silently resuming
/// at an unrelated offset. Store order is global across this service, so without the
/// operation name a `ListInvocationSteps` cursor would read as a valid
/// `ListInvocations` cursor for the same session.
pub fn pagination_scope(
    listing: &str,
    session_id: &str,
    invocation_filter: Option<&str>,
) -> String {
    format!("{listing}:{session_id}:{}", invocation_filter.unwrap_or(""))
}

/// Encode a pagination cursor: the scope it belongs to, and the store order of the
/// last item already returned.
///
/// Encoded with `aws_smithy_types::base64` — the codec the wire protocols already use
/// for every Smithy blob, so no new one is introduced — which keeps the token opaque
/// to the client and inside the model's `NextToken` constraints (1..2048 chars,
/// `^\S*$`).
pub fn encode_next_token(scope: &str, last_order: u64) -> String {
    ::aws_smithy_types::base64::encode(format!("v1:{scope}:{last_order}"))
}

/// Decode a cursor [`encode_next_token`] issued for `scope`. `None` for anything
/// else — a malformed token, or one issued for a different session or filter —
/// which a handler answers with the modelled `ValidationException`.
pub fn decode_next_token(scope: &str, token: &str) -> Option<u64> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix("v1:")?
        .strip_prefix(scope)?
        .strip_prefix(':')?
        .parse()
        .ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState>)
}

/// The ARN of the session a `sessionIdentifier` names, which is the RESOURCE an
/// authorization decision about a session-scoped call is made against.
///
/// An identifier that is already a session ARN IS that resource; a bare session id names
/// the session in the region the caller signed for and this emulator's account, the same
/// ARN `CreateSession` mints. Built without a store lookup, so the decision is made
/// before — and independently of — whether the session exists.
pub fn session_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    identifier: &str,
) -> String {
    if identifier.starts_with("arn:") {
        return identifier.to_string();
    }
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        &request_region(ctx),
        &format!("session/{identifier}"),
    )
}

/// The IAM service prefix this service's actions carry: `bedrock:GetSession`,
/// `bedrock:PutInvocationStep`, … It is the ARN namespace, not the endpoint prefix
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`), exactly as
/// [`ARN_NAMESPACE`] is.
pub const ACTION_PREFIX: &str = ARN_NAMESPACE;

/// Authorize this request, the one way every session-scoped operation does it.
///
/// `operation` is the operation's name (`Self::NAME` in a handler), which names the IAM
/// action `bedrock:<Operation>`; `resource` is the ARN the request acts on (the session's
/// ARN). `Ok(())` when the caller may make the call, and the modelled
/// `AccessDeniedException` (HTTP 403) when it may not.
///
/// The decision has two halves, and neither is invented here:
///
/// * WHO the caller is comes from the same source `sts:GetCallerIdentity` answers from —
///   the STS session registry, keyed by the access key id the request signed with
///   ([`caller`]) — so the principal a refusal blames is the one the caller reads back
///   from STS for its own credentials.
/// * WHAT that caller may do comes from the identity-based policy documents the IAM store
///   holds for it (a role's inline policies and the default version of each managed policy
///   attached to it; a user's inline policies), evaluated by the ONE shared reading of an
///   IAM document (`service_common::iam_authorization`). IAM's default is deny, so a
///   principal the account grants nothing authorizes nothing. A session has no
///   resource-based policy of its own — the model gives this service no
///   `PutResourcePolicy` — so the identity-based decision is the whole decision.
///
/// THE SAME RULE, AND THE SAME READING OF IT, as
/// `services/service-com-amazonaws-rum-rum/src/state/authorization.rs`, which is the
/// in-tree precedent: the two agree on the account's own credential, on an attachment this
/// emulator cannot read, and on an expired credential (below). The gathering is not shared
/// in `services/common` because it reads the IAM and STS service stores, and
/// `services/common` is a dependency of nearly every service crate — putting those two
/// crates behind it would place them in every service's build graph. Only the DECISION,
/// which needs no store, lives there.
///
/// Three callers are not principals the IAM store holds policies for, and each is
/// answered the way that precedent answers it:
///
/// * the account's own published credential (the long-term key this emulator hands out),
///   which is the position AWS account root credentials are in: allowed;
/// * a principal with an AWS-MANAGED policy attached, whose document the IAM store
///   deliberately does not hold (`iam_state::AWS_MANAGED_POLICY_ACCOUNT`): allowed,
///   because refusing on a grant this emulator cannot read would deny access AWS permits;
/// * an expired session credential, which speaks for nobody: refused.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    use ::service_common::iam_authorization as iam;

    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    if caller.unbounded {
        return Ok(());
    }
    // A session carries no resource-based policy, so the identity-based decision is the
    // whole one and `NotStated` is IAM's implicit deny.
    let request = iam::Request::new(&action, resource, &[]);
    let identity = iam::identity_decision(&caller.documents, &request);
    if iam::is_allowed(identity, iam::Decision::NotStated) {
        return Ok(());
    }
    let reason = if identity == iam::Decision::Denied {
        format!("with an explicit deny in an identity-based policy for the {action} action")
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    Err(AccessDeniedException {
        // Built from the request: the principal that signed it, the action it performs
        // and the resource it names. AWS words this refusal the same way, and a client
        // reads the principal and the action out of it to fix its policy.
        message: Some(format!(
            "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
            caller.arn
        )),
    })
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when the identity side cannot refuse this caller: the account's own published
    /// credential, or a principal carrying a grant this emulator cannot read (see
    /// [`authorize`]).
    pub unbounded: bool,
}

/// The principal THIS request speaks for, and the identity-based policies in force on it.
///
/// Read from the same source `sts:GetCallerIdentity` answers from — the STS session the
/// request's access key id names — so the principal an authorization decision is made
/// about, and the one a refusal blames, is the one the caller reads back from STS for its
/// own credentials. `resolve_caller` materialises the session expiries the clock has
/// already made due before it answers, a write owed at the same instant whether or not
/// anybody called, which is why a REPORTING operation may make this decision.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_state::service_state(engine) {
        Some(sts) => sts_state::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_state::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents and
        // nothing unbounded, which refuses every action it asks for.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                unbounded: false,
            }
        }
    };

    let mut caller = Caller {
        unbounded: identity.arn == sts_state::ambient_identity().arn,
        arn: identity.arn,
        documents: Vec::new(),
    };
    let Some(iam) = iam_state::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_state::name_key(role_name);
        caller
            .documents
            .extend(inline_documents(&store.role_inline_policies, &role_key));
        for attached in store
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                // Only an AWS-MANAGED policy is a grant this emulator cannot read; an
                // attachment naming a customer-managed policy the account does not hold
                // grants nothing, because the account IS the authority on its own.
                None if is_aws_managed_policy(attached) => caller.unbounded = true,
                None => {}
            }
        }
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_state::name_key(user_name),
        ));
    }
    caller
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's `aws`,
/// so it belongs to AWS rather than to this account and the IAM store holds no document
/// for it. Read through IAM's own ARN parser, so this service and IAM agree on what an
/// AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_state::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_state::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`
/// (`sts_state::sts_arn`), so the role is the first segment after `assumed-role/`. `None`
/// for any other ARN form.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The ACCOUNT the principal a request speaks for acts in, read off that principal's own
/// ARN ([`Caller::arn`]), or `None` when it names none — an expired session credential,
/// which speaks for nobody.
///
/// What it is FOR: an authorization decision is made about a principal, and the resources
/// this emulator holds belong to one account ([`::engine::emulator_account_id`]), so an
/// operation that resolves a resource after that decision keys the resolution to the same
/// principal's account rather than to the request's identifiers alone. Read through the ONE
/// ARN parse this tree has (`service_common::arn`).
#[must_use]
pub fn caller_account(caller: &Caller) -> Option<String> {
    ::service_common::arn::parse(&caller.arn).map(|arn| arn.account.to_string())
}

/// The resource part of an ARN whose service segment is `service`.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and the resource
/// may itself carry colons, so only the first five fields are split off.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(named), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (named == service).then_some(resource)
}

/// The inline policy documents one entity holds, from the IAM store's
/// `<entity> -> <policy name> -> policy` map.
fn inline_documents(
    policies: &BTreeMap<String, BTreeMap<String, iam_state::InlinePolicy>>,
    entity_key: &str,
) -> Vec<String> {
    policies
        .get(entity_key)
        .map(|held| {
            held.values()
                .map(|policy| policy.policy_document.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The document of a managed policy's DEFAULT version — the one in force on every entity
/// it is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_state::ManagedPolicy) -> Option<String> {
    let default = managed.policy.default_version_id.as_deref();
    managed
        .versions
        .values()
        .find(|version| default.is_some() && version.version_id.as_deref() == default)
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}
