// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-qconnect-wisdomservice service.
//!
//! The service's root resource is the Amazon Q in Connect *assistant*; a session is
//! created inside one assistant and is reachable only through its parent. Everything a
//! handler answers with is derived from the records kept here, so what one request
//! writes is what the next request reads.
//!
//! One resource FAMILY whose records and projections stand on their own lives in a child
//! module beside this one, re-exported here so every handler keeps reaching the store
//! through `crate::state::…`.

/// The AI-prompt family: the record, its projection and its lookups.
mod ai_prompts;
pub use ai_prompts::*;

/// The published versions of an AI agent: the snapshot record, its projections and its
/// lookups.
mod ai_agent_versions;
pub use ai_agent_versions::*;

/// The published versions of an AI prompt: the snapshot record, its projections and its
/// lookups.
mod ai_prompt_versions;
pub use ai_prompt_versions::*;

/// The message-template family: the record, its projections and its lookups.
mod message_templates;
pub use message_templates::*;

/// The published versions of a message template: the snapshot record, its projections and
/// its lookups.
mod message_template_versions;
pub use message_template_versions::*;

/// The attachment files of a message template: the record, its projection and the locator
/// the download plane serves one under.
mod message_template_attachments;
pub use message_template_attachments::*;

/// Rendering a message template: the variable placeholders its content names and the
/// attribute values a render resolves them from.
mod message_template_render;
pub use message_template_render::*;

/// The Amazon SNS topic an assistant streams chat messages through, provisioned in and
/// resolved against the service that owns topics.
mod chat_streaming_topic;
pub use chat_streaming_topic::*;

/// The replica-Region KMS keys a resource's encryption configuration carries.
mod replica_keys;
pub use replica_keys::*;

/// The URLs this service hands out for a caller to follow later, and where they are
/// signed.
mod signed_urls;
pub use signed_urls::*;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    AIAgentConfiguration, AIAgentConfigurationData, AIAgentData, AIAgentSummary,
    AIGuardrailContentPolicyConfig,
    AIGuardrailContextualGroundingPolicyConfig, AIGuardrailData,
    AIGuardrailSensitiveInformationPolicyConfig, AIGuardrailTopicPolicyConfig,
    AIGuardrailWordPolicyConfig, AssistantAssociationData, AssistantAssociationInputData,
    AssistantAssociationOutputData, AssistantAssociationSummary, AssistantCapabilityConfiguration,
    AssistantData,
    AssistantIntegrationConfiguration, AssistantSummary, ContentData, ContentSummary,
    ExternalBedrockKnowledgeBaseConfig,
    GroupingConfiguration, KnowledgeBaseAssociationData, KnowledgeBaseData, KnowledgeBaseSummary,
    OrchestratorConfigurationEntry,
    QuickResponseContentProvider, QuickResponseContents, QuickResponseData,
    QuickResponseSearchResultData, QuickResponseSummary,
    RenderingConfiguration, RecommendationData, RecommendationTrigger,
    ServerSideEncryptionConfiguration, SessionData, SessionIntegrationConfiguration,
    SourceConfiguration, TagFilter, VectorIngestionConfiguration,
};

/// The only `AssistantType` the model declares
/// (`smithy_query(query="shape",shape="AssistantType")`).
pub const ASSISTANT_TYPE_AGENT: &str = "AGENT";

/// The terminal `AssistantStatus` of a usable assistant, and the one a create answers
/// with: the service reports the assistant ready in the response to the call that made
/// it, so no later status change is owed and every read reports the same value
/// (`smithy_query(query="shape",shape="AssistantStatus")` also declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `DELETE_IN_PROGRESS`, `DELETE_FAILED` and
/// `DELETED`, which this emulator reaches for no assistant it holds: a delete removes
/// the record outright).
pub const ASSISTANT_STATUS_ACTIVE: &str = "ACTIVE";

/// The `AssistantCapabilityType` an assistant created through the Amazon Q in Connect
/// API carries (`smithy_query(query="shape",shape="AssistantCapabilityType")` declares
/// `V1` and `V2`): `V1` is the legacy Amazon Connect Wisdom capability, `V2` the one
/// this model's AI agent, AI prompt and AI guardrail surface needs. `CreateAssistant`
/// takes no capability member, so the service chooses it.
pub const ASSISTANT_CAPABILITY_V2: &str = "V2";

/// The `Origin` of a session created through the `CreateSession` API — the model's own
/// wording: "`SYSTEM` for a default Session created by Amazon Q in Connect or
/// `CUSTOMER` for a Session created by calling CreateSession API".
pub const SESSION_ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// The `Origin` of the session the SERVICE creates for an assistant, the other half of
/// the wording quoted on [`SESSION_ORIGIN_CUSTOMER`]: the default session Amazon Q in
/// Connect provisions with the assistant rather than one a caller asked for.
pub const SESSION_ORIGIN_SYSTEM: &str = "SYSTEM";

/// The `sessionId` of the service-managed session every assistant owns.
///
/// Amazon Q in Connect provisions one default session per assistant and addresses it by
/// this fixed all-zero id, which is why a client that was given no session of its own
/// (the Amazon Connect integration path) can still read and notify the assistant's
/// session. It satisfies the modelled `Uuid` form
/// (`smithy_query(query="shape",shape="Uuid")`) without being a minted id, so no
/// [`mint_candidate`] can collide with it.
pub const SYSTEM_SESSION_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `name` of the service-managed session [`SYSTEM_SESSION_ID`] names. Fixed text the
/// service gives it — the caller names nothing, since the caller did not create it.
pub const SYSTEM_SESSION_NAME: &str = "QinConnectSystemSession";

/// The `description` of the service-managed session [`SYSTEM_SESSION_ID`] names, fixed
/// text on the same footing as [`SYSTEM_SESSION_NAME`].
pub const SYSTEM_SESSION_DESCRIPTION: &str = "System-managed session for Amazon Q in Connect.";

/// The page size `ListAssistants` serves when a call names no `maxResults`: the
/// modelled maximum of the `maxResults` range (1..100).
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The most assistants one account may hold at a time; a create beyond it is refused
/// with the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN: AWS quotas assistants per account but does not publish the figure. The bound
/// is also what keeps this store finite — a record is released only by
/// `DeleteAssistant`.
pub const MAX_ASSISTANTS: usize = 100;

/// The most live sessions one assistant may hold at a time, with the same contract as
/// [`MAX_ASSISTANTS`]. `CreateSession` models no quota error, so a create beyond this
/// bound is refused with its modelled `ValidationException` (HTTP 400).
pub const MAX_SESSIONS_PER_ASSISTANT: usize = 200;

/// The `KnowledgeBaseStatus` a knowledge base this service holds carries
/// (`smithy_query(query="shape",shape="KnowledgeBaseStatus")` also declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `DELETE_IN_PROGRESS`, `DELETE_FAILED` and
/// `DELETED`).
///
/// A `CreateKnowledgeBase` answer is recorded with `status` ACTIVE, so the create owes no
/// later transition: a resource the service returns ready stays ready. The assistant
/// above is recorded the same way — see [`ASSISTANT_STATUS_ACTIVE`].
pub const KNOWLEDGE_BASE_STATUS_ACTIVE: &str = "ACTIVE";

/// The sentence this service answers a request addressed to an assistant this account
/// does not hold with.
///
/// Every message constant below is a FIXED service text: it is the same sentence for
/// every request that meets the condition and it quotes nothing from the request. The
/// recorded wire responses show exactly that — the not-found answer to a request for an
/// assistant id names no id — so a message built by interpolating the requested
/// identifier is a message this service never sends. The requested label still travels,
/// in `ResourceNotFoundException`'s own `resourceName` member, which the model provides
/// for naming it.
pub const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";

/// The fixed text for a request addressed to a knowledge base this account does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The internal service that HOLDS this endpoint's knowledge bases, as a refusal relayed
/// from it names that service.
///
/// The knowledge-base plane is a service of its own behind this endpoint, so an operation
/// of the ASSISTANT plane that takes a `knowledgeBaseId` resolves it by CALLING that
/// service instead of answering for it, and the refusal that call comes back with is
/// reported relayed: the called service's own sentence followed by the envelope naming the
/// call ([`message_knowledge_base_not_found_relayed`]). An operation addressed to the
/// knowledge-base plane itself is answered BY that service, so there is no relayed call to
/// name and its refusal is reported bare ([`MESSAGE_KNOWLEDGE_BASE_NOT_FOUND`]).
///
/// The name is the one the SDK layer that relays the call reports the service under, which
/// is neither this endpoint's signing name nor its `sdkId`. The Amazon Connect Wisdom view
/// of this same endpoint relays it from its own copy of this constant
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client cannot
/// read a different service out of the message depending on which model it addressed.
pub const KNOWLEDGE_BASE_SERVICE: &str = "CloverKnowledge";

/// The status the knowledge-base service answers a `knowledgeBaseId` it holds nothing under
/// with: the `@httpError` of the `ResourceNotFoundException` its own operations declare
/// (`smithy_query(query="operation",operation="GetKnowledgeBase")`), which is the status the
/// same lookup answers a client with when the client makes it directly.
pub const KNOWLEDGE_BASE_NOT_FOUND_STATUS: u16 = 404;

/// [`MESSAGE_KNOWLEDGE_BASE_NOT_FOUND`] as an operation of the ASSISTANT plane reports it:
/// the knowledge-base service's own sentence, then the envelope naming the relayed call it
/// came from — rendered by the workspace's one writer of that envelope
/// (`service_common::relayed_refusal`), so the form cannot drift per service.
///
/// The request id in it belongs to the RELAYED call and is minted per call, so two refusals
/// never report one id and nothing in the request decides it.
pub fn message_knowledge_base_not_found_relayed() -> String {
    ::service_common::relayed_refusal::relayed_refusal_message(
        MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        ::service_common::relayed_refusal::RelayedCall::once(
            KNOWLEDGE_BASE_SERVICE,
            KNOWLEDGE_BASE_NOT_FOUND_STATUS,
        ),
    )
}

/// The fixed text for a request addressed to a session its named assistant does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_SESSION_NOT_FOUND: &str = "Session does not exist.";

/// The fixed text for a request addressed to an assistant association its named
/// assistant does not hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_ASSOCIATION_NOT_FOUND: &str = "Assistant Association does not exist.";

/// The most tags one resource carries.
///
/// AWS's tagging restrictions put the maximum number of tags per resource at 50, and
/// this model puts no constraint on the `Tags` map
/// (`smithy_query(query="shape",shape="Tags")` constrains only each key and value), so
/// without the limit one caller could store an arbitrarily large tag map in this
/// process's heap. A `TagResource` that would leave the resource holding more than this
/// is refused the way the recorded wire answer of this endpoint refuses it
/// (`crate::ops::tag_resource`).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The sentence a tagging operation refuses a tag set larger than
/// [`MAX_TAGS_PER_RESOURCE`] with.
///
/// It states the RULE — the ceiling the RESULTING tag set must stay within — and is the
/// sentence AWS's common tag validator words that rule with, counting the user tags a
/// resource would be left holding. It carries nothing from the request and nothing about
/// what the resource already holds, so a caller cannot read the stored tag count out of
/// the wording. The Amazon Connect Wisdom view of this same endpoint words it from its
/// own copy of this text
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client
/// cannot get a different sentence depending on which model it addressed.
pub fn message_too_many_tags() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags.")
}

/// The success status an operation of this endpoint whose output structure declares NO
/// members answers with: 204 No Content, with no document in the body.
///
/// Every member-less response of this endpoint is answered this way. Most of them say so
/// in the model — the deletes and the removes carry an explicit `@http` code
/// (`smithy_query(query="operation",operation="DeleteAssistant")`: `code: 204`), and an
/// operation whose model states a code keeps the stated one (the AI-guardrail deletes
/// state 202, which this constant is not used for). The two tagging mutations carry no
/// `@http` code at all, so they would otherwise fall back to the protocol default 200,
/// which tells a client a document follows where none does; they use this constant
/// instead (`crate::ops::tag_resource`, `crate::ops::untag_resource`).
pub const EMPTY_RESPONSE_SUCCESS_STATUS: u16 = 204;

/// The fixed text of the modelled `ConflictException` a create whose `name` a live
/// resource already holds is refused with. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_NAME_IN_USE: &str = "Name is already in use";

/// The fixed text of the modelled `ConflictException` a create that reuses an
/// `@idempotencyToken` with different parameters is refused with. See
/// [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// The fixed text of the modelled `ConflictException` a `CreateAssistantAssociation`
/// naming a target its assistant is already associated with is refused with — the
/// sentence the recorded answer of that endpoint carries for this class. Like every
/// message constant here it is FIXED text: it names neither the assistant, nor the
/// knowledge base, nor the association that already holds the pair. See
/// [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_ASSOCIATION_ALREADY_EXISTS: &str =
    "Association already exists for these two resources";

/// The fixed text of the modelled `UnauthorizedException`. See
/// [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_UNAUTHORIZED: &str = "Unauthorized";

/// The `Origin` of an AI agent the service created for an assistant itself, as the
/// model words it: "`SYSTEM` for a default AI Agent created by Q in Connect or
/// `CUSTOMER` for an AI Agent created by calling AI Agent creation APIs"
/// (`smithy_query(query="shape",shape="AIAgentData")`, member `origin`).
pub const AI_AGENT_ORIGIN_SYSTEM: &str = "SYSTEM";

/// The `Origin` of an AI agent a CALLER created, the other half of the wording quoted on
/// [`AI_AGENT_ORIGIN_SYSTEM`]: an agent made by calling the AI agent creation APIs rather
/// than one Amazon Q in Connect provisioned with the assistant.
pub const AI_AGENT_ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// The `VisibilityStatus` of a default AI agent: it is in service for the assistant the
/// moment the assistant exists, which is what `PUBLISHED` means
/// (`smithy_query(query="shape",shape="VisibilityStatus")` declares `SAVED` and
/// `PUBLISHED`).
pub const AI_AGENT_VISIBILITY_PUBLISHED: &str = "PUBLISHED";

/// The terminal `Status` of an AI resource in service — an AI agent, and an AI guardrail
/// too, since the model gives both the one `Status` shape
/// (`smithy_query(query="shape",shape="Status")`, which also declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `DELETE_IN_PROGRESS`, `DELETE_FAILED` and
/// `DELETED`).
///
/// A create answers with the resource already in service, so no later status change is
/// owed and every read reports the same value — the rule [`ASSISTANT_STATUS_ACTIVE`]
/// states for the assistant that holds them, and a delete removes the record outright
/// rather than moving it through a deleting status.
pub const AI_RESOURCE_STATUS_ACTIVE: &str = "ACTIVE";

/// The one AI agent type whose binding can be turned off, which is why its binding is
/// the one that reports `enabled`: the model's `AIAgentConfigurationData$enabled` doc
/// says "Setting this value to false is currently supported only for the
/// `ANSWER_RECOMMENDATION` AI Agent type".
///
/// Its default binding is reported DISABLED, which is what the recorded wire answer for
/// a fresh assistant shows (and what the model's wording describes as "the AI Agent ID
/// is retained but no AI Agent runs for the AI Agent type"): answer recommendation runs
/// against knowledge-base content, and a fresh assistant is associated with none yet.
/// Every other type's binding reports no `enabled` at all, since no other type's can be
/// turned off.
pub const AI_AGENT_TYPE_ANSWER_RECOMMENDATION: &str = "ANSWER_RECOMMENDATION";

/// Which revision of a built-in AI agent the service's own binding to it names: the
/// agent's floating alias, or one published version of it.
///
/// The qualifier is part of the service's published identity for the type's default
/// agent, not something an assistant or a session decides, so it is recorded per
/// built-in agent beside that agent's id — see [`BUILT_IN_AI_AGENTS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInAiAgentRevision {
    /// The agent itself, named by the floating [`VERSION_QUALIFIER_LATEST`] alias: the
    /// binding tracks the agent rather than pinning one of its versions, so a client that
    /// follows the reported id reaches the agent that is running for the type.
    Latest,
    /// One published version of the agent, named by its number. Within the modelled
    /// `Version` range (`smithy_query(query="shape",shape="Version")`: `range 1..`).
    Version(i64),
}

impl BuiltInAiAgentRevision {
    /// The `:<QUALIFIER>` text this revision is named by, which is what a binding appends
    /// to the agent's id — see [`qualified_ai_agent_id`].
    pub fn qualifier(self) -> String {
        match self {
            Self::Latest => VERSION_QUALIFIER_LATEST.to_string(),
            Self::Version(version_number) => version_number.to_string(),
        }
    }

    /// The version number this revision pins, or `None` when it names the agent's
    /// floating alias instead — which is what a read of the agent reports for
    /// `versionNumber`: a label carrying the floating alias names the agent's own working
    /// copy, which is no numbered version.
    pub fn published_version(self) -> Option<i64> {
        match self {
            Self::Latest => None,
            Self::Version(version_number) => Some(version_number),
        }
    }
}

/// One built-in AI agent of the service: the `AIAgentType` it serves, the id Amazon Q in
/// Connect publishes it under and the revision of it the service's own binding names.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInAiAgent {
    pub r#type: &'static str,
    pub ai_agent_id: &'static str,
    /// The revision of this agent the binding that names it qualifies the id with — see
    /// [`qualified_ai_agent_id`].
    pub revision: BuiltInAiAgentRevision,
}

/// The built-in (`SYSTEM`-origin) AI agents Amazon Q in Connect provisions every
/// assistant with, which is what a fresh assistant's and a fresh session's
/// `aiAgentConfiguration` is bound to.
///
/// These agents belong to the SERVICE, not to the assistant that reports them, so their
/// ids and the revisions bound to them are service constants: every assistant and every
/// session, in every account and region, names the same eight `<id>:<qualifier>` pairs.
/// The ids and qualifiers below are the ones the recorded answers of `CreateAssistant`,
/// `CreateSession` and `GetSession` report, which agree across independently created
/// assistants: the service names the answer-recommendation default by its floating alias
/// and every other type's default by a published version number.
///
/// One entry per `AIAgentType` the model declares
/// (`smithy_query(query="shape",shape="AIAgentType")`) except `ORCHESTRATION`, which has
/// no built-in agent: a recorded fresh assistant carries a binding for each of the eight
/// below and none for `ORCHESTRATION`. Sorted by type, so the list reads as the set it
/// is.
pub const BUILT_IN_AI_AGENTS: [BuiltInAiAgent; 8] = [
    BuiltInAiAgent {
        r#type: "ANSWER_RECOMMENDATION",
        ai_agent_id: "3025a3fa-5419-4c9c-a040-877213b93f70",
        revision: BuiltInAiAgentRevision::Latest,
    },
    BuiltInAiAgent {
        r#type: "CASE_SUMMARIZATION",
        ai_agent_id: "9114812c-2a1e-4501-8817-daec4223ce57",
        revision: BuiltInAiAgentRevision::Version(3),
    },
    BuiltInAiAgent {
        r#type: "EMAIL_GENERATIVE_ANSWER",
        ai_agent_id: "4d7e9f2a-1b3c-5d6e-8f7a-9b0c1d2e3f4a",
        revision: BuiltInAiAgentRevision::Version(7),
    },
    BuiltInAiAgent {
        r#type: "EMAIL_OVERVIEW",
        ai_agent_id: "91bb967b-bd5e-4e02-a8d5-cb7c5b2bf9c7",
        revision: BuiltInAiAgentRevision::Version(5),
    },
    BuiltInAiAgent {
        r#type: "EMAIL_RESPONSE",
        ai_agent_id: "8f4e7d2a-9c31-4b5e-a6f8-e2d9c7b54321",
        revision: BuiltInAiAgentRevision::Version(7),
    },
    BuiltInAiAgent {
        r#type: "MANUAL_SEARCH",
        ai_agent_id: "652f0287-1610-4fee-bbe3-c687c13f5ec3",
        revision: BuiltInAiAgentRevision::Version(9),
    },
    BuiltInAiAgent {
        r#type: "NOTE_TAKING",
        ai_agent_id: "7fc84960-5b46-4e44-9dd6-5956b2acfaa4",
        revision: BuiltInAiAgentRevision::Version(3),
    },
    BuiltInAiAgent {
        r#type: "SELF_SERVICE",
        ai_agent_id: "529207fe-80fd-42be-bcf2-facd2141aec7",
        revision: BuiltInAiAgentRevision::Version(5),
    },
];

/// The binding an assistant reports for one `AIAgentType` while no caller has configured
/// an agent of its own for it: the SERVICE's built-in agent for the type, named by its
/// published id and the revision of it the service binds (see [`qualified_ai_agent_id`]).
///
/// `None` when the service publishes no built-in agent for the label — the `ORCHESTRATION`
/// type, and any string that is no `AIAgentType` this model declares — so a type with no
/// default is reported by no binding at all until a caller configures one.
///
/// The one place a default binding is built, so the bindings a create provisions
/// ([`default_ai_agents`]) and the binding a `RemoveAssistantAIAgent` falls back to are
/// the same value.
pub fn default_ai_agent_binding(agent_type: &str) -> Option<AIAgentConfigurationData> {
    BUILT_IN_AI_AGENTS
        .into_iter()
        .find(|agent| agent.r#type == agent_type)
        .map(built_in_ai_agent_binding)
}

/// The binding that names one built-in agent, which is what every default binding is:
/// taken from the agent's own record, so neither the id nor the revision is materialised
/// here.
fn built_in_ai_agent_binding(agent: BuiltInAiAgent) -> AIAgentConfigurationData {
    AIAgentConfigurationData {
        // The binding names the agent by its id AND the version of it that runs — the
        // model's own wording for the member is "The ID of the AI Agent to be configured",
        // and the map it lives in maps an AI agent type "to AI Agent version" — so what an
        // assistant reports is the id of the service's agent for the type, qualified by the
        // revision of it the service publishes for that type.
        ai_agent_id: qualified_ai_agent_id(agent.ai_agent_id, agent.revision),
        // The model's own wording: "Indicates whether the AI Agent configured for this AI
        // Agent type is enabled … Setting this value to false is currently supported only
        // for the `ANSWER_RECOMMENDATION` AI Agent type" — see
        // [`AI_AGENT_TYPE_ANSWER_RECOMMENDATION`] for why that one default is bound
        // disabled and why no other type reports the member.
        enabled: (agent.r#type == AI_AGENT_TYPE_ANSWER_RECOMMENDATION).then_some(false),
    }
}

/// The AI agent types an assistant gets a default `SYSTEM` AI agent for when the service
/// creates it, read off [`BUILT_IN_AI_AGENTS`] so the two cannot drift apart.
pub const DEFAULT_AI_AGENT_TYPES: [&str; BUILT_IN_AI_AGENTS.len()] = [
    BUILT_IN_AI_AGENTS[0].r#type,
    BUILT_IN_AI_AGENTS[1].r#type,
    BUILT_IN_AI_AGENTS[2].r#type,
    BUILT_IN_AI_AGENTS[3].r#type,
    BUILT_IN_AI_AGENTS[4].r#type,
    BUILT_IN_AI_AGENTS[5].r#type,
    BUILT_IN_AI_AGENTS[6].r#type,
    BUILT_IN_AI_AGENTS[7].r#type,
];

/// The fixed text for a request addressed to an AI agent its named assistant does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`] for why the sentence quotes nothing from the
/// request.
pub const MESSAGE_AI_AGENT_NOT_FOUND: &str = "AI Agent does not exist.";

/// The most AI agents one assistant may hold at a time, with the same contract as
/// [`MAX_AI_GUARDRAILS_PER_ASSISTANT`]: a create beyond it is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402), which `CreateAIAgent` declares for exactly
/// this, and which is also what keeps this store finite — an agent is released only by
/// `DeleteAIAgent` or by the delete of its assistant.
///
/// CHOSEN: AWS quotas AI agents per assistant but does not publish the figure. The
/// built-in agents the SERVICE provisions (see [`BUILT_IN_AI_AGENTS`]) are not what a
/// caller asked for, so they do not spend the caller's allowance — the rule
/// [`Assistant::customer_session_count`] states for the other family the service
/// provisions records in.
pub const MAX_AI_AGENTS_PER_ASSISTANT: usize = 100;

/// The sentence a create past [`MAX_AI_AGENTS_PER_ASSISTANT`] is refused with, stating
/// the RULE — the ceiling and what to do about it — and carrying nothing from the request
/// or from what the assistant already holds, the way [`message_ai_guardrail_quota`] does
/// for the other family with a per-parent ceiling.
pub fn message_ai_agent_quota() -> String {
    format!(
        "The maximum number of AI Agents ({MAX_AI_AGENTS_PER_ASSISTANT}) in this assistant \
         has been reached. Delete an AI Agent before creating another."
    )
}

/// The sentence a create or update whose `configuration` is not the union member the AI
/// agent's `type` is configured through is refused with. It states the RULE and names the
/// type the request itself carried.
pub fn message_configuration_type_mismatch(agent_type: &str) -> String {
    format!("The configuration must be the AI Agent configuration for type {agent_type}.")
}

/// The sentence a request that would bind an AI agent to an `AIAgentType` the agent is
/// not of is refused with. It states the RULE — a type runs an agent OF that type — and
/// names the type the request itself carried.
///
/// An AI agent's `type` is fixed when it is created and an assistant's binding map is
/// documented as mapping an AI agent type "to AI Agent version"
/// (`smithy_query(query="shape",shape="AssistantData")`, member `aiAgentConfiguration`),
/// so an agent of another type is one the named type cannot run. The class is the modelled
/// `ValidationException` (HTTP 400), which the model declares for a request the service's
/// own constraints refuse.
pub fn message_ai_agent_type_mismatch(agent_type: &str) -> String {
    format!("The AI Agent configured for AI Agent type {agent_type} must be an AI Agent of that type.")
}

/// The sentence a request that would turn a binding off for a type whose binding cannot
/// be turned off is refused with, stating the rule the model's own
/// `AIAgentConfigurationData$enabled` doc states: "Setting this value to false is
/// currently supported only for the `ANSWER_RECOMMENDATION` AI Agent type; other requests
/// to set it to false are rejected with a validation error" — see
/// [`AI_AGENT_TYPE_ANSWER_RECOMMENDATION`].
pub fn message_ai_agent_not_disableable() -> String {
    format!(
        "Setting enabled to false is supported only for the \
         {AI_AGENT_TYPE_ANSWER_RECOMMENDATION} AI Agent type."
    )
}

/// The sentence an update addressed to a built-in ([`AI_AGENT_ORIGIN_SYSTEM`]-origin) AI
/// agent is refused with: those records are the ones Amazon Q in Connect provisions with
/// the assistant (see [`default_ai_agents`]), so they are not a caller's to write — an
/// account's own configuration goes on an agent that account created.
///
/// CHOSEN WORDING: AWS states the rule (a default AI agent cannot be edited; make one of
/// your own instead) but publishes no sentence for this refusal, so this one states that
/// rule and carries nothing from the request. The class is the modelled
/// `ValidationException` (HTTP 400), which the model declares for a request the service's
/// own constraints refuse.
pub fn message_system_ai_agent_not_modifiable() -> String {
    "A default AI Agent created by Amazon Q in Connect cannot be modified. Create an AI \
     Agent of your own to carry this configuration."
        .to_string()
}

/// The sentence a DELETE addressed to a built-in ([`AI_AGENT_ORIGIN_SYSTEM`]-origin) AI
/// agent is refused with: the same rule [`message_system_ai_agent_not_modifiable`] states,
/// for the write that would remove the record rather than change it — those agents are the
/// ones Amazon Q in Connect provisions with the assistant, and every binding a fresh
/// assistant and session reports names one of them.
///
/// CHOSEN WORDING: AWS states the rule (a default AI agent is the service's, not an
/// account's) but publishes no sentence for this refusal, so this one states that rule and
/// carries nothing from the request. The class is the modelled `ValidationException`
/// (HTTP 400), which the model declares for a request the service's own constraints refuse.
pub fn message_system_ai_agent_not_deletable() -> String {
    "A default AI Agent created by Amazon Q in Connect cannot be deleted.".to_string()
}

/// The `AIAgentType` one `AIAgentConfiguration` configures.
///
/// The model declares that union as "a typed union that specifies the configuration based
/// on the type of AI Agent" and documents each member as the configuration "for AI Agents
/// of type `<TYPE>`" (`smithy_query(query="shape",shape="AIAgentConfiguration")`), so a
/// configuration names exactly one type — which is what an operation taking both a `type`
/// and a `configuration` holds the pair to.
pub fn ai_agent_configuration_type(configuration: &AIAgentConfiguration) -> &'static str {
    match configuration {
        AIAgentConfiguration::manualSearchAIAgentConfiguration(_) => "MANUAL_SEARCH",
        AIAgentConfiguration::answerRecommendationAIAgentConfiguration(_) => {
            AI_AGENT_TYPE_ANSWER_RECOMMENDATION
        }
        AIAgentConfiguration::selfServiceAIAgentConfiguration(_) => "SELF_SERVICE",
        AIAgentConfiguration::emailResponseAIAgentConfiguration(_) => "EMAIL_RESPONSE",
        AIAgentConfiguration::emailOverviewAIAgentConfiguration(_) => "EMAIL_OVERVIEW",
        AIAgentConfiguration::emailGenerativeAnswerAIAgentConfiguration(_) => {
            "EMAIL_GENERATIVE_ANSWER"
        }
        AIAgentConfiguration::orchestrationAIAgentConfiguration(_) => "ORCHESTRATION",
        AIAgentConfiguration::noteTakingAIAgentConfiguration(_) => "NOTE_TAKING",
        AIAgentConfiguration::caseSummarizationAIAgentConfiguration(_) => "CASE_SUMMARIZATION",
    }
}

/// The configuration a built-in (`SYSTEM`-origin) AI agent of `agent_type` reports: the
/// union member that type is configured through, carrying no member of its own. `None`
/// when the label is no `AIAgentType` this model declares, since no configuration is
/// declared for it.
///
/// A built-in agent runs the SERVICE's own prompts and guardrails, which belong to no
/// account and which this model gives a caller no id for, so the record states the typed
/// shape of the configuration and claims nothing beyond it. An agent a CALLER creates
/// reports the configuration that caller sent, member for member.
///
/// The one mapping [`ai_agent_configuration_type`] states, read the other way round, so
/// the two cannot drift apart.
pub fn ai_agent_configuration_for_type(agent_type: &str) -> Option<AIAgentConfiguration> {
    [
        AIAgentConfiguration::manualSearchAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::answerRecommendationAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::selfServiceAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::emailResponseAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::emailOverviewAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::emailGenerativeAnswerAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::orchestrationAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::noteTakingAIAgentConfiguration(Default::default()),
        AIAgentConfiguration::caseSummarizationAIAgentConfiguration(Default::default()),
    ]
    .into_iter()
    .find(|configuration| ai_agent_configuration_type(configuration) == agent_type)
}

/// The most entries one AI agent's configuration carries across the lists it is created
/// with — the association configurations, the suggested messages, and the orchestration
/// agent's tool, collaborator and schema lists.
///
/// The model puts no maximum on most of those lists
/// (`smithy_query(query="shape",shape="AssociationConfigurationList")` and its siblings),
/// so without this ceiling one caller could leave an arbitrarily long configuration in
/// this process's heap — the same reason [`MAX_TAGS_PER_RESOURCE`] exists for the equally
/// unbounded tag map, and the one [`MAX_AI_GUARDRAIL_POLICY_ENTRIES`] states for the other
/// family whose create carries open-ended lists.
///
/// It is a HEAP GUARD rather than a quota: it sits an order of magnitude above the most
/// associations an assistant can hold ([`MAX_ASSOCIATIONS_PER_ASSISTANT`]), which is what
/// an association configuration list can meaningfully name, so a configuration AWS would
/// accept is never refused here, and the figure is this emulator's own.
pub const MAX_AI_AGENT_CONFIGURATION_ENTRIES: usize = 1_000;

/// The sentence a create or update whose configuration carries more than
/// [`MAX_AI_AGENT_CONFIGURATION_ENTRIES`] entries between its lists is refused with. It
/// states the RULE — the ceiling the resulting configuration must stay within — and
/// carries nothing from the request.
pub fn message_too_many_configuration_entries() -> String {
    format!(
        "The resultant AI Agent configuration must not have more than \
         {MAX_AI_AGENT_CONFIGURATION_ENTRIES} entries."
    )
}

/// How many entries one AI agent configuration's lists carry between them, which is what
/// [`MAX_AI_AGENT_CONFIGURATION_ENTRIES`] bounds: every list of the union member the
/// request carried, counted once, since it is the whole stored configuration the ceiling
/// is about.
pub fn ai_agent_configuration_entries(configuration: &AIAgentConfiguration) -> usize {
    /// The entries one optional list carries.
    fn count<T>(list: &Option<Vec<T>>) -> usize {
        list.as_ref().map_or(0, Vec::len)
    }

    match configuration {
        AIAgentConfiguration::manualSearchAIAgentConfiguration(config) => {
            count(&config.association_configurations)
        }
        AIAgentConfiguration::answerRecommendationAIAgentConfiguration(config) => {
            count(&config.association_configurations)
                .saturating_add(count(&config.suggested_messages))
        }
        AIAgentConfiguration::selfServiceAIAgentConfiguration(config) => {
            count(&config.association_configurations)
        }
        AIAgentConfiguration::emailResponseAIAgentConfiguration(config) => {
            count(&config.association_configurations)
        }
        AIAgentConfiguration::emailGenerativeAnswerAIAgentConfiguration(config) => {
            count(&config.association_configurations)
        }
        AIAgentConfiguration::orchestrationAIAgentConfiguration(config) => {
            count(&config.tool_configurations)
                .saturating_add(count(&config.multi_agent_configurations))
                .saturating_add(count(&config.input_schemas))
                .saturating_add(count(&config.output_schemas))
        }
        // The configurations of these types carry no list at all: an AI prompt id, an AI
        // guardrail id and a locale, each a single bounded value.
        AIAgentConfiguration::emailOverviewAIAgentConfiguration(_)
        | AIAgentConfiguration::noteTakingAIAgentConfiguration(_)
        | AIAgentConfiguration::caseSummarizationAIAgentConfiguration(_) => 0,
    }
}

/// The most orchestrator use cases one assistant may carry a configuration for at a time,
/// which is what keeps this part of the store finite: an entry is released only by
/// `RemoveAssistantAIAgent` naming its use case, or by the delete of its assistant.
///
/// It is a HEAP GUARD rather than a published quota — AWS does not publish a figure — and
/// the same one [`MAX_AI_GUARDRAILS_PER_ASSISTANT`] states for the other family with a
/// per-assistant ceiling: a use case is a caller-chosen string of up to the modelled
/// `NonEmptyString` length, so without a ceiling one caller could leave an arbitrarily
/// long list in this process's heap. `UpdateAssistantAIAgent` models no quota error, so
/// the refusal past it is the operation's `ValidationException` (HTTP 400). Reconfiguring
/// a use case the assistant already carries is never refused by it: that entry is
/// replaced, so the list does not grow.
pub const MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT: usize = 100;

/// The sentence a configure past [`MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT`] is
/// refused with, stating the RULE — the ceiling and what to do about it — and carrying
/// nothing from the request, the way [`message_ai_guardrail_quota`] does for the other
/// family with a per-assistant ceiling.
pub fn message_orchestrator_configuration_quota() -> String {
    format!(
        "The maximum number of orchestrator configurations \
         ({MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT}) in this assistant has been \
         reached. Remove an orchestrator configuration before adding another."
    )
}

/// The fixed text for a request addressed to an AI guardrail its named assistant does
/// not hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`] for why the sentence quotes nothing
/// from the request.
pub const MESSAGE_AI_GUARDRAIL_NOT_FOUND: &str = "AI Guardrail does not exist.";

/// The most AI guardrails one assistant may hold at a time. A create beyond it is
/// refused with the modelled `ServiceQuotaExceededException` (HTTP 402), which
/// `CreateAIGuardrail` declares for exactly this, and which is also what keeps this
/// store finite: a guardrail is released only by `DeleteAIGuardrail` or by the delete of
/// its assistant.
///
/// CHOSEN: AWS quotas AI guardrails per assistant but does not publish the figure.
pub const MAX_AI_GUARDRAILS_PER_ASSISTANT: usize = 100;

/// The sentence a create past [`MAX_AI_GUARDRAILS_PER_ASSISTANT`] is refused with,
/// stating the RULE — the ceiling and what to do about it — and carrying nothing from the
/// request or from what the assistant already holds, the way
/// [`message_quick_response_quota`] does for the other family with a per-parent ceiling.
pub fn message_ai_guardrail_quota() -> String {
    format!(
        "The maximum number of AI Guardrails ({MAX_AI_GUARDRAILS_PER_ASSISTANT}) in this \
         assistant has been reached. Delete an AI Guardrail before creating another."
    )
}

/// The most policy entries one AI guardrail carries across the five policy
/// configurations it is created with — denied topics, content filters, words and managed
/// word lists, PII entities and regular expressions, and contextual-grounding filters.
///
/// The model bounds some of those lists and leaves others open
/// (`smithy_query(query="shape",shape="AIGuardrailTopicPolicyConfig")` and its siblings
/// put `length min 1` on `topicsConfig`, `wordsConfig`, `piiEntitiesConfig`,
/// `regexesConfig` and the grounding `filtersConfig` with no maximum), so without this
/// ceiling one caller could store an arbitrarily long policy in this process's heap — the
/// same reason [`MAX_TAGS_PER_RESOURCE`] exists for the equally unbounded tag map.
///
/// It is a HEAP GUARD rather than a quota: it sits at twice the largest list AWS
/// documents for a guardrail policy (ten thousand custom words), so a policy AWS would
/// accept is never refused here, and the figure is this emulator's own.
pub const MAX_AI_GUARDRAIL_POLICY_ENTRIES: usize = 20_000;

/// The sentence a create whose policy configurations carry more than
/// [`MAX_AI_GUARDRAIL_POLICY_ENTRIES`] entries between them is refused with. It states
/// the RULE — the ceiling the resulting policy must stay within — and carries nothing
/// from the request.
pub fn message_too_many_policy_entries() -> String {
    format!(
        "The resultant AI Guardrail policy must not have more than \
         {MAX_AI_GUARDRAIL_POLICY_ENTRIES} entries."
    )
}

/// The most knowledge bases one account may hold at a time, with the same contract as
/// [`MAX_ASSISTANTS`]: a create beyond it is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402), which is also what keeps this store
/// finite.
///
/// CHOSEN: AWS quotas knowledge bases per account but does not publish the figure.
pub const MAX_KNOWLEDGE_BASES: usize = 100;

/// The most assistant associations one assistant may hold at a time. A create beyond it
/// is refused with the modelled `ServiceQuotaExceededException` (HTTP 402), which
/// `CreateAssistantAssociation` declares for exactly this, and which is also what keeps
/// this store finite: an association is released only by `DeleteAssistantAssociation`
/// or by the delete of its assistant.
///
/// CHOSEN: AWS quotas associations per assistant but does not publish the figure.
pub const MAX_ASSOCIATIONS_PER_ASSISTANT: usize = 100;

/// The `AssociationType` of an association with a knowledge base this account holds
/// (`smithy_query(query="shape",shape="AssociationType")`).
pub const ASSOCIATION_TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// The most quick responses one knowledge base may hold at a time, with the same
/// contract as [`MAX_KNOWLEDGE_BASES`] one level down: a create beyond it is refused
/// with the modelled `ServiceQuotaExceededException` (HTTP 402), which
/// `CreateQuickResponse` declares for exactly this, and which is also what keeps this
/// store finite — a quick response is released only by `DeleteQuickResponse` or by the
/// delete of its knowledge base.
///
/// CHOSEN: AWS quotas quick responses per knowledge base but does not publish the
/// figure.
pub const MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE: usize = 100;

/// The terminal `QuickResponseStatus` of a usable quick response, and the one a create
/// answers with: the service reports the quick response ready in the response to the
/// call that made it, so no later status change is owed
/// (`smithy_query(query="shape",shape="QuickResponseStatus")` also declares the
/// transient `CREATE_IN_PROGRESS`, `UPDATE_IN_PROGRESS`, `DELETE_IN_PROGRESS` and the
/// failure and deleted values, none of which a record this store holds reaches: a delete
/// removes the record outright).
pub const QUICK_RESPONSE_STATUS_CREATED: &str = "CREATED";

/// The `QuickResponseType` of a quick response written in plain text: the form the
/// content is read as when the caller marks it up as nothing else
/// (`smithy_query(query="doc",shape="CreateQuickResponseRequest",member="contentType")`:
/// "Use application/x.quickresponse;format=plain for a quick response written in plain
/// text").
pub const QUICK_RESPONSE_TYPE_PLAIN: &str = "application/x.quickresponse;format=plain";

/// The `QuickResponseType` of a quick response written in richtext — the same member's
/// other documented value ("Use application/x.quickresponse;format=markdown for a quick
/// response written in richtext"), which decides which `QuickResponseContents` members
/// carry the stored content.
pub const QUICK_RESPONSE_TYPE_MARKDOWN: &str = "application/x.quickresponse;format=markdown";

/// The media type a create that names NO `contentType` is stored with.
///
/// `contentType` is optional on `CreateQuickResponse` while
/// `QuickResponseData.contentType` is required, so the service decides the media type of
/// a quick response whose create named none: it is the richtext one, which is what the
/// recorded wire answer of this endpoint reports for such a create (corpus
/// `simparity-wire-aws-v1`, fixture `wisdom-quick-response-error-paths`, step `setup#3`,
/// a create carrying only a name, a content and a shortcut key). Richtext is also the
/// media type that carries the content in BOTH forms a client may read it as — see
/// [`QuickResponse::contents`].
pub const QUICK_RESPONSE_TYPE_BY_DEFAULT: &str = QUICK_RESPONSE_TYPE_MARKDOWN;

/// The contact channels a quick response whose create named NO `channels` applies to.
///
/// `channels` is optional on `CreateQuickResponse`, and a quick response the caller did
/// not scope to a channel applies to every contact channel this service supports —
/// "<p>The Connect Customer contact channels this quick response applies to. The
/// supported contact channel types include <code>Chat</code>.</p>"
/// (`smithy_query(query="shape",shape="QuickResponseData")`, member `channels`), so the
/// whole supported set is the one value below. The recorded wire answers of this endpoint
/// report exactly that for a create that named no channel (corpus
/// `simparity-wire-aws-v1`, fixtures `wisdom-quick-response-error-paths` and
/// `wisdom-quick-response-lifecycle`).
pub const QUICK_RESPONSE_CHANNEL_CHAT: &str = "Chat";

/// The `LanguageCode` a quick response whose create named NO `language` is written in:
/// the service's default language code, which the recorded wire answers of this endpoint
/// report for such a create (corpus `simparity-wire-aws-v1`, the `wisdom-quick-response-*`
/// fixtures). It is one of the codes the member documents
/// (`smithy_query(query="doc",shape="CreateQuickResponseRequest",member="language")`: "The
/// supported language codes include de_DE, en_US, es_ES, fr_FR, id_ID, it_IT, ja_JP,
/// ko_KR, pt_BR, zh_CN, zh_TW").
pub const QUICK_RESPONSE_LANGUAGE_BY_DEFAULT: &str = "en_US";

/// Whether a quick response whose create named no `isActive` is in service.
///
/// `isActive` is OPTIONAL on `CreateQuickResponse` and on `QuickResponseData`, but
/// REQUIRED on `QuickResponseSearchResultData`
/// (`smithy_query(query="shape",shape="QuickResponseSearchResultData")`), so a search hit
/// has to report a value for a record whose create named none. A quick response nothing
/// deactivated is usable the moment it is created — that is what the create answering it
/// already `CREATED` means — so the value reported, and the one a filter on `isActive`
/// matches against, is this one.
pub const QUICK_RESPONSE_ACTIVE_BY_DEFAULT: bool = true;

/// The message a read or a write of a quick response this knowledge base does not hold
/// is refused with, worded as this endpoint words the other not-found resources of this
/// family ([`MESSAGE_KNOWLEDGE_BASE_NOT_FOUND`]), with the resource token spelled as the
/// recorded wire answer of this endpoint spells it: one word, as the API itself names the
/// resource. The Amazon Connect Wisdom view of this same endpoint words it from its own
/// copy of this text, so a client cannot get a different sentence depending on which model
/// it addressed.
pub const MESSAGE_QUICK_RESPONSE_NOT_FOUND: &str = "QuickResponse does not exist.";

/// The sentence a create past [`MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE`] is refused
/// with, stating the RULE — the ceiling and what to do about it — and carrying nothing
/// from the request or from what the knowledge base already holds.
///
/// The Amazon Connect Wisdom view of this same endpoint words it from its own copy of
/// this text (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a
/// client cannot get a different sentence depending on which model it addressed.
pub fn message_quick_response_quota() -> String {
    format!(
        "The maximum number of quick responses ({MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE}) in \
         this knowledge base has been reached. Delete a quick response before creating another."
    )
}

/// The sentence a create whose `name` or `shortcutKey` a live quick response of the same
/// knowledge base already holds is refused with — one message class, worded with the
/// member whose value clashes.
///
/// A quick response's name and shortcut key each identify it inside its knowledge base
/// (the model states the shortcut key's rule on the member itself:
/// `smithy_query(query="doc",shape="CreateQuickResponseRequest",member="shortcutKey")`,
/// "The value should be unique across the knowledge base"), so a second create claiming
/// either conflicts with the first rather than replacing it.
///
/// The fixed text is the sentence the recorded wire answer of this endpoint carries for a
/// shortcut key already in use — which is why it is not [`MESSAGE_NAME_IN_USE`], the
/// shorter sentence the other resource families of this crate refuse a duplicate name
/// with — and the member is named in it as that answer names it: as PROSE ("shortcut
/// key"), not as the modelled member name (`shortcutKey`). The member name is the
/// sentence's only variable part, so no stored value travels in it.
pub fn message_value_in_use(member: &str) -> String {
    format!("The value for {member} is already in use. Please update and try again.")
}

/// The member names [`message_value_in_use`] is worded with, as a caller names them: the
/// request member whose value must be unique inside the knowledge base.
pub const MEMBER_QUICK_RESPONSE_NAME: &str = "name";
pub const MEMBER_SHORTCUT_KEY: &str = "shortcut key";

/// The sentence an update that both SETS a member and asks for that same member to be
/// REMOVED is refused with.
///
/// `UpdateQuickResponse` pairs three optional members with a `remove…` flag
/// (`smithy_query(query="operation",operation="UpdateQuickResponse")`:
/// `description`/`removeDescription`, `shortcutKey`/`removeShortcutKey`,
/// `groupingConfiguration`/`removeGroupingConfiguration`), and a request carrying both
/// halves of a pair asks for two incompatible outcomes on one member, so it is refused
/// with the modelled `ValidationException` rather than silently resolved one way. The
/// sentence states the rule and names the two members as the model declares them,
/// carrying no value from the request. The Amazon Connect Wisdom view of this same
/// endpoint words it from its own copy of this text
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client
/// cannot get a different sentence depending on which model it addressed.
pub fn message_conflicting_update_members(set_member: &str, remove_member: &str) -> String {
    format!("Cannot specify both {set_member} and {remove_member}.")
}

/// The only `KnowledgeBaseType` a content upload may be started against.
///
/// `StartContentUpload`'s own documentation states the rule — "You can only upload
/// content to a knowledge base of type CUSTOM" — so a start against a knowledge base
/// of any other modelled type is refused instead of being handed a URL that could not
/// be used.
pub const KNOWLEDGE_BASE_TYPE_CUSTOM: &str = "CUSTOM";

/// The only `KnowledgeBaseType` whose knowledge bases hold quick responses, and the
/// modelled type whose INTERNAL name is not its modelled value
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")`: `EXTERNAL`, `CUSTOM`,
/// `QUICK_RESPONSES`, `MESSAGE_TEMPLATES`, `MANAGED`).
///
/// Every quick-response operation documents the rule on its own `knowledgeBaseId`
/// (`smithy_query(query="doc",shape="SearchQuickResponsesRequest",member="knowledgeBaseId")`:
/// "This should be a QUICK_RESPONSES type knowledge base"), so a knowledge base of any
/// other modelled type exposes no quick-response plane at all
/// ([`message_quick_response_plane_unsupported`]).
///
/// The recorded wire answer of this endpoint for an upload against a knowledge base of
/// this type names the type `quick_replies` — the name it carried before the API spelled
/// it `QUICK_RESPONSES` — so that token travels from the recording rather than from the
/// modelled value. See [`knowledge_base_type_token`].
pub const KNOWLEDGE_BASE_TYPE_QUICK_RESPONSES: &str = "QUICK_RESPONSES";
const KNOWLEDGE_BASE_TYPE_TOKEN_QUICK_RESPONSES: &str = "quick_replies";

/// The name this service uses for a `KnowledgeBaseType` INTERNALLY, which is what its
/// refusals name a knowledge base's type with rather than the modelled enum value.
///
/// The internal name is the modelled value in lower case
/// (`QUICK_RESPONSES` -> [`KNOWLEDGE_BASE_TYPE_TOKEN_QUICK_RESPONSES`] being the one that
/// is not simply that, because the type was renamed in the API and not internally). A
/// value no modelled enumeration declares cannot reach a stored record — the boundary
/// refuses it before any handler runs — so this is total over the stored types. The legacy
/// Amazon Connect Wisdom view of this same endpoint words it from its own copy
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client
/// cannot get a different sentence depending on which model it addressed.
pub fn knowledge_base_type_token(knowledge_base_type: &str) -> String {
    if knowledge_base_type == KNOWLEDGE_BASE_TYPE_QUICK_RESPONSES {
        return KNOWLEDGE_BASE_TYPE_TOKEN_QUICK_RESPONSES.to_string();
    }
    knowledge_base_type.to_ascii_lowercase()
}

/// The sentence this endpoint refuses an upload against a knowledge base whose type
/// cannot take content with.
///
/// It is the fixed text the recorded wire answer of this endpoint carries for that class,
/// and its one variable part is the INTERNAL name of the type the request pointed at
/// ([`knowledge_base_type_token`]), read from the STORED knowledge base — the only thing a
/// caller learns from it is which type it addressed, which is what the refusal is for.
pub fn message_unsupported_content_type(knowledge_base_type: &str) -> String {
    format!(
        "Unsupported ContentType for KnowledgeBaseType: {}",
        knowledge_base_type_token(knowledge_base_type)
    )
}

/// The sentence every operation of the quick-response plane refuses a knowledge base that
/// holds no quick responses with — one whose `knowledgeBaseType` is not
/// [`KNOWLEDGE_BASE_TYPE_QUICK_RESPONSES`].
///
/// It is the fixed text the recorded wire answer of this endpoint carries for that class,
/// and its one variable part is the MODELLED type of the knowledge base the request named,
/// read from the STORED record: the only thing a caller learns from it is which type it
/// addressed, which is what the refusal is for. The legacy Amazon Connect Wisdom view of
/// this same endpoint words it from its own copy of this text
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client cannot
/// get a different sentence depending on which model it addressed.
pub fn message_quick_response_plane_unsupported(knowledge_base_type: &str) -> String {
    format!("QuickResponse APIs are not supported for {knowledge_base_type} knowledge bases")
}

/// The `KnowledgeBaseType` values whose content comes from a source OUTSIDE this service
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")` declares `EXTERNAL`, `CUSTOM`,
/// `QUICK_RESPONSES`, `MESSAGE_TEMPLATES` and `MANAGED`).
///
/// The model states which ones those are on the member that carries the source:
/// `sourceConfiguration` is "The source of the knowledge base content. Only set this
/// argument for EXTERNAL or Managed knowledge bases"
/// (`smithy_query(query="doc",shape="CreateKnowledgeBaseRequest",member="sourceConfiguration")`),
/// so a knowledge base of one of these types IS its source — created without one it would
/// describe no content at all. See [`message_source_configuration_required`].
pub const SOURCED_KNOWLEDGE_BASE_TYPES: [&str; 2] = ["EXTERNAL", "MANAGED"];

/// Whether a knowledge base of this `knowledgeBaseType` cannot exist without a
/// `sourceConfiguration` — see [`SOURCED_KNOWLEDGE_BASE_TYPES`].
pub fn requires_source_configuration(knowledge_base_type: &str) -> bool {
    SOURCED_KNOWLEDGE_BASE_TYPES.contains(&knowledge_base_type)
}

/// The sentence a create of a knowledge base whose type requires a source, carrying none,
/// is refused with — stating the type the request named, as the recorded wire answer of
/// this endpoint for that class does.
pub fn message_source_configuration_required(knowledge_base_type: &str) -> String {
    format!(
        "KnowledgeBase of type {knowledge_base_type} must have at least one \
         SourceConfiguration."
    )
}

/// How long an upload URL stays valid when the caller names no
/// `presignedUrlTimeToLive`.
///
/// CHOSEN, not documented: the model bounds that member at `range 1..60` minutes
/// (`smithy_query(query="operation",operation="StartContentUpload")`) and states no
/// default, so this emulator picks one inside those bounds. A caller that cares names
/// its own value, which is used as given.
pub const DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES: u64 = 30;

/// The most content uploads this store holds at once.
///
/// An upload is reclaimed when its URL expires or when its knowledge base is deleted,
/// so the store drains by itself; the ceiling bounds how much a caller that keeps
/// starting uploads can hold at any one instant. It is a resource guard this emulator
/// owns, NOT an AWS-published quota, and a start past it is refused with the modelled
/// `ValidationException` (the operation models no quota error).
pub const MAX_CONTENT_UPLOADS: usize = 1_000;

/// The most bytes one upload's `PUT` may carry.
///
/// The bytes live in this process's heap until the upload expires, so the body is
/// bounded the way the number of uploads is. It is a resource guard this emulator owns,
/// NOT an AWS-published quota, and a larger body is refused by the upload plane rather
/// than stored.
pub const MAX_CONTENT_UPLOAD_BYTES: usize = 8 * 1024 * 1024;

/// The most written bytes this store holds across EVERY live upload, so the
/// uploads x bytes-per-upload PRODUCT is finite and not just each factor: an upload's
/// body stays in this process's heap until the upload expires, and each one may carry up
/// to [`MAX_CONTENT_UPLOAD_BYTES`], so without this a caller that starts
/// [`MAX_CONTENT_UPLOADS`] uploads and writes a full-size body to each would hold the
/// product of the two. The counterpart of [`MAX_STORED_CONTENT_BYTES`] on the upload
/// side, and refused the same way a body past the per-body bound is.
pub const MAX_PENDING_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// The `ContentStatus` a `CreateContent` answers with: the service takes the uploaded
/// asset and ingests it, so the content is reported in progress by the very call that
/// started that work (`smithy_query(query="shape",shape="ContentStatus")` declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `ACTIVE`, `DELETE_IN_PROGRESS`,
/// `DELETE_FAILED`, `DELETED` and `UPDATE_FAILED`).
pub const CONTENT_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The terminal `ContentStatus` of an ingested content, and the one every read reports
/// once [`CONTENT_ACTIVATION_DELAY`] has passed.
pub const CONTENT_STATUS_ACTIVE: &str = "ACTIVE";

/// How long after the create a content's ingestion is due to finish.
///
/// CHOSEN, not documented: the create answers `CREATE_IN_PROGRESS`, so the move to
/// `ACTIVE` is a transition the CLOCK owes, and this is the span this emulator owes it
/// over. It is never zero — a caller that reads the content straight after creating it
/// sees the transient status the service reported — and it is short enough that nothing
/// has to wait on it for long.
pub const CONTENT_ACTIVATION_DELAY: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// The most contents one knowledge base holds at once.
///
/// A content stays until something deletes it or its knowledge base goes, so the
/// ceiling is the bound on that growth: it is a resource guard this emulator owns, NOT
/// an AWS-published quota, and a create past it is refused with the error the model
/// declares for exactly this on `CreateContent`, a 402 `ServiceQuotaExceededException`.
pub const MAX_CONTENTS_PER_KNOWLEDGE_BASE: usize = 1_000;

/// The most asset bytes this store holds across EVERY content, so the
/// contents x bytes-per-content PRODUCT is finite and not just each factor: a content's
/// asset lives in this process's heap until the content goes, and each one may carry up
/// to [`MAX_CONTENT_UPLOAD_BYTES`]. Refused the same way as
/// [`MAX_CONTENTS_PER_KNOWLEDGE_BASE`].
pub const MAX_STORED_CONTENT_BYTES: usize = 64 * 1024 * 1024;

/// The sentence a `CreateContent` past either content ceiling is refused with. It states
/// what the caller can DO about it and names no figure of the store, which is this
/// emulator's own rather than an AWS-published quota.
pub fn message_content_store_full() -> String {
    "This endpoint holds as many contents as its store keeps. Delete a content before \
     creating another."
        .to_string()
}

/// The fixed text for a request addressed to a content its knowledge base does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_CONTENT_NOT_FOUND: &str = "Content does not exist.";

/// The sentence a `NotifyRecommendationsReceived` entry carries for a
/// `recommendationId` the session's feed does not hold.
///
/// It is a PER-ENTRY error inside a successful answer, not a refusal: the call reports
/// which of the ids it was given it could not acknowledge, and names the id in the
/// entry's own `recommendationId` member
/// (`smithy_query(query="shape",shape="NotifyRecommendationsReceivedError")`:
/// `recommendationId` is "The identifier of the recommendation that is in error") rather
/// than in this sentence. The sentence itself is the ONE fixed service text for the
/// class — it does not name a reason and does not vary per entry — so every entry of
/// every answer carries exactly this string, and the Wisdom view of this same endpoint
/// words it from its own copy of this constant
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client
/// cannot read a different sentence depending on which model it addressed.
pub const MESSAGE_RECOMMENDATION_NOT_PROCESSED: &str = "Could not process recommendation";

/// The fixed text of the modelled `ValidationException` a `CreateContent` naming an
/// `uploadId` this service never issued (or one already consumed, or one whose URL has
/// expired) is refused with — the sentence the recorded answer of this endpoint carries
/// for that class.
pub const MESSAGE_INVALID_UPLOAD_ID: &str = "Invalid uploadId";

/// The fixed text the recorded answers of this service's TAGGING endpoints carry for a
/// `resourceArn` that names no resource of this account — the one sentence the whole
/// tagging surface words that class with, whichever of the three operations was called.
/// See [`crate::scaffolded::NOT_FOUND_CODE`] for why those answers' error code is not the
/// modelled shape name, and [`MESSAGE_ASSISTANT_NOT_FOUND`] for why the sentence carries
/// nothing from the request.
pub const MESSAGE_TAGGING_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The header an upload's `PUT` carries the `contentType` the upload was started for
/// under: an upload URL is minted for ONE media type, exactly as an S3 presigned `PUT`
/// signed over a content type is.
pub const CONTENT_TYPE_HEADER: &str = "content-type";

/// The header naming the server-side encryption an encrypted knowledge base's upload
/// must be written with, and its value: a knowledge base that carries a
/// `serverSideEncryptionConfiguration` is written under that KMS key, so the upload
/// must say so.
pub const SERVER_SIDE_ENCRYPTION_HEADER: &str = "x-amz-server-side-encryption";
pub const SERVER_SIDE_ENCRYPTION_AWS_KMS: &str = "aws:kms";

/// The header naming the customer-managed KMS key an encrypted knowledge base's upload
/// is written under, carried only when the knowledge base names one.
pub const SERVER_SIDE_ENCRYPTION_KEY_ID_HEADER: &str =
    "x-amz-server-side-encryption-aws-kms-key-id";

/// The endpoint prefix this service publishes
/// (`smithy_query(query="service_info")`: endpoint templates
/// `https://wisdom.{Region}.{dnsSuffix}`), which is what a client addresses this service
/// at and what its own `sdkId` key is derived from.
pub const ENDPOINT_PREFIX: &str = "wisdom";

/// The `AssociationType` of an association with a Bedrock knowledge base outside this
/// service (`smithy_query(query="shape",shape="AssociationType")`).
pub const ASSOCIATION_TYPE_EXTERNAL_BEDROCK_KNOWLEDGE_BASE: &str =
    "EXTERNAL_BEDROCK_KNOWLEDGE_BASE";

/// How many ids a mint tries before giving up, so no request can spin here. Each
/// candidate comes from the store's own sequence, so the first one is free unless the
/// store already holds that exact id.
const MINT_ATTEMPTS: usize = 8;

/// The ARN namespace (and SigV4 signing name) of this service: Amazon Q in Connect is
/// still `wisdom` on the wire.
pub const ARN_NAMESPACE: &str = "wisdom";

/// The region a request that carries no SigV4 credential scope is attributed to —
/// `emulator_arn`'s own documented default.
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of the identity a write of this endpoint is attributed to — what
/// `lastModifiedBy` (a `GenericArn`, "The Amazon Resource Name (ARN) of the user who last
/// updated the quick response data") carries.
///
/// The emulator publishes ONE long-term access key, and on AWS a long-term key belongs to
/// an IAM user of the account; that user is stated once for the whole tree
/// (`service_common::caller_principal`) and is the identity `sts:GetCallerIdentity`
/// answers with, so the principal a resource names as its author is the principal a client
/// reads as itself. It is a function of the account contract alone, so it is the same on
/// every call and across restarts, and it satisfies the member's own pattern
/// (`^arn:[a-z-]+?:[a-z-]+?:[a-z0-9-]*?:([0-9]{12})?:[a-zA-Z0-9-:/]+$`). The Amazon Connect
/// Wisdom view of this same endpoint attributes a write to the same identity
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs`), so a client cannot
/// read a different author depending on which model it addressed.
pub fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The region the caller signed this request for: an ARN names the region the request
/// was addressed to. Falls back to [`DEFAULT_REGION`] for an unsigned request, and for
/// a scope region the modelled `Arn` pattern (`[a-z0-9-]*?`) would not admit.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a resource this emulator minted lives in, read off its own stored ARN —
/// the fourth field of `arn:aws:wisdom:<region>:<account>:<resource>`.
///
/// This is what every `lastModifiedRegion` reports
/// (`smithy_query(query="doc",shape="AssistantData",member="lastModifiedRegion")`: "The
/// Amazon Web Services Region where the assistant was last modified"). The resources of
/// this service are regional — a resource is addressable only in the Region it was created
/// in, and that Region is the one its ARN names — so the Region in the stored ARN is the
/// Region every write of that record happened in. Reading it off the record keeps ONE
/// source of truth for a resource's Region rather than a second copy that could disagree
/// with the ARN beside it.
///
/// `None` for a value that is not an ARN this service minted, so a record carrying one
/// reports no Region rather than a field of some other string.
pub fn arn_region(arn: &str) -> Option<String> {
    let mut fields = arn.split(':');
    let minted_here = fields.next() == Some("arn")
        && fields.next() == Some("aws")
        && fields.next() == Some(ARN_NAMESPACE);
    if !minted_here {
        return None;
    }
    fields
        .next()
        .filter(|region| !region.is_empty())
        .map(str::to_string)
}

/// This service's ARN for an assistant:
/// `arn:aws:wisdom:<region>:<account>:assistant/<assistantId>`.
pub fn assistant_arn(region: &str, assistant_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("assistant/{assistant_id}"))
}

/// This service's ARN for a knowledge base:
/// `arn:aws:wisdom:<region>:<account>:knowledge-base/<knowledgeBaseId>`.
pub fn knowledge_base_arn(region: &str, knowledge_base_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("knowledge-base/{knowledge_base_id}"),
    )
}

/// This service's ARN for an assistant association, which names its parent assistant
/// too: `arn:aws:wisdom:<region>:<account>:association/<assistantId>/<assistantAssociationId>`.
///
/// Two id segments, which is the form the modelled `Arn` pattern admits for a nested
/// resource (`<label>/<uuid>(/<uuid>){0,2}`), and the form [`resolve_id`] reads the
/// resource's own id back out of.
pub fn assistant_association_arn(
    region: &str,
    assistant_id: &str,
    assistant_association_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("association/{assistant_id}/{assistant_association_id}"),
    )
}

/// This service's ARN for a session, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:session/<assistantId>/<sessionId>`.
pub fn session_arn(region: &str, assistant_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("session/{assistant_id}/{session_id}"),
    )
}

/// This service's ARN for a content, which names its parent knowledge base too:
/// `arn:aws:wisdom:<region>:<account>:content/<knowledgeBaseId>/<contentId>`.
///
/// The same two-id-segment form [`assistant_association_arn`] uses, for the same
/// reason: it is what the modelled `Arn` pattern admits for a nested resource, and what
/// [`resolve_id`] reads the content's own id back out of.
pub fn content_arn(region: &str, knowledge_base_id: &str, content_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("content/{knowledge_base_id}/{content_id}"),
    )
}

/// This service's ARN for a quick response, which names its parent knowledge base too:
/// `arn:aws:wisdom:<region>:<account>:quick-response/<knowledgeBaseId>/<quickResponseId>`.
///
/// Two id segments, the same nested form [`session_arn`] uses and the one the modelled
/// `Arn` pattern admits for a resource inside another (`<label>/<uuid>(/<uuid>)?`), with
/// the quick response's own id LAST so [`resolve_id`] reads it back out.
pub fn quick_response_arn(region: &str, knowledge_base_id: &str, quick_response_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("quick-response/{knowledge_base_id}/{quick_response_id}"),
    )
}

/// The version qualifier that names a resource's unversioned WORKING COPY rather than one
/// of its published versions.
///
/// The modelled label shapes admit a qualifier of `[A-Z0-9_$]+`
/// (`smithy_query(query="shape",shape="UuidOrArnOrEitherWithQualifier")`), and AWS
/// addresses the working copy of a versioned resource by this one — which is why a read
/// addressed with it answers with the resource itself and reports no `versionNumber`,
/// while a numbered qualifier names a version.
pub const VERSION_QUALIFIER_LATEST: &str = "$LATEST";

/// The id a binding to an AI agent names that agent by: the agent's own `Uuid` followed
/// by the `:<QUALIFIER>` of the revision of it that runs.
///
/// A binding targets `UuidWithQualifier`, whose pattern is a uuid plus an OPTIONAL
/// qualifier (`^[a-f0-9]{8}-…-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$`,
/// `smithy_query(query="shape",shape="UuidWithQualifier")`), and an AI agent type's
/// binding is documented as mapping the type "to AI Agent version"
/// (`smithy_query(query="shape",shape="SessionData")`, member `aiAgentConfiguration`):
/// what runs for the type is one revision of an agent, so the binding the service itself
/// made names the revision it bound rather than leaving a client unable to tell which one
/// it got. WHICH revision that is belongs to the agent, not to the assistant or session
/// reporting it — a floating alias where the service binds the agent itself, a number
/// where it pins a published version — so it is taken from the built-in agent's own
/// record ([`BuiltInAiAgentRevision`]) and never materialised here.
///
/// An id a CALLER configured is stored and reported exactly as given — the caller already
/// chose whether to pin a version — so this is used only where the SERVICE binds an agent
/// of its own.
pub fn qualified_ai_agent_id(ai_agent_id: &str, revision: BuiltInAiAgentRevision) -> String {
    format!("{ai_agent_id}:{}", revision.qualifier())
}

/// This service's ARN for an AI agent, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:ai-agent/<assistantId>/<aiAgentId>`.
pub fn ai_agent_arn(region: &str, assistant_id: &str, ai_agent_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("ai-agent/{assistant_id}/{ai_agent_id}"),
    )
}

/// This service's ARN for an AI guardrail, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:ai-guardrail/<assistantId>/<aiGuardrailId>`.
///
/// The same nested two-id-segment form [`ai_agent_arn`] uses, for the same reason: an AI
/// guardrail is reachable only through its assistant, and the guardrail's own id comes
/// LAST so [`resolve_id`] reads it back out.
pub fn ai_guardrail_arn(region: &str, assistant_id: &str, ai_guardrail_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("ai-guardrail/{assistant_id}/{ai_guardrail_id}"),
    )
}

/// The ARN a resource of a VERSIONED family reports when it is addressed through a label
/// that may carry a version qualifier: the resource's own ARN with the qualifier of the
/// version addressed appended, `<resourceArn>:<QUALIFIER>`.
///
/// The model's `UuidOrArnOrEitherWithQualifier` labels name either a resource or one
/// VERSION of it (`smithy_query(query="shape",shape="UuidOrArnOrEitherWithQualifier")`),
/// and a request that names no version addresses the working copy, whose qualifier is the
/// floating [`VERSION_QUALIFIER_LATEST`] alias — so an operation reached through such a
/// label reports which version it answered for, rather than the bare resource path that
/// names no version at all. An operation whose label carries no qualifier at all (a
/// create, a list) reports the bare ARN, because it addressed no version.
pub fn version_qualified_arn(resource_arn: &str, qualifier: &str) -> String {
    format!("{resource_arn}:{qualifier}")
}

/// The built-in AI agents an assistant is provisioned with, and the bindings that name
/// them: one `SYSTEM`-origin record per [`BUILT_IN_AI_AGENTS`] entry, under that entry's
/// service-published id and version.
///
/// The ids are the SERVICE's, so every assistant holds the same eight agents and every
/// binding a fresh assistant or session reports names one of them: nothing here is
/// minted per assistant. Returned rather than stored, so the create puts them on the
/// record it inserts.
pub fn default_ai_agents(
    region: &str,
    assistant_id: &str,
) -> (
    HashMap<String, AiAgent>,
    HashMap<String, AIAgentConfigurationData>,
) {
    let created_at = SystemTime::now();
    let mut agents: HashMap<String, AiAgent> = HashMap::new();
    let mut bindings: HashMap<String, AIAgentConfigurationData> = HashMap::new();
    for agent in BUILT_IN_AI_AGENTS {
        let Some(configuration) = ai_agent_configuration_for_type(agent.r#type) else {
            // Unreachable: every built-in's type is one of the modelled `AIAgentType`
            // values, each of which the union declares a configuration member for.
            continue;
        };
        agents.insert(
            agent.ai_agent_id.to_string(),
            AiAgent {
                ai_agent_arn: ai_agent_arn(region, assistant_id, agent.ai_agent_id),
                ai_agent_id: agent.ai_agent_id.to_string(),
                assistant_id: assistant_id.to_string(),
                assistant_arn: assistant_arn(region, assistant_id),
                name: agent.r#type.to_string(),
                r#type: agent.r#type.to_string(),
                configuration,
                // Nothing a caller sent is on a built-in agent: no caller created it.
                description: None,
                tags: HashMap::new(),
                origin: AI_AGENT_ORIGIN_SYSTEM.to_string(),
                visibility_status: AI_AGENT_VISIBILITY_PUBLISHED.to_string(),
                status: AI_RESOURCE_STATUS_ACTIVE.to_string(),
                // The revision the service's binding names: a numbered version where it
                // pins one, and none at all where the binding names the agent's floating
                // alias — see [`BuiltInAiAgentRevision::published_version`].
                published_version: agent.revision.published_version(),
                created_at,
                // Nothing has modified the record yet, so it was last written when the
                // assistant it belongs to was created.
                modified_at: created_at,
            },
        );
        bindings.insert(
            agent.r#type.to_string(),
            // The binding a fresh assistant reports for the type, built where every
            // default binding is built — see [`default_ai_agent_binding`].
            built_in_ai_agent_binding(agent),
        );
    }
    (agents, bindings)
}

/// The service-managed session an assistant is provisioned with, as this emulator
/// persists it: the session [`SYSTEM_SESSION_ID`] names, of `SYSTEM` origin, carrying
/// the fixed name and description the service gives it and the assistant's own AI agent
/// bindings.
///
/// Nothing on it comes from a caller — no caller created it — so it holds no tags, no tag
/// filter, no contact, no topic integration and no orchestrator configuration, and
/// reports only the members it holds.
pub fn system_session(
    region: &str,
    assistant_id: &str,
    ai_agent_configuration: &HashMap<String, AIAgentConfigurationData>,
) -> Session {
    Session {
        session_id: SYSTEM_SESSION_ID.to_string(),
        session_arn: session_arn(region, assistant_id, SYSTEM_SESSION_ID),
        assistant_id: assistant_id.to_string(),
        name: SYSTEM_SESSION_NAME.to_string(),
        description: Some(SYSTEM_SESSION_DESCRIPTION.to_string()),
        tags: HashMap::new(),
        tag_filter: None,
        ai_agent_configuration: ai_agent_configuration.clone(),
        topic_integration_arn: None,
        orchestrator_configuration_list: None,
        contact_arn: None,
        origin: SESSION_ORIGIN_SYSTEM.to_string(),
        created_at: SystemTime::now(),
        recommendations: RecommendationFeed::default(),
    }
}

/// One id candidate for the mint block the store's sequence number `seq` owns.
///
/// The form is the modelled `Uuid` shape
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). UNIQUENESS comes
/// from the sequence, which no two mints share — two resources created in the same
/// nanosecond still get different ids; the digest only spreads that counter (and the
/// wall clock, so two runs of the same process do not repeat ids) over the 32 hex digits
/// the pattern wants, so an id carries no readable structure and no sequence to guess.
fn mint_candidate(seq: u64, attempt: u64) -> String {
    use ::sha2::Digest;

    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let block = seq
        .wrapping_mul(MINT_ATTEMPTS as u64)
        .wrapping_add(attempt);
    let digest = ::sha2::Sha256::digest(format!("{ARN_NAMESPACE}:{block}:{nanos}").as_bytes());
    let hex = ::hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The bare UUID a `UuidOrArn` label names: the model's pattern admits either the id or
/// the resource's full ARN, and every ARN this service mints ends in the resource's own
/// UUID, so the trailing path segment is the id.
///
/// An ARN this emulator could not have minted — another partition, service or account —
/// resolves to NOTHING: the label comes back unchanged, so the lookup misses and the
/// caller gets the operation's not-found error instead of a resource the ARN does not
/// name.
pub fn resolve_id(label: &str) -> &str {
    let Some((prefix, id)) = label.rsplit_once('/') else {
        // No path segment at all: a bare UUID.
        return label;
    };
    let mut fields = prefix.split(':');
    let minted_here = fields.next() == Some("arn")
        && fields.next() == Some("aws")
        && fields.next() == Some(ARN_NAMESPACE)
        && fields.next().is_some_and(|region| !region.is_empty())
        && fields.next() == Some(::engine::emulator_account_id())
        // The resource segment, plus any parent id a nested resource's ARN carries.
        && fields.next().is_some();
    if minted_here {
        id
    } else {
        label
    }
}

/// The id and the VERSION QUALIFIER a label of the model's
/// `UuidOrArnOrEitherWithQualifier` shape carries
/// (`smithy_query(query="shape",shape="UuidOrArnOrEitherWithQualifier")`: a uuid or an
/// ARN, each optionally followed by `:<QUALIFIER>`).
///
/// The id is resolved exactly as [`resolve_id`] resolves a plain label, so an ARN this
/// emulator could not have minted still names nothing. The qualifier is what the model
/// says selects a VERSION of the resource rather than the resource itself — the
/// `versionNumber` of a read "is returned if a version was specified via use of a
/// qualifier" — so an operation that reads one has to tell the two apart.
pub fn resolve_qualified_id(label: &str) -> (&str, Option<&str>) {
    let id = resolve_id(label);
    match id.split_once(':') {
        Some((id, qualifier)) => (id, Some(qualifier)),
        None => (id, None),
    }
}

/// Whether `value` has the form the model's `Uuid` shape declares
/// (`smithy_query(query="shape",shape="Uuid")`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) — five lowercase-hex
/// groups of 8, 4, 4, 4 and 12 digits. The id-FORM question alone, so the decision that
/// reads it states only what it decides.
fn is_uuid_shaped(value: &str) -> bool {
    /// The group lengths of the modelled `Uuid` form, in order.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut groups = value.split('-');
    GROUPS.iter().all(|expected| {
        groups.next().is_some_and(|group| {
            group.len() == *expected
                && group
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }) && groups.next().is_none()
}

/// Whether `label` names a resource IDENTITY in this account at all: the id it resolves
/// to ([`resolve_id`]) has the form the model gives this service's ids
/// ([`is_uuid_shaped`]).
///
/// This is a question about the LABEL, not about the store: a well-formed id that names
/// nothing is still an identity of this account, and gets the operation's not-found
/// answer once it is resolved. A label that is not one — a bare string that is no id, or
/// an ARN of another partition, account or service, which [`resolve_id`] hands back
/// unchanged — names no resource any permission of this account could cover, which is
/// what an operation that decides authorization before it resolves anything keys on.
pub fn names_an_account_resource(label: &str) -> bool {
    is_uuid_shaped(resolve_id(label))
}

/// A non-tag map field rendered as the modelled map shape, or `None` when it is empty:
/// a resource with nothing to report under it reports no member rather than an empty
/// object. The `tags` member follows the opposite rule — see [`tags_out`], which is what
/// the recorded responses show for it.
fn map_out<V: Clone>(map: &HashMap<String, V>) -> Option<HashMap<String, V>> {
    (!map.is_empty()).then(|| map.clone())
}

/// The `tags` member of a resource this service reports: ALWAYS present, and an empty
/// object when the resource carries no tag.
///
/// This is what the service answers on the wire — a recorded create of an untagged
/// resource carries the `tags` member all the same — so a client reading
/// `response["tags"]` finds a map for every resource rather than having to tell an
/// untagged resource's absent member from a tagged one's map.
///
/// It is the ONE owner of that rule in this crate: every operation that reports a `tags`
/// member — a resource read, and `ListTagsForResource`, whose whole output is that
/// member — renders it through here rather than restating the rule.
pub fn tags_out(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// The recommendations one session has available, oldest first, with the triggers that
/// produced them.
///
/// `GetRecommendations` REPORTS this feed — the operation is `@readonly` — so a read
/// never consumes an entry: two reads of an unchanged feed answer alike, and only an
/// acknowledgement removes one. Nothing in this model's implemented surface publishes a
/// recommendation (the service makes one available out of band, from the contact's
/// detected intent, and `QueryAssistant`/`SendMessage` are still scaffold stubs with
/// their own issues), so a session's feed is empty until something calls
/// [`RecommendationFeed::publish`] — which is exactly the "no recommendations available"
/// case the operation documents: the call returns successfully with an empty list.
#[derive(Debug, Clone, Default)]
pub struct RecommendationFeed {
    /// The available recommendations, oldest first: the order `GetRecommendations`
    /// serves a page in, and the order the cap evicts in.
    pub recommendations: VecDeque<RecommendationData>,
    /// The triggers that produced them, each naming the `recommendationId`s it produced
    /// (`RecommendationTrigger.recommendationIds`). A trigger is kept only while the
    /// feed still holds a recommendation it names, so a trigger that names none is not
    /// kept and not reported.
    pub triggers: Vec<RecommendationTrigger>,
}

/// The most recommendations one session's feed holds at once, and the most triggers kept
/// beside them.
///
/// A recommendation stays available until something acknowledges it
/// ([`RecommendationFeed::acknowledge`], which `NotifyRecommendationsReceived` is), so an
/// unbounded feed would let a session nobody acknowledges grow this process's heap
/// without limit: the cap is the bound on that growth. It is a resource guard this
/// emulator owns, NOT an AWS-published quota, and it equals the largest page
/// `GetRecommendations` can serve ([`DEFAULT_MAX_RESULTS`], the model's `maxResults`
/// ceiling), so a full feed can still fill any page the model allows. Publishing past it
/// forgets the OLDEST entry, the way a live feed drops what nobody collected rather than
/// refusing new material.
pub const MAX_PENDING_RECOMMENDATIONS_PER_SESSION: usize = DEFAULT_MAX_RESULTS;

impl RecommendationFeed {
    /// Make one recommendation available on this session, with the trigger that produced
    /// it (if any).
    ///
    /// Bounded by [`MAX_PENDING_RECOMMENDATIONS_PER_SESSION`]: the oldest recommendation
    /// is forgotten first, and a trigger no held recommendation names is forgotten with
    /// it.
    pub fn publish(
        &mut self,
        recommendation: RecommendationData,
        trigger: Option<RecommendationTrigger>,
    ) {
        self.recommendations.push_back(recommendation);
        if let Some(trigger) = trigger {
            self.triggers.push(trigger);
        }
        while self.recommendations.len() > MAX_PENDING_RECOMMENDATIONS_PER_SESSION {
            self.recommendations.pop_front();
        }
        let held: HashSet<String> = self
            .recommendations
            .iter()
            .map(|recommendation| recommendation.recommendation_id.clone())
            .collect();
        self.triggers
            .retain(|trigger| trigger.recommendation_ids.iter().any(|id| held.contains(id)));
        while self.triggers.len() > MAX_PENDING_RECOMMENDATIONS_PER_SESSION {
            self.triggers.remove(0);
        }
    }

    /// Acknowledge receipt of `recommendation_ids`: every id this feed holds is REMOVED
    /// from it — which is what the acknowledgement does, in the operation's own words
    /// ("Removes the specified recommendations from the specified assistant's queue of
    /// newly available recommendations") — and answered in the first list, while an id
    /// the feed does not hold is answered in the second.
    ///
    /// A trigger no held recommendation names any more is forgotten with them, the way
    /// [`Self::publish`] drops one. An id named twice in one call is acknowledged once:
    /// the second mention names a recommendation the feed no longer holds.
    pub fn acknowledge(&mut self, recommendation_ids: &[String]) -> (Vec<String>, Vec<String>) {
        let mut received = Vec::new();
        let mut unknown = Vec::new();
        for id in recommendation_ids {
            let held = self
                .recommendations
                .iter()
                .any(|recommendation| recommendation.recommendation_id == *id);
            if held {
                self.recommendations
                    .retain(|recommendation| recommendation.recommendation_id != *id);
                received.push(id.clone());
            } else {
                unknown.push(id.clone());
            }
        }
        let still_held: HashSet<String> = self
            .recommendations
            .iter()
            .map(|recommendation| recommendation.recommendation_id.clone())
            .collect();
        self.triggers.retain(|trigger| {
            trigger
                .recommendation_ids
                .iter()
                .any(|id| still_held.contains(id))
        });
        (received, unknown)
    }

    /// The page of at most `max_results` available recommendations, oldest first, and the
    /// triggers that produced the recommendations IN that page.    ///
    /// `recommendation_type` is the modelled `recommendationType` filter — "the type of
    /// recommendation being requested" — so a call that names one is served only the
    /// recommendations of that `RecommendationType`.
    pub fn page(
        &self,
        max_results: usize,
        recommendation_type: Option<&str>,
    ) -> (Vec<RecommendationData>, Vec<RecommendationTrigger>) {
        let recommendations: Vec<RecommendationData> = self
            .recommendations
            .iter()
            .filter(|recommendation| {
                recommendation_type.is_none_or(|wanted| {
                    recommendation.r#type.as_deref() == Some(wanted)
                })
            })
            .take(max_results)
            .cloned()
            .collect();
        let paged: HashSet<&str> = recommendations
            .iter()
            .map(|recommendation| recommendation.recommendation_id.as_str())
            .collect();
        let triggers: Vec<RecommendationTrigger> = self
            .triggers
            .iter()
            .filter(|trigger| {
                trigger
                    .recommendation_ids
                    .iter()
                    .any(|id| paged.contains(id.as_str()))
            })
            .cloned()
            .collect();
        (recommendations, triggers)
    }
}

/// One session of an assistant, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub session_arn: String,
    pub assistant_id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub tag_filter: Option<TagFilter>,
    /// The AI agent bound per `AIAgentType` for this session: the parent assistant's
    /// bindings, overlaid per type by whatever the create configured for the session
    /// itself. Stored on the session, so every read of it reports the same bindings.
    pub ai_agent_configuration: HashMap<String, AIAgentConfigurationData>,
    /// The ARN of the SNS topic this session streams chat messages through: the topic the
    /// parent assistant was provisioned with (see
    /// [`provision_assistant_streaming_topic`]), which is what a session of that assistant
    /// is integrated with. `None` on a session that is integrated with no topic, which
    /// reports no integration configuration at all.
    pub topic_integration_arn: Option<String>,
    /// This session's orchestrator configurations, as its create stored them: `Some` of
    /// the list a create was given — empty when it named none — and `None` on a session
    /// no create configured, which reports no orchestrator list at all.
    pub orchestrator_configuration_list: Option<Vec<OrchestratorConfigurationEntry>>,
    /// The contact this session was opened for, as the caller gave it. Not a member of
    /// `SessionData`, so it is persisted but not reported.
    pub contact_arn: Option<String>,
    pub origin: String,
    pub created_at: SystemTime,
    /// The recommendations available on this session, which `GetRecommendations`
    /// reports. It lives ON the session, so the delete of the owning assistant reclaims
    /// it with the session rather than leaving an orphan feed behind in a map beside the
    /// sessions.
    pub recommendations: RecommendationFeed,
}

impl Session {
    /// The `SessionData` every operation that reports this session answers with.
    pub fn to_session_data(&self) -> SessionData {
        SessionData {
            session_arn: self.session_arn.clone(),
            session_id: self.session_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            // Every configuration member of the stored record is reported, each out of
            // what the record holds — the same rule the assistant is rendered by: the
            // topic the session streams through, the AI agent bound per `AIAgentType`,
            // and the orchestrator configurations. A member the session HOLDS is
            // reported even when it is empty (a create that named no orchestrator
            // configuration reports an empty list, not nothing); a member it holds
            // nothing under at all is omitted.
            integration_configuration: self.topic_integration_arn.as_ref().map(|topic| {
                SessionIntegrationConfiguration {
                    topic_integration_arn: Some(topic.clone()),
                }
            }),
            tag_filter: self.tag_filter.clone(),
            ai_agent_configuration: map_out(&self.ai_agent_configuration),
            origin: Some(self.origin.clone()),
            orchestrator_configuration_list: self.orchestrator_configuration_list.clone(),
        }
    }
}

/// One AI agent of an assistant, as this emulator persists it.
///
/// The built-in agents the service provisions with the assistant are `SYSTEM`-origin
/// records under the service's own ids (see [`BUILT_IN_AI_AGENTS`]); an agent a caller
/// creates through `CreateAIAgent` is `CUSTOMER`-origin and carries the configuration the
/// caller gave. Every member a create carried is kept here, so what the create answered is
/// what every later read answers, and the assistant's `aiAgentConfiguration` names these
/// records rather than ids with nothing behind them.
#[derive(Debug, Clone)]
pub struct AiAgent {
    pub ai_agent_id: String,
    pub ai_agent_arn: String,
    pub assistant_id: String,
    /// The ARN of the assistant this agent belongs to, read off that stored record rather
    /// than rebuilt, so the agent reports its parent's own ARN.
    pub assistant_arn: String,
    pub name: String,
    pub r#type: String,
    /// The configuration this agent runs with: the union member its `type` is configured
    /// through — see [`ai_agent_configuration_type`].
    pub configuration: AIAgentConfiguration,
    pub description: Option<String>,
    pub origin: String,
    pub visibility_status: String,
    pub status: String,
    pub tags: HashMap<String, String>,
    /// The version of this agent that is published, and so the version an assistant's or
    /// session's binding to it names — see [`qualified_ai_agent_id`]. A built-in agent
    /// carries the version the SERVICE pins its binding to, and none where that binding
    /// names the agent's floating alias instead; an agent a caller created has none until
    /// `CreateAIAgentVersion` publishes one.
    pub published_version: Option<i64>,
    pub created_at: SystemTime,
    /// When this record was last written: the create that minted it, until something
    /// modifies it. It is what the reported `modifiedTime` renders.
    pub modified_at: SystemTime,
}

impl AiAgent {
    /// The `AIAgentData` every operation that reports this agent answers with, rendered
    /// entirely out of the stored record.
    pub fn to_ai_agent_data(&self) -> AIAgentData {
        AIAgentData {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            ai_agent_arn: self.ai_agent_arn.clone(),
            ai_agent_id: self.ai_agent_id.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            configuration: self.configuration.clone(),
            description: self.description.clone(),
            visibility_status: self.visibility_status.clone(),
            tags: tags_out(&self.tags),
            origin: Some(self.origin.clone()),
            status: Some(self.status.clone()),
            modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            // The same instant `modifiedTime` renders and the Region the record's own ARN
            // names: both members report when and where this agent was last modified (see
            // [`arn_region`]).
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.ai_agent_arn),
        }
    }

    /// The same `AIAgentData` as [`Self::to_ai_agent_data`], for an operation that
    /// addressed this agent through a label that may carry a VERSION qualifier: every
    /// member comes out of the stored record, and the reported ARN names the version the
    /// request addressed ([`version_qualified_arn`]).
    pub fn to_ai_agent_data_at_version(&self, qualifier: &str) -> AIAgentData {
        AIAgentData {
            ai_agent_arn: version_qualified_arn(&self.ai_agent_arn, qualifier),
            ..self.to_ai_agent_data()
        }
    }

    /// The `AIAgentSummary` every operation that SUMMARISES this agent answers with,
    /// rendered out of the same stored record [`Self::to_ai_agent_data`] renders: a summary
    /// reports what a read of the agent it summarises reports.
    pub fn to_ai_agent_summary(&self) -> AIAgentSummary {
        AIAgentSummary {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            ai_agent_arn: self.ai_agent_arn.clone(),
            ai_agent_id: self.ai_agent_id.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            configuration: Some(self.configuration.clone()),
            description: self.description.clone(),
            visibility_status: self.visibility_status.clone(),
            tags: tags_out(&self.tags),
            origin: Some(self.origin.clone()),
            status: Some(self.status.clone()),
            modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            // As on the data shape: the instant the record was last written, and the
            // Region its ARN names.
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.ai_agent_arn),
        }
    }

    /// The same `AIAgentSummary` as [`Self::to_ai_agent_summary`], for a summary of one
    /// VERSION of this agent: the reported ARN names the version summarised
    /// ([`version_qualified_arn`]), the way a read of that version reports it.
    pub fn to_ai_agent_summary_at_version(&self, qualifier: &str) -> AIAgentSummary {
        AIAgentSummary {
            ai_agent_arn: version_qualified_arn(&self.ai_agent_arn, qualifier),
            ..self.to_ai_agent_summary()
        }
    }
}

/// One Amazon Q in Connect AI guardrail, as this emulator persists it.
///
/// An AI guardrail is created inside an assistant and is reachable only through it, as a
/// session and an AI agent are, so the record lives on [`Assistant`] rather than in a
/// store-wide map: an id of one assistant's guardrail does not resolve under another.
/// Every member a create carried is kept here, so what the create answered is what every
/// later read answers.
#[derive(Debug, Clone)]
pub struct AiGuardrail {
    pub ai_guardrail_id: String,
    pub ai_guardrail_arn: String,
    pub assistant_id: String,
    /// The ARN of the assistant this guardrail belongs to, read off that stored record
    /// rather than rebuilt, so the guardrail reports its parent's own ARN.
    pub assistant_arn: String,
    pub name: String,
    pub visibility_status: String,
    pub blocked_input_messaging: String,
    pub blocked_outputs_messaging: String,
    pub description: Option<String>,
    pub topic_policy_config: Option<AIGuardrailTopicPolicyConfig>,
    pub content_policy_config: Option<AIGuardrailContentPolicyConfig>,
    pub word_policy_config: Option<AIGuardrailWordPolicyConfig>,
    pub sensitive_information_policy_config: Option<AIGuardrailSensitiveInformationPolicyConfig>,
    pub contextual_grounding_policy_config: Option<AIGuardrailContextualGroundingPolicyConfig>,
    pub tags: HashMap<String, String>,
    /// The `Status` this guardrail reports. The create answers with the guardrail already
    /// in service — see [`AI_RESOURCE_STATUS_ACTIVE`] — so it is stored once and reported
    /// unchanged.
    pub status: String,
    /// When this record was last written: the create that minted it, until something
    /// modifies it. It is what the reported `modifiedTime` renders.
    pub modified_at: SystemTime,
}

impl AiGuardrail {
    /// How many entries this guardrail's policy configurations carry between them, which
    /// is what [`MAX_AI_GUARDRAIL_POLICY_ENTRIES`] bounds: every list of every policy,
    /// counted once, since it is the whole stored policy the ceiling is about.
    pub fn policy_entries(&self) -> usize {
        let topics = self
            .topic_policy_config
            .as_ref()
            .map_or(0, |policy| policy.topics_config.len());
        let filters = self
            .content_policy_config
            .as_ref()
            .map_or(0, |policy| policy.filters_config.len());
        let words = self.word_policy_config.as_ref().map_or(0, |policy| {
            policy.words_config.as_ref().map_or(0, Vec::len)
                + policy.managed_word_lists_config.as_ref().map_or(0, Vec::len)
        });
        let sensitive = self
            .sensitive_information_policy_config
            .as_ref()
            .map_or(0, |policy| {
                policy.pii_entities_config.as_ref().map_or(0, Vec::len)
                    + policy.regexes_config.as_ref().map_or(0, Vec::len)
            });
        let grounding = self
            .contextual_grounding_policy_config
            .as_ref()
            .map_or(0, |policy| policy.filters_config.len());
        topics
            .saturating_add(filters)
            .saturating_add(words)
            .saturating_add(sensitive)
            .saturating_add(grounding)
    }

    /// The `AIGuardrailData` every operation that reports this guardrail answers with,
    /// rendered entirely out of the stored record.
    pub fn to_ai_guardrail_data(&self) -> AIGuardrailData {
        AIGuardrailData {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            ai_guardrail_arn: self.ai_guardrail_arn.clone(),
            ai_guardrail_id: self.ai_guardrail_id.clone(),
            name: self.name.clone(),
            visibility_status: self.visibility_status.clone(),
            blocked_input_messaging: self.blocked_input_messaging.clone(),
            blocked_outputs_messaging: self.blocked_outputs_messaging.clone(),
            description: self.description.clone(),
            // Each policy configuration is reported exactly as the create persisted it,
            // and a guardrail configured with none reports none rather than an empty
            // structure the caller never sent.
            topic_policy_config: self.topic_policy_config.clone(),
            content_policy_config: self.content_policy_config.clone(),
            word_policy_config: self.word_policy_config.clone(),
            sensitive_information_policy_config: self
                .sensitive_information_policy_config
                .clone(),
            contextual_grounding_policy_config: self.contextual_grounding_policy_config.clone(),
            tags: tags_out(&self.tags),
            status: Some(self.status.clone()),
            modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            // The same instant `modifiedTime` renders and the Region the record's own ARN
            // names: both members report when and where this guardrail was last modified
            // (see [`arn_region`]).
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.ai_guardrail_arn),
        }
    }

    /// The same `AIGuardrailData` as [`Self::to_ai_guardrail_data`], for an operation that
    /// addressed this guardrail through a label that may carry a VERSION qualifier: every
    /// member comes out of the stored record, and the reported ARN names the version the
    /// request addressed ([`version_qualified_arn`]).
    pub fn to_ai_guardrail_data_at_version(&self, qualifier: &str) -> AIGuardrailData {
        AIGuardrailData {
            ai_guardrail_arn: version_qualified_arn(&self.ai_guardrail_arn, qualifier),
            ..self.to_ai_guardrail_data()
        }
    }
}

/// One Amazon Q in Connect assistant, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Assistant {
    pub assistant_id: String,
    pub assistant_arn: String,
    pub name: String,
    pub r#type: String,
    pub status: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    pub created_at: SystemTime,
    /// When this assistant's own members were last written: the create that minted it,
    /// until an operation that writes the record — an AI-agent binding, a replica key —
    /// re-stamps it. It is what the reported `lastModifiedTime` renders, so that member is
    /// never earlier than the create.
    pub modified_at: SystemTime,
    /// The `AssistantCapabilityType` this assistant was provisioned with. Chosen by the
    /// service — `CreateAssistant` models no capability member — so it is stored once
    /// and reported unchanged.
    pub capability_type: String,
    /// The ARN of the SNS topic this assistant streams chat messages through, as Amazon SNS
    /// — which owns topics — answered when the create provisioned it; see
    /// [`provision_assistant_streaming_topic`]. Stored, so every read of the assistant
    /// reports the same topic. `None` when the assistant has no topic provisioned.
    pub topic_integration_arn: Option<String>,
    /// The AI agent bound to this assistant per `AIAgentType`, which is what
    /// `UpdateAssistantAIAgent`/`RemoveAssistantAIAgent` write. Every entry names an
    /// agent in [`Self::ai_agents`].
    pub ai_agent_configuration: HashMap<String, AIAgentConfigurationData>,
    /// The AI agents this assistant holds, keyed by `aiAgentId`: the default `SYSTEM`
    /// ones the create provisions, and any a caller adds later. An AI agent is
    /// reachable only through its assistant, as a session is.
    pub ai_agents: HashMap<String, AiAgent>,
    /// What every AI agent of this assistant has PUBLISHED, keyed by `aiAgentId`, at most
    /// [`MAX_AI_AGENT_VERSIONS_PER_AGENT`] versions per agent. A version is a snapshot of
    /// the agent in [`Self::ai_agents`] that published it, so it goes when that agent goes
    /// — and when this assistant goes.
    pub ai_agent_versions: AiAgentVersions,
    /// The AI guardrails this assistant holds, keyed by `aiGuardrailId`, at most
    /// [`MAX_AI_GUARDRAILS_PER_ASSISTANT`] of them. A guardrail is reachable only through
    /// its assistant, as an AI agent and a session are, so it goes when the assistant
    /// goes.
    pub ai_guardrails: HashMap<String, AiGuardrail>,
    /// The AI prompts this assistant holds, keyed by `aiPromptId`, at most
    /// [`MAX_AI_PROMPTS_PER_ASSISTANT`] of them, on the same footing as
    /// [`Self::ai_guardrails`]: a prompt is reachable only through its assistant, so it
    /// goes when the assistant goes.
    pub ai_prompts: HashMap<String, AiPrompt>,
    /// What every AI prompt of this assistant has PUBLISHED, keyed by `aiPromptId`, at
    /// most [`MAX_AI_PROMPT_VERSIONS_PER_PROMPT`] versions per prompt, on the same footing
    /// as [`Self::ai_agent_versions`]: a version is a snapshot of the prompt in
    /// [`Self::ai_prompts`] that published it, so it goes when that prompt goes — and when
    /// this assistant goes.
    pub ai_prompt_versions: AiPromptVersions,
    /// This assistant's orchestrator configurations, with the same contract as
    /// [`Self::ai_agent_configuration`].
    pub orchestrator_configuration_list: Vec<OrchestratorConfigurationEntry>,
    /// This assistant's sessions, keyed by `sessionId`: the service-managed one the
    /// create provisions it with (see [`system_session`]) and every session a caller
    /// creates. A session is reachable only through its parent, so a session id does not
    /// resolve under another assistant.
    pub sessions: HashMap<String, Session>,
}

impl Assistant {
    /// The assistant's integration configuration, as every operation that reports this
    /// assistant renders it: the member is always present, and carries a
    /// `topicIntegrationArn` only once a topic is integrated with the assistant.
    fn integration_configuration(&self) -> Option<AssistantIntegrationConfiguration> {
        Some(AssistantIntegrationConfiguration {
            topic_integration_arn: self.topic_integration_arn.clone(),
        })
    }

    /// The assistant's capability configuration, rendered from the stored capability.
    fn capability_configuration(&self) -> Option<AssistantCapabilityConfiguration> {
        Some(AssistantCapabilityConfiguration {
            r#type: Some(self.capability_type.clone()),
        })
    }

    /// The `AssistantData` every operation that reports this assistant answers with.
    pub fn to_assistant_data(&self) -> AssistantData {
        AssistantData {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // Every configuration member of the stored record is reported, each out of
            // what the record holds: the capability the assistant was provisioned with,
            // the topic it streams through, the AI agent bound per `AIAgentType` and its
            // orchestrator configurations — the last reported empty, not omitted, the
            // way `tags` is.
            integration_configuration: self.integration_configuration(),
            capability_configuration: self.capability_configuration(),
            ai_agent_configuration: Some(self.ai_agent_configuration.clone()),
            orchestrator_configuration_list: Some(self.orchestrator_configuration_list.clone()),
            // When and where this record was last written: the stored instant, and the
            // Region the assistant's own ARN names (see [`arn_region`]).
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.assistant_arn),
            // Amazon Connect Global Resiliency replicates the CONNECT INSTANCE an
            // assistant is associated with, and the member's own documentation says it is
            // absent for an assistant not yet associated with one, and that an absent
            // member must not be read as DISABLED. Nothing associates an assistant here
            // with a Connect instance, so there is no replication state to report.
            assistant_global_resiliency_status: None,
        }
    }

    /// The `AssistantSummary` `ListAssistants` reports for this assistant — the same
    /// stored values `GetAssistant` answers with.
    pub fn to_summary(&self) -> AssistantSummary {
        AssistantSummary {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // The same record, rendered the same way: a summary reports what
            // `GetAssistant` reports for the assistant it summarises.
            integration_configuration: self.integration_configuration(),
            capability_configuration: self.capability_configuration(),
            ai_agent_configuration: Some(self.ai_agent_configuration.clone()),
            orchestrator_configuration_list: Some(self.orchestrator_configuration_list.clone()),
            // The same stored instant and Region a read of the assistant reports.
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.assistant_arn),
        }
    }

    /// A session id no session of this assistant holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_session_id(&self, seq: u64) -> Option<String> {
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.sessions.contains_key(candidate))
    }

    /// How many sessions a CALLER has created under this assistant, which is what
    /// [`MAX_SESSIONS_PER_ASSISTANT`] bounds: the session the SERVICE provisioned with
    /// the assistant (see [`system_session`]) is not one a caller asked for, so it does
    /// not spend the caller's allowance.
    pub fn customer_session_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| session.origin != SESSION_ORIGIN_SYSTEM)
            .count()
    }

    /// An `aiGuardrailId` no guardrail of this assistant holds, with the same contract as
    /// [`Self::mint_session_id`].
    pub fn mint_ai_guardrail_id(&self, seq: u64) -> Option<String> {
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.ai_guardrails.contains_key(candidate))
    }

    /// An `aiAgentId` no AI agent of this assistant holds, with the same contract as
    /// [`Self::mint_session_id`]. The built-in agents' ids are in that map too, so a
    /// minted id never collides with one the SERVICE publishes.
    pub fn mint_ai_agent_id(&self, seq: u64) -> Option<String> {
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.ai_agents.contains_key(candidate))
    }

    /// The AI agent a `UuidOrArnOrEitherWithQualifier` label names, or `None` when this
    /// assistant holds none under it.
    ///
    /// A label carrying the `$LATEST` qualifier names the agent's own working copy, which
    /// is the record below; any other qualifier names a numbered VERSION of the agent,
    /// which is a published snapshot rather than the working copy, so such a label names
    /// nothing HERE and the caller gets the operation's not-found answer. An operation that
    /// reports whichever revision a label names resolves through
    /// [`Self::ai_agent_revision`] instead.
    pub fn ai_agent(&self, label: &str) -> Option<&AiAgent> {
        let (id, qualifier) = resolve_qualified_id(label);
        match qualifier {
            Some(qualifier) if qualifier != VERSION_QUALIFIER_LATEST => None,
            _ => self.ai_agents.get(id),
        }
    }

    /// The same AI agent as [`Self::ai_agent`], for a caller that WRITES to it: the seam
    /// `UpdateAIAgent` applies its change through, so an update and a read reach one
    /// record rather than two copies.
    ///
    /// A label resolves here exactly as it does for a read, so a qualifier naming a
    /// numbered VERSION of the agent names nothing to write either: a version is a
    /// snapshot of the agent, not the working copy an update changes.
    pub fn ai_agent_mut(&mut self, label: &str) -> Option<&mut AiAgent> {
        let (id, qualifier) = resolve_qualified_id(label);
        match qualifier {
            Some(qualifier) if qualifier != VERSION_QUALIFIER_LATEST => None,
            _ => self.ai_agents.get_mut(id),
        }
    }

    /// Bind `agent_type` to the agent `binding` names, which is the write
    /// `UpdateAssistantAIAgent` makes: from here on every read of this assistant reports
    /// this binding for the type, in place of the default it reported before.
    ///
    /// The binding is stored EXACTLY as the caller sent it — the caller already chose
    /// which agent and, through the qualifier, which revision of it runs for the type — so
    /// nothing about it is materialised here.
    pub fn configure_ai_agent(&mut self, agent_type: &str, binding: AIAgentConfigurationData) {
        self.ai_agent_configuration
            .insert(agent_type.to_string(), binding);
    }

    /// Drop the caller's binding for `agent_type`, which is the write
    /// `RemoveAssistantAIAgent` makes.
    ///
    /// What the type reports afterwards is what it reported before any caller configured
    /// it: the SERVICE's default binding for the type ([`default_ai_agent_binding`]), which
    /// a removal cannot take away because it belongs to the service and not to this
    /// assistant — and no binding at all for a type the service publishes no default for.
    pub fn remove_configured_ai_agent(&mut self, agent_type: &str) {
        match default_ai_agent_binding(agent_type) {
            Some(default) => self.configure_ai_agent(agent_type, default),
            None => {
                self.ai_agent_configuration.remove(agent_type);
            }
        }
    }

    /// Bind the agent `ai_agent_id` names to one ORCHESTRATOR USE CASE of this assistant,
    /// replacing the entry that use case already had. `false` — nothing stored — when this
    /// assistant already carries [`MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT`] use
    /// cases and this is another one.
    ///
    /// A use case identifies the entry — the model makes `orchestratorUseCase` the
    /// required member of an `OrchestratorConfigurationEntry` and the `aiAgentId` the
    /// optional one (`smithy_query(query="shape",shape="OrchestratorConfigurationEntry")`)
    /// — so one use case carries one agent, and configuring it again replaces it rather
    /// than listing it twice. Entries are kept in the order they were first configured, so
    /// a read reports a stable list.
    pub fn configure_orchestrator(&mut self, orchestrator_use_case: &str, ai_agent_id: &str) -> bool {
        let entry = OrchestratorConfigurationEntry {
            ai_agent_id: Some(ai_agent_id.to_string()),
            orchestrator_use_case: orchestrator_use_case.to_string(),
        };
        match self
            .orchestrator_configuration_list
            .iter_mut()
            .find(|existing| existing.orchestrator_use_case == orchestrator_use_case)
        {
            Some(existing) => *existing = entry,
            None => {
                if self.orchestrator_configuration_list.len()
                    >= MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT
                {
                    return false;
                }
                self.orchestrator_configuration_list.push(entry);
            }
        }
        true
    }

    /// Forget this assistant's entry for one orchestrator use case. The service publishes
    /// no default orchestrator entry, so the use case is reported by nothing afterwards.
    pub fn remove_orchestrator(&mut self, orchestrator_use_case: &str) {
        self.orchestrator_configuration_list
            .retain(|entry| entry.orchestrator_use_case != orchestrator_use_case);
    }

    /// How many AI agents a CALLER has created under this assistant, which is what
    /// [`MAX_AI_AGENTS_PER_ASSISTANT`] bounds: the agents the SERVICE provisioned with the
    /// assistant (see [`default_ai_agents`]) are not ones a caller asked for, so they do
    /// not spend the caller's allowance.
    pub fn customer_ai_agent_count(&self) -> usize {
        self.ai_agents
            .values()
            .filter(|agent| agent.origin != AI_AGENT_ORIGIN_SYSTEM)
            .count()
    }

    /// The AI guardrail a `UuidOrArnOrEitherWithQualifier` label names, or `None` when
    /// this assistant holds none under it.
    ///
    /// A label carrying the [`VERSION_QUALIFIER_LATEST`] qualifier names the guardrail's
    /// own working copy, which is the record below — so the ARN a read reports, which
    /// names that version, addresses the same record it was read from. Any other qualifier
    /// names a numbered VERSION of the guardrail, and this emulator publishes none —
    /// `CreateAIGuardrailVersion` is still a scaffold stub — so such a label names nothing
    /// here and the caller gets the operation's not-found answer.
    pub fn ai_guardrail(&self, label: &str) -> Option<&AiGuardrail> {
        let (id, qualifier) = resolve_qualified_id(label);
        match qualifier {
            Some(qualifier) if qualifier != VERSION_QUALIFIER_LATEST => None,
            _ => self.ai_guardrails.get(id),
        }
    }

    /// The same AI guardrail as [`Self::ai_guardrail`], for a caller that WRITES to it:
    /// the seam `UpdateAIGuardrail` applies its change through, so an update and a read
    /// reach one record rather than two copies.
    ///
    /// A label resolves here exactly as it does for a read, so a qualifier naming a
    /// numbered VERSION of the guardrail names nothing to write either: a version is a
    /// snapshot of the guardrail, not the working copy an update changes.
    pub fn ai_guardrail_mut(&mut self, label: &str) -> Option<&mut AiGuardrail> {
        let (id, qualifier) = resolve_qualified_id(label);
        match qualifier {
            Some(qualifier) if qualifier != VERSION_QUALIFIER_LATEST => None,
            _ => self.ai_guardrails.get_mut(id),
        }
    }

    /// The id of the live AI guardrail of this assistant already using this name, if any.
    /// A guardrail's name identifies it inside its assistant, so this is what a second
    /// create under that name is refused by.
    pub fn ai_guardrail_id_with_name(&self, name: &str) -> Option<&str> {
        self.ai_guardrails
            .values()
            .find(|guardrail| guardrail.name == name)
            .map(|guardrail| guardrail.ai_guardrail_id.as_str())
    }
}

/// One knowledge base, as this emulator persists it.
///
/// The knowledge base is the root of the content/quick-response/message-template side of
/// the service, the way [`Assistant`] is the root of the session side.
#[derive(Debug, Clone)]
pub struct KnowledgeBase {
    pub knowledge_base_id: String,
    pub knowledge_base_arn: String,
    pub name: String,
    pub knowledge_base_type: String,
    pub status: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub source_configuration: Option<SourceConfiguration>,
    pub rendering_configuration: Option<RenderingConfiguration>,
    pub vector_ingestion_configuration: Option<VectorIngestionConfiguration>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    /// When the create that minted this record ran, as [`Assistant`] and [`Session`]
    /// keep it too. `KnowledgeBaseData` models no creation-time member, so nothing
    /// reports it yet; it is the record's own age, not a reported value.
    pub created_at: SystemTime,
    /// When this knowledge base's own members were last written: the create that minted
    /// it, until an operation that writes the record — a template URI, a replica key —
    /// re-stamps it. It is what the reported `lastModifiedTime` renders.
    pub modified_at: SystemTime,
}

impl KnowledgeBase {
    /// The `KnowledgeBaseData` every operation that reports this knowledge base answers
    /// with.
    pub fn to_knowledge_base_data(&self) -> KnowledgeBaseData {
        KnowledgeBaseData {
            knowledge_base_id: self.knowledge_base_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            name: self.name.clone(),
            knowledge_base_type: self.knowledge_base_type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            source_configuration: self.source_configuration.clone(),
            rendering_configuration: self.rendering_configuration.clone(),
            vector_ingestion_configuration: self.vector_ingestion_configuration.clone(),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // The model's own wording: "If no content exists in a knowledge base, this
            // value is unset". No content operation is implemented yet, so no knowledge
            // base this service holds has content to have modified.
            last_content_modification_time: None,
            // Ingestion runs against a data source, and nothing in this emulator ingests
            // one, so there is no sync status or failure to report rather than an
            // invented one.
            ingestion_status: None,
            ingestion_failure_reasons: None,
            // When and where this record was last written: the stored instant, and the
            // Region the knowledge base's own ARN names (see [`arn_region`]).
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.knowledge_base_arn),
        }
    }

    /// The `KnowledgeBaseSummary` a listing of this knowledge base reports: the same
    /// stored fields [`Self::to_knowledge_base_data`] answers with, reduced to the
    /// members the summary shape declares
    /// (`smithy_query(query="shape",shape="KnowledgeBaseSummary")`), so a list page and a
    /// read of the same record cannot disagree.
    pub fn to_summary(&self) -> KnowledgeBaseSummary {
        KnowledgeBaseSummary {
            knowledge_base_id: self.knowledge_base_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            name: self.name.clone(),
            knowledge_base_type: self.knowledge_base_type.clone(),
            status: self.status.clone(),
            source_configuration: self.source_configuration.clone(),
            vector_ingestion_configuration: self.vector_ingestion_configuration.clone(),
            rendering_configuration: self.rendering_configuration.clone(),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            // The same stored instant and Region a read of the knowledge base reports.
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.knowledge_base_arn),
        }
    }
}

/// One persisted quick response: a reply an agent inserts by name or by shortcut key,
/// held INSIDE the knowledge base it was created under.
///
/// The record is the single copy of the resource: `CreateQuickResponse` writes it and
/// every read of it answers out of these fields, so a create's answer and a later
/// `GetQuickResponse` cannot disagree. The content is kept as the caller gave it,
/// together with the media type it is read as, and rendered into the modelled
/// `QuickResponseContents` union on the way out ([`Self::contents`]).
#[derive(Debug, Clone)]
pub struct QuickResponse {
    pub quick_response_id: String,
    pub quick_response_arn: String,
    /// The RESOLVED `knowledgeBaseId` of the parent, never the label form the request
    /// used, so a quick response resolves the same way under either label form.
    pub knowledge_base_id: String,
    pub knowledge_base_arn: String,
    pub name: String,
    /// `QuickResponseType` on the wire: the media type the content is read as.
    pub content_type: String,
    /// `QuickResponseStatus` on the wire.
    pub status: String,
    /// The content the create carried, as given.
    pub content: String,
    pub description: Option<String>,
    pub grouping_configuration: Option<GroupingConfiguration>,
    pub shortcut_key: Option<String>,
    pub is_active: Option<bool>,
    pub channels: Option<Vec<String>>,
    pub language: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When this record last changed. Equal to [`Self::created_at`] until an update
    /// writes it, so `lastModifiedTime` is never earlier than `createdTime`.
    pub last_modified_at: SystemTime,
    /// The ARN of the identity that last wrote this record ([`caller_principal`]),
    /// stamped by the create and re-stamped by every update, so a read reports who
    /// wrote the values it is reading rather than who is reading them.
    pub last_modified_by: String,
}

/// The contact-attribute keys `content` names, each once and in the order the content
/// first names it.
///
/// A placeholder is `{{<key>}}`, as the service's own render path describes a template
/// variable ("the rendered message content will keep the variable placeholder as it is
/// and return the attribute keys that are missing",
/// `smithy_query(query="operation",operation="RenderMessageTemplate")`), with the key
/// trimmed of surrounding whitespace; an unterminated or empty placeholder names no
/// attribute and is ignored, so no key this emulator cannot read off the content is ever
/// claimed.
///
/// Read off the content by [`text_attribute_keys`], which is the one place this service
/// decides what a placeholder is: the keys a quick response reports and the placeholders a
/// render substitutes therefore cannot disagree.
fn content_attribute_keys(content: &str) -> Vec<String> {
    text_attribute_keys(content)
}

impl QuickResponse {
    /// The stored content as the modelled `QuickResponseContents`: "<p>The content of the
    /// quick response stored in different media types.</p>"
    /// (`smithy_query(query="shape",shape="QuickResponseContents")`), so the structure
    /// reports the content under EVERY member whose media type this quick response can be
    /// read as.
    ///
    /// Plain text is always one of them — the stored text is readable as plain text
    /// whatever it is marked up as — and `markdown` is reported as well when the quick
    /// response is written in richtext, which is the media type its `contentType` names.
    /// A plain-text quick response therefore reports `plainText` alone, and a richtext one
    /// reports both; that is what the recorded wire answers of this endpoint carry (corpus
    /// `simparity-wire-aws-v1`, fixtures `wisdom-quick-response-error-paths` and
    /// `wisdom-quick-response-lifecycle`).
    fn contents(&self) -> QuickResponseContents {
        let provider = QuickResponseContentProvider::Content(self.content.clone());
        QuickResponseContents {
            plain_text: Some(provider.clone()),
            markdown: (self.content_type == QUICK_RESPONSE_TYPE_MARKDOWN).then_some(provider),
        }
    }

    /// The contact attributes this quick response's content references, which the service
    /// reports beside the content as `attributeList`.
    ///
    /// A quick response carries no attribute member of its own — neither
    /// `CreateQuickResponse` nor `UpdateQuickResponse` declares one — so the attributes it
    /// holds are the ones its CONTENT names, as a placeholder `{{<key>}}`; the model's own
    /// `QuickResponseSearchResultData` reports which of those a search resolved
    /// (`attributesInterpolated`) and which it did not
    /// (`attributesNotInterpolated`, target `ContactAttributeKeys`), so the keys are
    /// contact-attribute keys and this list is their set. Each key is reported once, in
    /// the order the content first names it, and a content that names none reports the
    /// EMPTY list rather than no list at all — which is what the recorded wire answers of
    /// this endpoint carry for a quick response whose content references no attribute
    /// (corpus `simparity-wire-aws-v1`, the `wisdom-quick-response-*` fixtures).
    ///
    /// What the recording pins is that answer for a content naming no attribute; the keys
    /// of a content that DOES name one are read off the stored content, which is the only
    /// place this service holds any, rather than claimed from anywhere else.
    pub fn attribute_list(&self) -> Vec<String> {
        content_attribute_keys(&self.content)
    }

    /// This quick response as the `QuickResponseData` shape `CreateQuickResponse` and
    /// `GetQuickResponse` return.
    pub fn to_quick_response_data(&self) -> QuickResponseData {
        QuickResponseData {
            quick_response_arn: self.quick_response_arn.clone(),
            quick_response_id: self.quick_response_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            content_type: self.content_type.clone(),
            status: self.status.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_at),
            // The Region the quick response's own ARN names (see [`arn_region`]), which is
            // where every write of this record happened.
            last_modified_region: arn_region(&self.quick_response_arn),
            contents: Some(self.contents()),
            description: self.description.clone(),
            grouping_configuration: self.grouping_configuration.clone(),
            shortcut_key: self.shortcut_key.clone(),
            // `lastModifiedBy` is the ARN of the identity that last wrote the resource,
            // which this record carries because each write stamped it.
            last_modified_by: Some(self.last_modified_by.clone()),
            is_active: self.is_active,
            channels: self.channels.clone(),
            language: self.language.clone(),
            // The contact attributes the content names, empty when it names none — see
            // [`Self::attribute_list`].
            attribute_list: Some(self.attribute_list()),
            tags: tags_out(&self.tags),
        }
    }

    /// The `QuickResponseSummary` a listing of this quick response reports: the same
    /// stored fields [`Self::to_quick_response_data`] answers with, reduced to the members
    /// the summary shape declares
    /// (`smithy_query(query="shape",shape="QuickResponseSummary")`), so a list page and a
    /// read of the same record cannot disagree.
    pub fn to_summary(&self) -> QuickResponseSummary {
        QuickResponseSummary {
            quick_response_arn: self.quick_response_arn.clone(),
            quick_response_id: self.quick_response_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            content_type: self.content_type.clone(),
            status: self.status.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_at),
            // The same Region a read of the quick response reports.
            last_modified_region: arn_region(&self.quick_response_arn),
            description: self.description.clone(),
            last_modified_by: Some(self.last_modified_by.clone()),
            is_active: self.is_active,
            channels: self.channels.clone(),
            tags: tags_out(&self.tags),
        }
    }

    /// Whether this quick response is in service, as the required `isActive` of a search
    /// hit reports it: the value the create named, or
    /// [`QUICK_RESPONSE_ACTIVE_BY_DEFAULT`] for a create that named none.
    pub fn is_active_or_default(&self) -> bool {
        self.is_active.unwrap_or(QUICK_RESPONSE_ACTIVE_BY_DEFAULT)
    }

    /// The `QuickResponseSearchResultData` a search of this quick response's knowledge
    /// base reports it as: the same stored fields the other two projections answer with,
    /// shaped to the members the search result declares
    /// (`smithy_query(query="shape",shape="QuickResponseSearchResultData")`), so a search
    /// hit and a read of the same record cannot disagree.
    pub fn to_search_result(&self) -> QuickResponseSearchResultData {
        QuickResponseSearchResultData {
            quick_response_arn: self.quick_response_arn.clone(),
            quick_response_id: self.quick_response_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            content_type: self.content_type.clone(),
            status: self.status.clone(),
            contents: self.contents(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_at),
            // The same Region a read of the quick response reports.
            last_modified_region: arn_region(&self.quick_response_arn),
            is_active: self.is_active_or_default(),
            description: self.description.clone(),
            grouping_configuration: self.grouping_configuration.clone(),
            shortcut_key: self.shortcut_key.clone(),
            // See [`Self::to_quick_response_data`]: the identity the stored record was
            // last written by.
            last_modified_by: Some(self.last_modified_by.clone()),
            channels: self.channels.clone(),
            language: self.language.clone(),
            // These two report which of the contact attributes a search was given were
            // resolved into the content it answers with. This endpoint resolves none —
            // the content travels exactly as the create stored it — so neither list is
            // claimed.
            attributes_not_interpolated: None,
            attributes_interpolated: None,
            tags: tags_out(&self.tags),
        }
    }
}

/// What one assistant association associates the assistant WITH: the modelled
/// `AssistantAssociationInputData` union, as this emulator persists it.
///
/// The variant is the one the create named, and it is stored RESOLVED: a knowledge base
/// of this account is kept by id together with the ARN it carried when the association
/// was made, so a later read of the association reports the knowledge base it was made
/// against even once that knowledge base is gone (`DeleteKnowledgeBase` does not refuse
/// over a live association).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssociationTarget {
    /// A knowledge base this account holds — `associationType` `KNOWLEDGE_BASE`.
    KnowledgeBase {
        knowledge_base_id: String,
        knowledge_base_arn: String,
    },
    /// A Bedrock knowledge base outside this service — `associationType`
    /// `EXTERNAL_BEDROCK_KNOWLEDGE_BASE`. Both ARNs name resources of OTHER services
    /// (Bedrock, IAM), which this emulator does not hold, so they are stored as the
    /// caller gave them; their form is already refused by the modelled `@pattern`/
    /// `@length` of `ExternalBedrockKnowledgeBaseConfig` before a handler runs.
    ExternalBedrockKnowledgeBase {
        bedrock_knowledge_base_arn: String,
        access_role_arn: String,
    },
}

impl AssociationTarget {
    /// This target as the modelled `AssistantAssociationOutputData` union every
    /// operation that reports an association answers with.
    fn to_output_data(&self) -> AssistantAssociationOutputData {
        match self {
            Self::KnowledgeBase {
                knowledge_base_id,
                knowledge_base_arn,
            } => AssistantAssociationOutputData::knowledgeBaseAssociation(
                KnowledgeBaseAssociationData {
                    knowledge_base_id: Some(knowledge_base_id.clone()),
                    knowledge_base_arn: Some(knowledge_base_arn.clone()),
                },
            ),
            Self::ExternalBedrockKnowledgeBase {
                bedrock_knowledge_base_arn,
                access_role_arn,
            } => AssistantAssociationOutputData::externalBedrockKnowledgeBaseConfig(
                ExternalBedrockKnowledgeBaseConfig {
                    bedrock_knowledge_base_arn: bedrock_knowledge_base_arn.clone(),
                    access_role_arn: access_role_arn.clone(),
                },
            ),
        }
    }
}

/// One association between an assistant and the data it answers out of, as this
/// emulator persists it.
///
/// An association is reachable only through its assistant — the modelled routes are all
/// `/assistants/{assistantId}/associations/…` — so [`Self::assistant_id`] is what makes
/// a lookup under another assistant miss.
#[derive(Debug, Clone)]
pub struct AssistantAssociation {
    pub assistant_association_id: String,
    pub assistant_association_arn: String,
    pub assistant_id: String,
    pub assistant_arn: String,
    pub association_type: String,
    pub target: AssociationTarget,
    pub tags: HashMap<String, String>,
    /// When the create that minted this record ran, as every other record here keeps it.
    /// `AssistantAssociationData` models no creation-time member, so nothing reports it.
    pub created_at: SystemTime,
}

impl AssistantAssociation {
    /// The `AssistantAssociationData` every operation that reports this association
    /// answers with.
    pub fn to_assistant_association_data(&self) -> AssistantAssociationData {
        AssistantAssociationData {
            assistant_association_id: self.assistant_association_id.clone(),
            assistant_association_arn: self.assistant_association_arn.clone(),
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            association_type: self.association_type.clone(),
            association_data: self.target.to_output_data(),
            tags: tags_out(&self.tags),
            // An association is written once and never updated — the model declares no
            // operation that changes one — so the instant it was last modified is the
            // instant the create wrote it, and the Region is the one its own ARN names
            // (see [`arn_region`]).
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_modified_region: arn_region(&self.assistant_association_arn),
        }
    }

    /// The `AssistantAssociationSummary` a listing of this association reports: the same
    /// stored fields [`Self::to_assistant_association_data`] answers with, shaped to the
    /// members the summary declares
    /// (`smithy_query(query="shape",shape="AssistantAssociationSummary")`), so a list page
    /// and a read of the same record cannot disagree.
    pub fn to_summary(&self) -> AssistantAssociationSummary {
        AssistantAssociationSummary {
            assistant_association_id: self.assistant_association_id.clone(),
            assistant_association_arn: self.assistant_association_arn.clone(),
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            association_type: self.association_type.clone(),
            association_data: self.target.to_output_data(),
            tags: tags_out(&self.tags),
            // The same instant and Region a read of the association reports.
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_modified_region: arn_region(&self.assistant_association_arn),
        }
    }
}

/// A stable fingerprint of the request members a create's `@idempotencyToken`
/// identifies.
///
/// An idempotency token identifies ONE request, so a replay carrying the identical
/// request is a retry and answers with the resource the first call created, while the
/// same token carrying a DIFFERENT request is an idempotency conflict and is refused.
/// The fingerprint is what tells those two apart, so a create fingerprints every member
/// the record it inserts is made of and nothing else — never the token itself, which both
/// requests share.
///
/// Every part is length-prefixed, so no value can run into its neighbour and leave the
/// fingerprint unchanged (`["a", "bc"]` and `["ab", "c"]` differ), and an optional member
/// the request omitted goes in as an empty part rather than being left out, so "absent"
/// and "present but empty" are different requests.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    let mut rendered = String::new();
    for part in parts {
        let part = part.as_ref();
        rendered.push_str(&part.len().to_string());
        rendered.push('\u{1f}');
        rendered.push_str(part);
        rendered.push('\u{1e}');
    }
    rendered
}

/// A tag map as ONE part of a [`request_fingerprint`]: the pairs in key order, so the
/// same tag set written in another order is one request.
pub fn tags_fingerprint(tags: &HashMap<String, String>) -> String {
    let mut pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    pairs.sort_unstable();
    let flattened: Vec<&str> = pairs.into_iter().flat_map(|(key, value)| [key, value]).collect();
    request_fingerprint(&flattened)
}

/// The most `clientToken` entries the store remembers at a time.
///
/// A remembered token exists only so that a RETRY of the call that used it can be
/// answered with the resource that call created, and a retry follows its call within
/// moments, so a few hundred of the most recent tokens is far more than the purpose needs.
/// The ceiling is what keeps the map finite: a caller varying the token sends an entry per
/// request, and an entry is otherwise released only when the resource it names is deleted,
/// so without it one caller could grow this process's heap through the map alone — the
/// same heap guard [`MAX_AI_AGENT_CONFIGURATION_ENTRIES`] is for the lists one request
/// carries. Past the ceiling the LEAST RECENTLY remembered entry is evicted (see
/// [`ComAmazonawsQconnectWisdomserviceState::remember_create`]), so a token's retry window
/// is the last [`MAX_IDEMPOTENCY_TOKENS`] tokens rather than forever, and a replay of a
/// token evicted long ago is answered as the fresh request it now is.
///
/// CHOSEN: AWS states no retention for an idempotency token's record; the figure is this
/// emulator's own, and nothing but the retry window depends on it.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 256;

/// What a `clientToken` a create used names: the resource that call created, and the
/// [`request_fingerprint`] of the request it created it from.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The id of the resource the first call carrying this token created.
    pub resource_id: String,
    /// The fingerprint of that call's idempotency-relevant members.
    pub fingerprint: String,
    /// When this entry was remembered, on the store's own sequence: the order
    /// [`MAX_IDEMPOTENCY_TOKENS`] evicts in, so the entries the ceiling keeps are the most
    /// recent ones.
    pub remembered_seq: u64,
}

/// What a create's `@idempotencyToken` says about THIS request — see
/// [`ComAmazonawsQconnectWisdomserviceState::replayed_create`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replay {
    /// No create has used this token (or the request carries none): mint a resource.
    Fresh,
    /// The same token AND the same request: a retry, answered with the resource the
    /// first call created.
    Retry(String),
    /// The same token carrying a DIFFERENT request: an idempotency conflict.
    Conflict,
}

/// What the association target a request names comes down to: the member of the
/// `AssistantAssociationInputData` union it carries, and the values that identify the
/// resource under it.
///
/// Taken from the REQUEST, so the create's [`request_fingerprint`] can be computed before
/// the target is resolved: a retry whose token is known is answered out of the record the
/// first call made even when the resource it named has since been deleted.
pub fn input_target_key(association: &AssistantAssociationInputData) -> String {
    match association {
        AssistantAssociationInputData::knowledgeBaseId(knowledge_base_id) => {
            format!("knowledgeBase={knowledge_base_id}")
        }
        AssistantAssociationInputData::externalBedrockKnowledgeBaseConfig(config) => format!(
            "externalBedrock={}|{}",
            config.bedrock_knowledge_base_arn, config.access_role_arn
        ),
    }
}

/// One content upload `StartContentUpload` opened: the slot the URL it handed out
/// writes into.
///
/// The record is what makes that URL a locator for something this emulator holds: the
/// upload plane (`crate::service`) serves a `PUT` to it out of this record, and
/// `CreateContent`/`UpdateContent` name the same `uploadId` to finalize the asset. The
/// published headers are STORED rather than recomputed, so the headers the start call
/// told the caller to include are exactly the ones the `PUT` is held to.
#[derive(Debug, Clone)]
pub struct ContentUpload {
    pub upload_id: String,
    /// The `knowledgeBaseId` the upload was started under — the resolved id, never the
    /// label form the request used, so an upload resolves the same way by id or by ARN.
    pub knowledge_base_id: String,
    /// `ContentType` on the wire: the one media type this upload accepts.
    pub content_type: String,
    /// The URL handed to the caller, kept so the record and the locator cannot drift.
    pub url: String,
    /// When the URL stops being usable. A `PUT` at or after it is refused and the
    /// record is reclaimed.
    pub url_expiry: SystemTime,
    /// The headers the start call published, which the `PUT` must carry. A `BTreeMap`,
    /// so the map a response renders does not depend on hash order.
    pub headers_to_include: BTreeMap<String, String>,
    pub started_at: SystemTime,
    /// The bytes a `PUT` to the URL stored, `None` until one does.
    pub body: Option<Vec<u8>>,
    /// When those bytes arrived.
    pub received_at: Option<SystemTime>,
}

impl ContentUpload {
    /// Whether this upload's URL is no longer usable at `now`.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        now >= self.url_expiry
    }

    /// Whether a request carries every header this upload was started with, each with
    /// the published value. `sent` reads one header off the request by name (HTTP header
    /// names are case-insensitive, and the published names are already lowercase).
    ///
    /// This is what `headersToInclude` MEANS: the upload is written only by a request
    /// that includes them, so a caller that echoes the start call's map is accepted and
    /// one that drops a header is refused.
    pub fn headers_satisfied<F>(&self, sent: F) -> bool
    where
        F: Fn(&str) -> Option<String>,
    {
        self.headers_to_include
            .iter()
            .all(|(name, value)| sent(name).as_deref() == Some(value.as_str()))
    }
}

/// What a request to a published content-asset URL names: the knowledge-base label and
/// the content id, as [`content_asset_path`] words them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentAssetTarget {
    pub knowledge_base_label: String,
    pub content_id: String,
}

/// The path the asset plane serves one content's bytes under.
///
/// It sits under the content it belongs to, one segment deeper than the modelled read of
/// that content (`GET /knowledgeBases/{knowledgeBaseId}/contents/{contentId}`), so it
/// names no modelled operation: the service's own data plane serves it, behind the
/// emulator's signature verification, the way [`content_upload_path`] is served. The URL a
/// caller is handed for it is signed by [`sign_data_plane_url`].
pub fn content_asset_path(knowledge_base_id: &str, content_id: &str) -> String {
    format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}/asset")
}

/// The content a request path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_content_asset_path(path: &str) -> Option<ContentAssetTarget> {
    let rest = path.strip_prefix("/knowledgeBases/")?;
    let mut segments = rest.split('/');
    let (
        Some(knowledge_base_label),
        Some("contents"),
        Some(content_id),
        Some("asset"),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    if knowledge_base_label.is_empty() || content_id.is_empty() {
        return None;
    }
    Some(ContentAssetTarget {
        knowledge_base_label: knowledge_base_label.to_string(),
        content_id: content_id.to_string(),
    })
}

/// One content of a knowledge base, as this emulator persists it.
///
/// `CreateContent` finalizes the asset a `StartContentUpload` URL received into one of
/// these, and every later read of that content — `GetContent`, and the `contentArn` a
/// tagging call addresses — is answered out of this one record.
#[derive(Debug, Clone)]
pub struct Content {
    pub content_id: String,
    pub content_arn: String,
    /// The `knowledgeBaseId` this content lives in — the resolved id, never the label
    /// form the request used.
    pub knowledge_base_id: String,
    pub knowledge_base_arn: String,
    pub name: String,
    /// `ContentData$title`, which the model requires: the title the create gave, or the
    /// content's own name when it gave none.
    pub title: String,
    /// `ContentData$revisionId`: which revision of this content a read is of. It changes
    /// whenever something writes the content, which is what lets a writer name the
    /// revision it read and be refused when another writer got there first — the
    /// optimistic lock `UpdateContent`'s `revisionId` documents.
    pub revision_id: String,
    /// The media type the upload this content was created from was started for, so the
    /// content reports the type its asset actually is.
    pub content_type: String,
    pub metadata: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub link_out_uri: Option<String>,
    /// The URL the asset is read from, kept so the record and the locator cannot drift.
    pub url: String,
    /// When that URL stops being usable — a read of the asset at or after it is refused.
    pub url_expiry: SystemTime,
    /// The asset bytes, as the upload plane received them.
    pub body: Vec<u8>,
    pub created_at: SystemTime,
    /// When this content's own members were last written: the create that minted it, until
    /// an `UpdateContent` re-stamps it. It is what the reported `lastModifiedTime` renders,
    /// so that member is never earlier than the create.
    pub modified_at: SystemTime,
    /// When the ingestion this create started is due to finish, after which every read
    /// reports the content ACTIVE. See [`CONTENT_ACTIVATION_DELAY`].
    pub active_at: SystemTime,
}

impl Content {
    /// The `ContentStatus` this content carries at `now`: the ingestion the create
    /// started is in progress until the clock reaches [`Self::active_at`], and the
    /// content is `ACTIVE` from then on. Computed rather than stored, so nothing has to
    /// be written for the transition the clock already made due.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.active_at {
            CONTENT_STATUS_CREATE_IN_PROGRESS
        } else {
            CONTENT_STATUS_ACTIVE
        }
    }

    /// Whether this content's asset URL is no longer usable at `now`.
    pub fn has_expired_url(&self, now: SystemTime) -> bool {
        now >= self.url_expiry
    }

    /// This content as the `ContentSummary` shape `ListContents` and `SearchContent`
    /// report, which is `ContentData` without the asset locator: the summary names the
    /// content, not a URL to download it.
    pub fn to_summary(&self, now: SystemTime) -> ContentSummary {
        ContentSummary {
            content_arn: self.content_arn.clone(),
            content_id: self.content_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            revision_id: self.revision_id.clone(),
            title: self.title.clone(),
            content_type: self.content_type.clone(),
            status: self.status(now).to_string(),
            metadata: self.metadata.clone(),
            tags: tags_out(&self.tags),
            // When and where this record was last written: the stored instant, and the
            // Region the content's own ARN names (see [`arn_region`]).
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.content_arn),
            // The model reports this hash "only for replicated content", and nothing here
            // replicates a content into another Region, so there is none to report.
            file_hash: None,
        }
    }

    /// This content as the `ContentData` shape `CreateContent` and `GetContent` return.
    pub fn to_content_data(&self, now: SystemTime) -> ContentData {
        ContentData {
            content_arn: self.content_arn.clone(),
            content_id: self.content_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            revision_id: self.revision_id.clone(),
            title: self.title.clone(),
            content_type: self.content_type.clone(),
            status: self.status(now).to_string(),
            metadata: self.metadata.clone(),
            tags: tags_out(&self.tags),
            link_out_uri: self.link_out_uri.clone(),
            url: self.url.clone(),
            url_expiry: ::aws_smithy_types::DateTime::from(self.url_expiry),
            // The same instant, Region and (absent) hash the summary of this content
            // reports — see [`Self::to_summary`].
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            last_modified_region: arn_region(&self.content_arn),
            file_hash: None,
        }
    }
}

/// What a request to a published upload URL names: the knowledge-base label and the
/// upload id, as [`content_upload_path`] words them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadTarget {
    pub knowledge_base_label: String,
    pub upload_id: String,
}

/// The path the upload plane serves one upload under.
///
/// It sits under the knowledge base the upload belongs to — the same `/knowledgeBases`
/// space the modelled routes use — and names no modelled operation (the model's own
/// upload route is `POST /knowledgeBases/{knowledgeBaseId}/upload`), so the service's own
/// data plane serves it, behind the emulator's signature verification. The URL a caller is
/// handed for it is signed by [`sign_data_plane_url`].
pub fn content_upload_path(knowledge_base_id: &str, upload_id: &str) -> String {
    format!("/knowledgeBases/{knowledge_base_id}/uploads/{upload_id}")
}

/// The upload a request path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_content_upload_path(path: &str) -> Option<ContentUploadTarget> {
    let rest = path.strip_prefix("/knowledgeBases/")?;
    let mut segments = rest.split('/');
    let (Some(knowledge_base_label), Some("uploads"), Some(upload_id), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if knowledge_base_label.is_empty() || upload_id.is_empty() {
        return None;
    }
    Some(ContentUploadTarget {
        knowledge_base_label: knowledge_base_label.to_string(),
        upload_id: upload_id.to_string(),
    })
}

/// Per-service state for com-amazonaws-qconnect-wisdomservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsQconnectWisdomserviceState {
    /// Every live assistant, keyed by its `assistantId` — the service's primary index.
    pub assistants: HashMap<String, Assistant>,
    /// Every live knowledge base, keyed by its `knowledgeBaseId`.
    pub knowledge_bases: HashMap<String, KnowledgeBase>,
    /// Every live quick response, keyed by its `quickResponseId`. Each record names the
    /// knowledge base it belongs to, so a lookup is always scoped to the parent the route
    /// named, and at most [`MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE`] of them belong to
    /// any one knowledge base.
    pub quick_responses: HashMap<String, QuickResponse>,
    /// Every live assistant association, keyed by its `assistantAssociationId`. Each
    /// record names the assistant it belongs to, so a lookup is always scoped to the
    /// parent the route named.
    pub assistant_associations: HashMap<String, AssistantAssociation>,
    /// `name` -> `knowledgeBaseId`: a knowledge base's name identifies it, so a second
    /// create under a live name is the conflict `ConflictException` describes.
    pub knowledge_base_name_index: HashMap<String, String>,
    /// `name` -> `assistantId`: an assistant's name identifies it, so a second create
    /// under a live name is the conflict `ConflictException` describes.
    pub assistant_name_index: HashMap<String, String>,
    /// `clientToken` -> what that token created and the request it created it from, so a
    /// replayed `@idempotencyToken` answers with the resource it already made instead of
    /// creating a second one, and the same token carrying a DIFFERENT request is refused
    /// as the conflict it is. Keyed through [`idempotency_key`], so one token used on
    /// two different creates does not collide. See [`Self::replayed_create`].
    pub idempotency: HashMap<String, IdempotentCreate>,
    /// The id mint's sequence: every mint takes the next number, so no two minted ids
    /// can be the same candidate.
    next_id_seq: u64,
    /// Every content upload `StartContentUpload` opened, keyed by its `uploadId`, at
    /// most [`MAX_CONTENT_UPLOADS`] of them and each holding at most
    /// [`MAX_CONTENT_UPLOAD_BYTES`] bytes. An entry is reclaimed once its URL expires
    /// or its knowledge base is deleted.
    pub content_uploads: HashMap<String, ContentUpload>,
    /// Every content `CreateContent` finalized, keyed by its `contentId`. Each record
    /// names the knowledge base it lives in, so a lookup is always scoped to the parent
    /// the route named.
    pub contents: HashMap<String, Content>,
    /// Every live message template, keyed by its `messageTemplateId`. Each record names
    /// the knowledge base it belongs to, so a lookup is always scoped to the parent the
    /// route named, and at most [`MAX_MESSAGE_TEMPLATES_PER_KNOWLEDGE_BASE`] of them
    /// belong to any one knowledge base.
    pub message_templates: HashMap<String, MessageTemplate>,
    /// Every published version of every message template, keyed by the
    /// `messageTemplateId` of the template that published it and held in ascending
    /// `versionNumber` order, at most [`MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE`] per
    /// template. A template that has published none holds no entry, which is the correct
    /// "no versions" answer for it.
    pub message_template_versions: MessageTemplateVersions,
}

impl ComAmazonawsQconnectWisdomserviceState {
    /// The next sequence number for the id mint. Hand it to [`Self::mint_assistant_id`]
    /// or [`Assistant::mint_session_id`]; each call owns its own block of candidates.
    pub fn next_seq(&mut self) -> u64 {
        self.next_id_seq = self.next_id_seq.wrapping_add(1);
        self.next_id_seq
    }

    /// An assistant id no stored assistant holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates of this sequence number's block were all taken.
    pub fn mint_assistant_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.assistants.contains_key(candidate))
    }

    /// An `uploadId` no stored upload holds, or `None` when [`MINT_ATTEMPTS`] candidates
    /// of this sequence number's block were all taken.
    ///
    /// The model constrains `UploadId` by length alone (`len 1..1200`, no pattern), so a
    /// minted id of the same form satisfies it. It names the slot in the URL's path; what
    /// admits a write to that URL is the signature the URL carries (see
    /// [`sign_data_plane_url`]).
    pub fn mint_content_upload_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.content_uploads.contains_key(candidate))
    }

    /// A `contentId` no stored content holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates of this sequence number's block were all taken.
    pub fn mint_content_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.contents.contains_key(candidate))
    }

    /// What a create's `clientToken` says about a request whose idempotency-relevant
    /// members fingerprint to `fingerprint`.
    ///
    /// THE ONE PLACE this service decides retry-or-conflict, so every `@idempotencyToken`
    /// create of this service decides it the same way: an idempotency token identifies
    /// ONE request, so the same token carrying the same request is a retry
    /// ([`Replay::Retry`]) while the same token carrying a different one is an
    /// idempotency conflict ([`Replay::Conflict`]). `scope` is the call the token was
    /// given to — and, for a nested resource, its parent — so the same token handed to
    /// two different creates is two different tokens here.
    ///
    /// A token whose resource has since been deleted has its entry dropped by the delete,
    /// so it replays as [`Replay::Fresh`].
    pub fn replayed_create(
        &self,
        scope: &str,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> Replay {
        let Some(token) = client_token else {
            return Replay::Fresh;
        };
        match self.idempotency.get(&idempotency_key(scope, token)) {
            None => Replay::Fresh,
            Some(known) if known.fingerprint == fingerprint => {
                Replay::Retry(known.resource_id.clone())
            }
            Some(_) => Replay::Conflict,
        }
    }

    /// Remember which resource this create made, and the request it made it from, so a
    /// later call carrying the same token is answered by [`Self::replayed_create`].
    ///
    /// The map is bounded by [`MAX_IDEMPOTENCY_TOKENS`]: remembering a token the map does
    /// not already hold, while it is full, first forgets the least recently remembered
    /// entry, so the entries kept are the most recent ones and the map's size does not
    /// depend on how many tokens a caller sends.
    pub fn remember_create(
        &mut self,
        scope: &str,
        client_token: Option<&str>,
        fingerprint: &str,
        resource_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        let key = idempotency_key(scope, token);
        let remembered_seq = self.next_seq();
        if !self.idempotency.contains_key(&key) && self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .idempotency
                .iter()
                .min_by_key(|(_, remembered)| remembered.remembered_seq)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.idempotency.remove(&oldest);
            }
        }
        self.idempotency.insert(
            key,
            IdempotentCreate {
                resource_id: resource_id.to_string(),
                fingerprint: fingerprint.to_string(),
                remembered_seq,
            },
        );
    }

    /// The content a (resolved `knowledgeBaseId`, `UuidOrArn` label) pair names, or
    /// `None` when that knowledge base holds none under that label — including a content
    /// of ANOTHER knowledge base, which this one does not hold.
    pub fn content(&self, knowledge_base_id: &str, label: &str) -> Option<&Content> {
        self.contents
            .get(resolve_id(label))
            .filter(|content| content.knowledge_base_id == knowledge_base_id)
    }

    /// Every content of one knowledge base, in `contentId` order, so what a read reports
    /// does not depend on map iteration order.
    pub fn contents_of(&self, knowledge_base_id: &str) -> Vec<&Content> {
        let mut contents: Vec<&Content> = self
            .contents
            .values()
            .filter(|content| content.knowledge_base_id == knowledge_base_id)
            .collect();
        contents.sort_by(|left, right| left.content_id.cmp(&right.content_id));
        contents
    }

    /// Persist a finalized content under the knowledge base it names. `false` — nothing
    /// stored — when that knowledge base already holds
    /// [`MAX_CONTENTS_PER_KNOWLEDGE_BASE`], or when the store already holds
    /// [`MAX_STORED_CONTENT_BYTES`] of assets, so both bounds hold however the caller
    /// reaches this.
    pub fn insert_content(&mut self, content: Content) -> bool {
        if self.contents_of(&content.knowledge_base_id).len() >= MAX_CONTENTS_PER_KNOWLEDGE_BASE {
            return false;
        }
        if self
            .stored_content_bytes()
            .saturating_add(content.body.len())
            > MAX_STORED_CONTENT_BYTES
        {
            return false;
        }
        self.contents.insert(content.content_id.clone(), content);
        true
    }

    /// The same content as [`Self::content`], for a caller that WRITES to it: the seam
    /// `UpdateContent` applies its changes through, so an update and a read reach one
    /// record rather than two copies.
    pub fn content_mut(&mut self, knowledge_base_id: &str, label: &str) -> Option<&mut Content> {
        self.contents
            .get_mut(resolve_id(label))
            .filter(|content| content.knowledge_base_id == knowledge_base_id)
    }

    /// Drop a content, together with the `clientToken` entry that created it — a token
    /// whose resource is gone must not resurrect it on a replay, exactly as
    /// [`Self::remove_quick_response`] does for a quick response.
    ///
    /// The name goes with it, so a later create may take that name again: a duplicate
    /// name conflicts with a LIVE content of the knowledge base, not with one that is
    /// gone, and the asset bytes it held come back off [`Self::stored_content_bytes`].
    /// `None` means the pair named no content of that knowledge base, which is the
    /// caller's not-found.
    pub fn remove_content(
        &mut self,
        knowledge_base_id: &str,
        label: &str,
    ) -> Option<Content> {
        let content_id = self.content(knowledge_base_id, label)?.content_id.clone();
        let content = self.contents.remove(&content_id)?;
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| created.resource_id == content_id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(content)
    }

    /// The asset bytes every content of this store holds together — what
    /// [`MAX_STORED_CONTENT_BYTES`] bounds, so a write that REPLACES an asset is measured
    /// against the same figure a create is.
    pub fn stored_content_bytes(&self) -> usize {
        self.contents
            .values()
            .map(|content| content.body.len())
            .sum()
    }

    /// The upload an `uploadId` names, whatever its state.
    pub fn content_upload(&self, upload_id: &str) -> Option<&ContentUpload> {
        self.content_uploads.get(upload_id)
    }

    /// The upload an `uploadId` names, for the plane that writes its bytes.
    pub fn content_upload_mut(&mut self, upload_id: &str) -> Option<&mut ContentUpload> {
        self.content_uploads.get_mut(upload_id)
    }

    /// Forget every upload whose URL has expired by `now`.
    ///
    /// This materialises what the CLOCK already made due — an expired URL cannot be used
    /// by anyone, so the slot was owed back at the instant it expired, whether or not
    /// anybody called — and it is what keeps the store draining rather than growing to
    /// [`MAX_CONTENT_UPLOADS`] and staying there.
    pub fn reclaim_expired_content_uploads(&mut self, now: SystemTime) {
        self.content_uploads
            .retain(|_, upload| !upload.has_expired(now));
    }

    /// Persist an opened upload. `false` — nothing stored — when the store already holds
    /// [`MAX_CONTENT_UPLOADS`], so the bound holds however the caller reaches this.
    pub fn insert_content_upload(&mut self, upload: ContentUpload) -> bool {
        if self.content_uploads.len() >= MAX_CONTENT_UPLOADS {
            return false;
        }
        self.content_uploads
            .insert(upload.upload_id.clone(), upload);
        true
    }

    /// Write one upload's body, as the upload plane's `PUT` does. `false` — nothing
    /// stored — when the written bytes this store already holds leave no room for it
    /// under [`MAX_PENDING_UPLOAD_BYTES`], so the aggregate bound holds however many
    /// uploads a caller opens; the slot the body would have filled is left as it was.
    ///
    /// The bytes an upload already holds do not count against the sum, so re-writing one
    /// upload is accepted at the ceiling: a second `PUT` to the same URL replaces the
    /// first body rather than adding to it.
    pub fn write_content_upload_body(&mut self, upload_id: &str, body: Vec<u8>) -> bool {
        let held: usize = self
            .content_uploads
            .iter()
            .filter(|(id, _)| id.as_str() != upload_id)
            .filter_map(|(_, upload)| upload.body.as_ref().map(Vec::len))
            .sum();
        if held.saturating_add(body.len()) > MAX_PENDING_UPLOAD_BYTES {
            return false;
        }
        match self.content_uploads.get_mut(upload_id) {
            None => false,
            Some(upload) => {
                upload.body = Some(body);
                true
            }
        }
    }

    /// A knowledge-base id no stored knowledge base holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_knowledge_base_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.knowledge_bases.contains_key(candidate))
    }

    /// The tags persisted on the resource a full resource ARN names, whatever resource
    /// family that ARN belongs to.
    ///
    /// Tagging is a cross-family surface: `ListTagsForResource`, `TagResource` and
    /// `UntagResource` take an `Arn` rather than one family's path label
    /// (`smithy_query(query="operation",operation="ListTagsForResource")`), so the lookup
    /// covers EVERY family this store holds a tag map for — assistants, knowledge bases,
    /// contents, sessions, AI guardrails, assistant associations, quick responses and
    /// message templates — and reads the tags off that one stored record. Nothing is copied into a second place: a
    /// tag a create persisted on its resource is the tag reported here, and a resource a
    /// delete removed holds none.
    ///
    /// The ARN is matched against the one the record itself carries, not merely against
    /// the id [`resolve_id`] reads out of it, so an ARN that pairs a stored id with
    /// another region, family or account names NOTHING. `None` therefore means the ARN
    /// names no resource this account holds.
    pub fn tags_of_resource(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        // Every ARN this service mints ends in the resource's own id.
        let id = resolve_id(resource_arn);
        if let Some(assistant) = self
            .assistants
            .get(id)
            .filter(|assistant| assistant.assistant_arn == resource_arn)
        {
            return Some(&assistant.tags);
        }
        if let Some(knowledge_base) = self
            .knowledge_bases
            .get(id)
            .filter(|knowledge_base| knowledge_base.knowledge_base_arn == resource_arn)
        {
            return Some(&knowledge_base.tags);
        }
        if let Some(association) = self
            .assistant_associations
            .get(id)
            .filter(|association| association.assistant_association_arn == resource_arn)
        {
            return Some(&association.tags);
        }
        if let Some(content) = self
            .contents
            .get(id)
            .filter(|content| content.content_arn == resource_arn)
        {
            return Some(&content.tags);
        }
        if let Some(quick_response) = self
            .quick_responses
            .get(id)
            .filter(|quick_response| quick_response.quick_response_arn == resource_arn)
        {
            return Some(&quick_response.tags);
        }
        if let Some(message_template) = self
            .message_templates
            .get(id)
            .filter(|template| template.message_template_arn == resource_arn)
        {
            return Some(&message_template.tags);
        }
        // A resource nested inside an assistant — a session, an AI agent, an AI guardrail
        // — is reachable only through its parent, whose id its ARN carries in the segment
        // before the resource's own ([`session_arn`], [`ai_agent_arn`],
        // [`ai_guardrail_arn`]). A single-segment ARN's preceding field still holds the
        // `arn:…:<label>` prefix, which is no assistant id, so it simply misses.
        let parent = resource_arn.rsplit('/').nth(1)?;
        let assistant = self.assistants.get(parent)?;
        if let Some(guardrail) = assistant
            .ai_guardrails
            .get(id)
            .filter(|guardrail| guardrail.ai_guardrail_arn == resource_arn)
        {
            return Some(&guardrail.tags);
        }
        if let Some(agent) = assistant
            .ai_agents
            .get(id)
            .filter(|agent| agent.ai_agent_arn == resource_arn)
        {
            return Some(&agent.tags);
        }
        if let Some(prompt) = assistant
            .ai_prompts
            .get(id)
            .filter(|prompt| prompt.ai_prompt_arn == resource_arn)
        {
            return Some(&prompt.tags);
        }
        assistant
            .sessions
            .get(id)
            .filter(|session| session.session_arn == resource_arn)
            .map(|session| &session.tags)
    }

    /// The same tag map as [`Self::tags_of_resource`], for a caller that WRITES it — the
    /// seam `TagResource` merges a request into.
    ///
    /// It resolves the ARN exactly as the reporting path does, against the ARN the record
    /// itself carries, and hands back the map ON that stored record: a write here is what
    /// a later `ListTagsForResource` — or a read of the resource — reports, because there
    /// is only ever one copy of a resource's tags. `None` means the ARN names no resource
    /// this account holds.
    pub fn tags_of_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut HashMap<String, String>> {
        let id = resolve_id(resource_arn).to_string();
        if self
            .assistants
            .get(&id)
            .is_some_and(|assistant| assistant.assistant_arn == resource_arn)
        {
            return self
                .assistants
                .get_mut(&id)
                .map(|assistant| &mut assistant.tags);
        }
        if self
            .knowledge_bases
            .get(&id)
            .is_some_and(|knowledge_base| knowledge_base.knowledge_base_arn == resource_arn)
        {
            return self
                .knowledge_bases
                .get_mut(&id)
                .map(|knowledge_base| &mut knowledge_base.tags);
        }
        if self
            .assistant_associations
            .get(&id)
            .is_some_and(|association| association.assistant_association_arn == resource_arn)
        {
            return self
                .assistant_associations
                .get_mut(&id)
                .map(|association| &mut association.tags);
        }
        if self
            .contents
            .get(&id)
            .is_some_and(|content| content.content_arn == resource_arn)
        {
            return self.contents.get_mut(&id).map(|content| &mut content.tags);
        }
        if self
            .quick_responses
            .get(&id)
            .is_some_and(|quick_response| quick_response.quick_response_arn == resource_arn)
        {
            return self
                .quick_responses
                .get_mut(&id)
                .map(|quick_response| &mut quick_response.tags);
        }
        if self
            .message_templates
            .get(&id)
            .is_some_and(|template| template.message_template_arn == resource_arn)
        {
            return self
                .message_templates
                .get_mut(&id)
                .map(|template| &mut template.tags);
        }
        // A resource nested inside an assistant — a session, an AI agent, an AI guardrail
        // — is reachable only through its parent, whose id its ARN carries in the segment
        // before the resource's own.
        let parent = resource_arn.rsplit('/').nth(1)?.to_string();
        let assistant = self.assistants.get_mut(&parent)?;
        if let Some(guardrail) = assistant
            .ai_guardrails
            .get_mut(&id)
            .filter(|guardrail| guardrail.ai_guardrail_arn == resource_arn)
        {
            return Some(&mut guardrail.tags);
        }
        if let Some(agent) = assistant
            .ai_agents
            .get_mut(&id)
            .filter(|agent| agent.ai_agent_arn == resource_arn)
        {
            return Some(&mut agent.tags);
        }
        if let Some(prompt) = assistant
            .ai_prompts
            .get_mut(&id)
            .filter(|prompt| prompt.ai_prompt_arn == resource_arn)
        {
            return Some(&mut prompt.tags);
        }
        assistant
            .sessions
            .get_mut(&id)
            .filter(|session| session.session_arn == resource_arn)
            .map(|session| &mut session.tags)
    }

    /// Store a knowledge base and index it by name.
    pub fn insert_knowledge_base(&mut self, knowledge_base: KnowledgeBase) {
        self.knowledge_base_name_index.insert(
            knowledge_base.name.clone(),
            knowledge_base.knowledge_base_id.clone(),
        );
        self.knowledge_bases
            .insert(knowledge_base.knowledge_base_id.clone(), knowledge_base);
    }

    /// The knowledge base a `UuidOrArn` label names, or `None` when this account holds
    /// none under that label — including an ARN this emulator could not have minted,
    /// which names nothing.
    pub fn knowledge_base(&self, label: &str) -> Option<&KnowledgeBase> {
        self.knowledge_bases.get(resolve_id(label))
    }

    /// The same knowledge base [`Self::knowledge_base`] resolves, for a caller that WRITES
    /// the record — the one resolution both paths share, so a write and a read cannot
    /// disagree about which record a label names.
    pub fn knowledge_base_mut(&mut self, label: &str) -> Option<&mut KnowledgeBase> {
        self.knowledge_bases.get_mut(resolve_id(label))
    }

    /// Forget a knowledge base and every index entry that named it, as
    /// `DeleteKnowledgeBase` does. `None` — nothing removed — when this account holds no
    /// knowledge base under that label, which is the caller's not-found.
    ///
    /// The name the record held is released by the same call, so a create may take it
    /// again: the conflict a duplicate name is refused with is a conflict with a LIVE
    /// knowledge base. The `clientToken` entries that created it go too — a token whose
    /// resource is gone must not resurrect it on a replay — exactly as
    /// [`Self::remove_assistant`] does for an assistant.
    pub fn remove_knowledge_base(&mut self, label: &str) -> Option<KnowledgeBase> {
        let knowledge_base = self.knowledge_bases.remove(resolve_id(label))?;
        self.knowledge_base_name_index.remove(&knowledge_base.name);
        // A content exists only inside its knowledge base, so it goes with it — and the
        // `clientToken` entries that created either are dropped below, since a token
        // whose resource is gone must not resurrect it on a replay.
        let contents: Vec<String> = self
            .contents
            .values()
            .filter(|content| content.knowledge_base_id == knowledge_base.knowledge_base_id)
            .map(|content| content.content_id.clone())
            .collect();
        for content_id in &contents {
            self.contents.remove(content_id);
        }
        // A quick response lives INSIDE the knowledge base it was created under, so a
        // knowledge base that is gone takes its quick responses with it: a later read of
        // one names nothing, and the name and shortcut key it held are released.
        let orphaned: Vec<String> = self
            .quick_responses_of(&knowledge_base.knowledge_base_id)
            .into_iter()
            .map(|quick_response| quick_response.quick_response_id.clone())
            .collect();
        for quick_response_id in &orphaned {
            self.quick_responses.remove(quick_response_id);
        }
        // A message template lives INSIDE its knowledge base on the same footing, so it
        // goes the same way: a later read of one names nothing, and the name it held is
        // released.
        let templates = self.forget_message_templates_of(&knowledge_base.knowledge_base_id);
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| {
                created.resource_id == knowledge_base.knowledge_base_id
                    || contents.contains(&created.resource_id)
                    || orphaned.contains(&created.resource_id)
                    || templates.contains(&created.resource_id)
            })
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        // An upload URL is a locator INSIDE the knowledge base it was started under, so
        // a knowledge base that is gone takes its open uploads with it: following such a
        // URL afterwards names nothing, and the bytes it held are reclaimed.
        self.content_uploads
            .retain(|_, upload| upload.knowledge_base_id != knowledge_base.knowledge_base_id);
        Some(knowledge_base)
    }

    /// The id of the live knowledge base already using this name, if any.
    pub fn knowledge_base_id_with_name(&self, name: &str) -> Option<&str> {
        self.knowledge_base_name_index
            .get(name)
            .map(String::as_str)
            .filter(|id| self.knowledge_bases.contains_key(*id))
    }

    /// A `quickResponseId` no stored quick response holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_quick_response_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.quick_responses.contains_key(candidate))
    }

    /// Store a quick response under the (already resolved) knowledge base it names.
    pub fn insert_quick_response(&mut self, quick_response: QuickResponse) {
        self.quick_responses
            .insert(quick_response.quick_response_id.clone(), quick_response);
    }

    /// The quick response a (knowledge base label, quick response label) pair names, in
    /// either label form.
    ///
    /// The parent is resolved first, and a quick response that belongs to a DIFFERENT
    /// knowledge base does not resolve: the route names a resource inside one knowledge
    /// base, so crossing the two names nothing. `None` is the caller's not-found.
    pub fn quick_response(
        &self,
        knowledge_base_label: &str,
        quick_response_label: &str,
    ) -> Option<&QuickResponse> {
        let knowledge_base_id = self.knowledge_base(knowledge_base_label)?.knowledge_base_id.clone();
        self.quick_responses
            .get(resolve_id(quick_response_label))
            .filter(|quick_response| quick_response.knowledge_base_id == knowledge_base_id)
    }

    /// Every quick response of one (resolved) knowledge base, in `quickResponseId`
    /// order, so what a read reports does not depend on map iteration order.
    pub fn quick_responses_of(&self, knowledge_base_id: &str) -> Vec<&QuickResponse> {
        let mut quick_responses: Vec<&QuickResponse> = self
            .quick_responses
            .values()
            .filter(|quick_response| quick_response.knowledge_base_id == knowledge_base_id)
            .collect();
        quick_responses.sort_by(|left, right| {
            left.quick_response_id.cmp(&right.quick_response_id)
        });
        quick_responses
    }

    /// The live quick response of this knowledge base already using `name`, if any: a
    /// quick response's name identifies it inside its knowledge base.
    pub fn quick_response_with_name(
        &self,
        knowledge_base_id: &str,
        name: &str,
    ) -> Option<&QuickResponse> {
        self.quick_responses_of(knowledge_base_id)
            .into_iter()
            .find(|quick_response| quick_response.name == name)
    }

    /// The live quick response of this knowledge base already using `shortcut_key`, if
    /// any — the uniqueness the model states on the member itself.
    pub fn quick_response_with_shortcut_key(
        &self,
        knowledge_base_id: &str,
        shortcut_key: &str,
    ) -> Option<&QuickResponse> {
        self.quick_responses_of(knowledge_base_id)
            .into_iter()
            .find(|quick_response| {
                quick_response.shortcut_key.as_deref() == Some(shortcut_key)
            })
    }

    /// The same quick response as [`Self::quick_response`], for a caller that WRITES to
    /// it: the seam `UpdateQuickResponse` applies its changes through, so an update and a
    /// read reach one record rather than two copies.
    pub fn quick_response_mut(
        &mut self,
        knowledge_base_label: &str,
        quick_response_label: &str,
    ) -> Option<&mut QuickResponse> {
        let knowledge_base_id = self
            .knowledge_base(knowledge_base_label)?
            .knowledge_base_id
            .clone();
        self.quick_responses
            .get_mut(resolve_id(quick_response_label))
            .filter(|quick_response| quick_response.knowledge_base_id == knowledge_base_id)
    }

    /// Drop a quick response, together with the `clientToken` entry that created it — a
    /// token whose resource is gone must not resurrect it on a replay, exactly as
    /// [`Self::remove_knowledge_base`] drops the tokens of what it reclaims.
    ///
    /// The name and the shortcut key go with it, so a later create may take either again:
    /// both identify a LIVE quick response of the knowledge base, not one that is gone.
    /// `None` means the label pair named no quick response of that knowledge base, which
    /// is the caller's not-found.
    pub fn remove_quick_response(
        &mut self,
        knowledge_base_label: &str,
        quick_response_label: &str,
    ) -> Option<QuickResponse> {
        let quick_response_id = self
            .quick_response(knowledge_base_label, quick_response_label)?
            .quick_response_id
            .clone();
        let quick_response = self.quick_responses.remove(&quick_response_id)?;
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| created.resource_id == quick_response_id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(quick_response)
    }

    /// Store an assistant and index it by name.
    pub fn insert_assistant(&mut self, assistant: Assistant) {
        self.assistant_name_index
            .insert(assistant.name.clone(), assistant.assistant_id.clone());
        self.assistants
            .insert(assistant.assistant_id.clone(), assistant);
    }

    /// The assistant a `UuidOrArn` label names, or `None` when this account holds none
    /// under that label — including an ARN this emulator could not have minted, which
    /// names nothing.
    pub fn assistant(&self, label: &str) -> Option<&Assistant> {
        self.assistants.get(resolve_id(label))
    }

    /// The assistant a `UuidOrArn` label names, for mutation.
    pub fn assistant_mut(&mut self, label: &str) -> Option<&mut Assistant> {
        self.assistants.get_mut(resolve_id(label))
    }

    /// Forget an assistant, its sessions, its associations and every index entry that
    /// named them, as `DeleteAssistant` does.
    pub fn remove_assistant(&mut self, label: &str) -> Option<Assistant> {
        let assistant = self.assistants.remove(resolve_id(label))?;
        self.assistant_name_index.remove(&assistant.name);
        // An association exists only inside its assistant, so it goes with it.
        let associations: Vec<String> = self
            .assistant_associations
            .iter()
            .filter(|(_, association)| association.assistant_id == assistant.assistant_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &associations {
            self.assistant_associations.remove(id);
        }
        // A token whose resource is gone must not resurrect it on a replay.
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| {
                created.resource_id == assistant.assistant_id
                    || assistant.sessions.contains_key(created.resource_id.as_str())
                    || assistant
                        .ai_guardrails
                        .contains_key(created.resource_id.as_str())
                    || assistant
                        .ai_agents
                        .contains_key(created.resource_id.as_str())
                    || assistant
                        .ai_prompts
                        .contains_key(created.resource_id.as_str())
                    || associations.contains(&created.resource_id)
            })
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(assistant)
    }

    /// The AI guardrail a (`assistantId` label, `aiGuardrailId` label) pair names, or
    /// `None` when this account holds no assistant under the first label or that
    /// assistant holds no guardrail under the second — including a guardrail of ANOTHER
    /// assistant, which this one does not hold.
    pub fn ai_guardrail(
        &self,
        assistant_label: &str,
        guardrail_label: &str,
    ) -> Option<&AiGuardrail> {
        self.assistant(assistant_label)?.ai_guardrail(guardrail_label)
    }

    /// The AI agent a (`assistantId` label, `aiAgentId` label) pair names, with the same
    /// contract as [`Self::ai_guardrail`]: an agent is reachable only through its
    /// assistant, so an agent of ANOTHER assistant is not found under this one.
    pub fn ai_agent(&self, assistant_label: &str, agent_label: &str) -> Option<&AiAgent> {
        self.assistant(assistant_label)?.ai_agent(agent_label)
    }

    /// The same AI guardrail as [`Self::ai_guardrail`], for a caller that WRITES to it —
    /// the seam [`Assistant::ai_guardrail_mut`] describes, reached through the assistant
    /// the route named.
    pub fn ai_guardrail_mut(
        &mut self,
        assistant_label: &str,
        guardrail_label: &str,
    ) -> Option<&mut AiGuardrail> {
        self.assistant_mut(assistant_label)?
            .ai_guardrail_mut(guardrail_label)
    }

    /// The same AI agent as [`Self::ai_agent`], for a caller that WRITES to it — the seam
    /// [`Assistant::ai_agent_mut`] describes, reached through the assistant the route
    /// named.
    pub fn ai_agent_mut(
        &mut self,
        assistant_label: &str,
        agent_label: &str,
    ) -> Option<&mut AiAgent> {
        self.assistant_mut(assistant_label)?.ai_agent_mut(agent_label)
    }

    /// Store an AI agent inside the assistant it names. `false` — nothing stored — when
    /// this account holds no such assistant, or when that assistant already holds
    /// [`MAX_AI_AGENTS_PER_ASSISTANT`] of a caller's own, so the bound holds however the
    /// caller reaches this.
    pub fn insert_ai_agent(&mut self, agent: AiAgent) -> bool {
        let Some(assistant) = self.assistants.get_mut(&agent.assistant_id) else {
            return false;
        };
        if assistant.customer_ai_agent_count() >= MAX_AI_AGENTS_PER_ASSISTANT {
            return false;
        }
        assistant.ai_agents.insert(agent.ai_agent_id.clone(), agent);
        true
    }

    /// Store an AI guardrail inside the assistant it names. `false` — nothing stored —
    /// when this account holds no such assistant, or when that assistant already holds
    /// [`MAX_AI_GUARDRAILS_PER_ASSISTANT`], so the bound holds however the caller reaches
    /// this.
    pub fn insert_ai_guardrail(&mut self, guardrail: AiGuardrail) -> bool {
        let Some(assistant) = self.assistants.get_mut(&guardrail.assistant_id) else {
            return false;
        };
        if assistant.ai_guardrails.len() >= MAX_AI_GUARDRAILS_PER_ASSISTANT {
            return false;
        }
        assistant
            .ai_guardrails
            .insert(guardrail.ai_guardrail_id.clone(), guardrail);
        true
    }

    /// Forget one AI guardrail, as `DeleteAIGuardrail` does, together with the
    /// `clientToken` entry that created it: a token whose resource is gone must not
    /// resurrect it on a replay. `None` — nothing removed — when the pair names no
    /// guardrail this account holds, which is the caller's not-found.
    ///
    /// The name the record held is released by the same call, so a create may take it
    /// again: the conflict a duplicate name is refused with is a conflict with a LIVE
    /// guardrail.
    pub fn remove_ai_guardrail(
        &mut self,
        assistant_label: &str,
        guardrail_label: &str,
    ) -> Option<AiGuardrail> {
        let assistant_id = self.assistant(assistant_label)?.assistant_id.clone();
        let guardrail_id = self
            .ai_guardrail(&assistant_id, guardrail_label)?
            .ai_guardrail_id
            .clone();
        let guardrail = self
            .assistants
            .get_mut(&assistant_id)?
            .ai_guardrails
            .remove(&guardrail_id)?;
        self.forget_idempotency_for(&guardrail_id);
        Some(guardrail)
    }

    /// Forget every `clientToken` entry that created the resource `resource_id` names,
    /// which every delete does once the record itself is gone: a token whose resource no
    /// longer exists must not resurrect it on a replay.
    fn forget_idempotency_for(&mut self, resource_id: &str) {
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| created.resource_id == resource_id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
    }

    /// Forget one AI agent, as `DeleteAIAgent` does, together with the `clientToken`
    /// entry that created it: a token whose resource is gone must not resurrect it on a
    /// replay. `None` — nothing removed — when the pair names no agent this account
    /// holds, which is the caller's not-found.
    ///
    /// The name the record held is released by the same call, so a create may take it
    /// again, and the assistant's agent count drops, so the per-assistant bound is about
    /// the agents still LIVE.
    pub fn remove_ai_agent(&mut self, assistant_label: &str, agent_label: &str) -> Option<AiAgent> {
        let assistant_id = self.assistant(assistant_label)?.assistant_id.clone();
        let agent_id = self
            .ai_agent(&assistant_id, agent_label)?
            .ai_agent_id
            .clone();
        let assistant = self.assistants.get_mut(&assistant_id)?;
        let agent = assistant.ai_agents.remove(&agent_id)?;
        // A version is a snapshot of the agent that published it, so an agent that is gone
        // takes every version of it along: nothing addresses `<aiAgentId>:<n>` afterwards.
        assistant.forget_ai_agent_versions_of(&agent_id);
        self.forget_idempotency_for(&agent_id);
        Some(agent)
    }

    /// The id of the live assistant already using this name, if any.
    pub fn assistant_id_with_name(&self, name: &str) -> Option<&str> {
        self.assistant_name_index
            .get(name)
            .map(String::as_str)
            .filter(|id| self.assistants.contains_key(*id))
    }

    /// Every stored assistant's id, in `assistantId` order — the stable order
    /// `ListAssistants` pages over. Ids only, so a list call projects just the page it
    /// answers with instead of cloning every record (and its sessions).
    pub fn assistant_ids_in_order(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.assistants.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Every stored knowledge base's id, in `knowledgeBaseId` order — the stable order
    /// `ListKnowledgeBases` pages over, as [`Self::assistant_ids_in_order`] is for
    /// assistants: a page boundary only holds if the order does not depend on map
    /// iteration.
    pub fn knowledge_base_ids_in_order(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.knowledge_bases.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// An assistant-association id no stored association holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_assistant_association_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.assistant_associations.contains_key(candidate))
    }

    /// Store an assistant association under the assistant it names.
    pub fn insert_assistant_association(&mut self, association: AssistantAssociation) {
        self.assistant_associations
            .insert(association.assistant_association_id.clone(), association);
    }

    /// The association a (resolved `assistantId`, `UuidOrArn` label) pair names, or
    /// `None` when this assistant holds none under that label — including an id that
    /// names an association of ANOTHER assistant, which this assistant does not hold.
    pub fn assistant_association(
        &self,
        assistant_id: &str,
        label: &str,
    ) -> Option<&AssistantAssociation> {
        self.assistant_associations
            .get(resolve_id(label))
            .filter(|association| association.assistant_id == assistant_id)
    }

    /// Every association of one assistant, in `assistantAssociationId` order, so what a
    /// read reports does not depend on map iteration order.
    pub fn assistant_associations_of(&self, assistant_id: &str) -> Vec<&AssistantAssociation> {
        let mut associations: Vec<&AssistantAssociation> = self
            .assistant_associations
            .values()
            .filter(|association| association.assistant_id == assistant_id)
            .collect();
        associations.sort_by(|left, right| {
            left.assistant_association_id
                .cmp(&right.assistant_association_id)
        });
        associations
    }

    /// Forget one association, as `DeleteAssistantAssociation` does, together with the
    /// `clientToken` entry that created it: a token whose resource is gone must not
    /// resurrect it on a replay.
    pub fn remove_assistant_association(
        &mut self,
        assistant_id: &str,
        label: &str,
    ) -> Option<AssistantAssociation> {
        let id = self.assistant_association(assistant_id, label)?.assistant_association_id.clone();
        let association = self.assistant_associations.remove(&id)?;
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| created.resource_id == id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(association)
    }
}

/// The key a `clientToken` is remembered under: the token scoped to the call it was
/// given to (and, for a nested resource, to its parent), so the same token handed to two
/// different creates identifies two different resources rather than colliding.
pub fn idempotency_key(scope: &str, token: &str) -> String {
    format!("{scope}\u{1f}{token}")
}

/// What a pagination cursor this service minted carries, so a token it did not mint is
/// refused rather than read as some position by accident.
const CURSOR_SCOPE: &str = "qconnect-assistants\u{1f}";

/// The pagination cursor for a page that starts at `id`.
///
/// Hex of a scoped key rather than the key itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `nextToken` is opaque and only
/// ever travels back unchanged. Well inside the modelled `NextToken` length (1..2048).
pub fn encode_cursor(id: &str) -> String {
    ::hex::encode(format!("{CURSOR_SCOPE}{id}").as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None` when
/// the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(CURSOR_SCOPE).map(str::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsQconnectWisdomserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsQconnectWisdomserviceState>)
}
