// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda service.
//!
//! The agent lifecycle (`CreateAgent`, `GetAgent`, `ListAgents`, `PrepareAgent`,
//! `DeleteAgent`) is backed by the [`agents`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::agents)
//! map below: every one of those handlers computes its whole response from a
//! record stored here, and the record -> wire projections
//! ([`AgentRecord::to_agent`], [`AgentRecord::to_summary`]) live here so no two
//! handlers can project the same record differently.
//!
//! The flow lifecycle (`CreateFlow`, `UpdateFlow`, `PrepareFlow`, `GetFlow`) is
//! backed by the
//! [`flows`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::flows)
//! map the same way, with [`FlowRecord::to_create_output`] /
//! [`FlowRecord::to_update_output`] / [`FlowRecord::to_get_output`] as its single
//! projections. A flow a `PrepareFlow` accepted leaves `Preparing` on the CLOCK
//! alone, through [`PendingPreparation`] and
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::materialise_due_transitions`],
//! so every reader of the flow observes the same progression.
//!
//! A flow's versions (`CreateFlowVersion`, `ListFlowVersions`) are the flow's
//! own DRAFT version plus the numbered versions cut from it. The DRAFT version
//! IS the stored flow — its status and definition are the ones a later
//! `UpdateFlow`/`PrepareFlow` moves — so it is projected straight off the flow
//! record ([`FlowRecord::to_draft_version_summary`]) rather than copied; each
//! numbered version is a snapshot of that DRAFT at the instant it was cut, held
//! in the
//! [`flow_versions`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::flow_versions)
//! map, keyed by the flow and the version number it was cut as, with
//! [`FlowVersionRecord::to_summary`] / [`FlowVersionRecord::to_create_output`]
//! as its only projections and
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::page_of_flow_versions`]
//! as the one place the listing's order and paging live.
//!
//! The knowledge-base lifecycle (`CreateKnowledgeBase`, `UpdateKnowledgeBase`,
//! `GetKnowledgeBase`, `ListKnowledgeBases`) is backed by the
//! [`knowledge_bases`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::knowledge_bases)
//! map, with [`KnowledgeBaseRecord::to_knowledge_base`] and
//! [`KnowledgeBaseRecord::to_summary`] as its only two projections and
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::page_of_knowledge_bases`]
//! as the one place the listing's order and paging live. What a knowledge base's
//! `storageConfiguration` binds it to is resolved in the service that owns that
//! vector store, in the `vector_store` module beside this one, so no knowledge
//! base is created against an index that is not there.
//!
//! A knowledge base's resource-based POLICY (`GetResourcePolicy`, and the write
//! and delete beside it) is held on the knowledge base's own record, as
//! [`KnowledgeBaseRecord::resource_policy`]: which knowledge base a
//! `resourceArn` names, which KIND of knowledge base carries a policy at all and
//! the one resolution both of those readings make are in the
//! `resource_policies` module beside this one, so every operation about a policy
//! answers one ARN alike.
//!
//! A knowledge base's data sources (`CreateDataSource`, `UpdateDataSource`,
//! `GetDataSource`) are backed by the
//! [`data_sources`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::data_sources)
//! map, keyed by `dataSourceId` and carrying the `knowledgeBaseId` it belongs
//! to, with [`DataSourceRecord::to_data_source`] as its single projection. What
//! that map holds is the data source's EFFECTIVE configuration: the connector
//! defaults the service applies to the members a request left unset are
//! completed once, in [`complete_connector_defaults`], before the record is
//! stored, so create, update and every read report one configuration. A data
//! source whose connector has a source outside the service to reach
//! ([`connects_to_an_external_source`]) is stored `CREATING` and reaches its
//! outcome on the CLOCK alone, through
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::materialise_due_transitions`].
//!
//! A mutation the service ACCEPTS rather than completes inside the call puts the
//! resource in the modelled transient status its own status enum declares for a
//! change in flight — `UPDATING` for both a knowledge base and a data source —
//! and leaves the settled status to the clock, by the same
//! `pending_status` seam a create uses. What a data source's stored
//! configuration settles at is [`settled_data_source_status`], so a create and
//! an update of one configuration reach one status.
//!
//! Both `UpdateDataSource` and `UpdateKnowledgeBase` hold members their own
//! contracts document as unchangeable once the resource exists; the one rule
//! behind those refusals lives here, in [`immutable_member_verdict`] and
//! [`immutable_member_message`], so neither handler can decide it differently.
//!
//! A delete the service accepts (`DeleteKnowledgeBase`, `DeleteDataSource`,
//! `DeleteFlow`) is what takes the resource out of those maps, so no later
//! operation can still reach it. A knowledge base and a data source carry the
//! modelled transient `DELETING` status while the removal the clock owes is
//! outstanding (
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::begin_knowledge_base_deletion`]
//! /
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::begin_data_source_deletion`]),
//! and
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::materialise_due_transitions`]
//! removes them once it comes due; a flow, whose modelled `FlowStatus` has no
//! such value, goes at once
//! ([`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::remove_flow`]).
//! A resource being deleted still reports its status to a read, and serves no
//! other operation: the `live_*` accessors here are what every operation that
//! acts ON a resource resolves it through.
//!
//! A data source's ingestion jobs (`StartIngestionJob`, `GetIngestionJob`) are
//! backed by the
//! [`ingestion_jobs`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::ingestion_jobs)
//! map, keyed by `ingestionJobId` and carrying the knowledge base and data
//! source pair it ran for, with [`IngestionJobRecord::to_ingestion_job`] as its
//! single projection. A job's status advances on the CLOCK alone, through
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::materialise_due_transitions`],
//! so every reader of the job observes the same progression. What the job DID —
//! the source documents it enumerated, the vectors it wrote and the
//! `IngestionJobStatistics` that measure them — lives in the `ingestion` module
//! beside this one.
//!
//! The documents pushed straight into a data source
//! (`IngestKnowledgeBaseDocuments`, `GetKnowledgeBaseDocuments`,
//! `ListKnowledgeBaseDocuments`, `DeleteKnowledgeBaseDocuments`) are backed by
//! the
//! [`documents`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::documents)
//! map, keyed by the knowledge base, the data source and the document's own
//! identity ([`DocumentKey`]), with [`DocumentRecord::to_detail`] and
//! [`not_found_document_detail`] as the only two projections any document
//! operation reports. A document's status advances on the CLOCK alone, through
//! the same `materialise_due_transitions` — including the chain an accepted
//! deletion starts, at the end of which the data source HOLDS the document no
//! more while its record stays behind carrying the terminal status the model
//! gives a document that was not found, so the collection a client polls
//! reports the outcome of the deletion it asked for and then converges on what
//! the data source holds. Only an identity nothing was ever stored under is
//! answered by the [`not_found_document_detail`] projection.
//!
//! A flow's aliases (`CreateFlowAlias`, `GetFlowAlias`, `ListFlowAliases`) are
//! backed by the
//! [`flow_aliases`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::flow_aliases)
//! map and live in the `flow_aliases` module beside this one, together with
//! the record -> wire projections ([`FlowAliasRecord::to_create_output`] /
//! [`FlowAliasRecord::to_get_output`] / [`FlowAliasRecord::to_summary`]) and the
//! one rule that resolves an alias's routing target against the versions its
//! flow holds. A flow carries the reserved test alias of its DRAFT version from
//! creation, stored with the flow by
//! [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::store_flow`],
//! so every reader reports it like any other alias.
//!
//! The prompt library (`CreatePrompt`, `GetPrompt`) is backed by the
//! [`prompts`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::prompts)
//! map and lives in the `prompts` module beside this one, together with its
//! record -> wire projections ([`PromptRecord::to_create_output`] /
//! [`PromptRecord::to_get_output`]) and the one resolution of a
//! `PromptIdentifier`. A prompt's working draft IS the stored record, the way a
//! flow's DRAFT version is the stored flow; the static snapshots
//! `CreatePromptVersion` cuts from that draft are held beside it in the
//! [`prompt_versions`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::prompt_versions)
//! map, keyed by the prompt and the version number, so a change to the draft
//! never moves what a numbered version reports. What `ListPrompts` enumerates
//! over those two maps, in which order, and the page it hands out live in the
//! `prompt_listing` module beside this one.
//!
//! Whether a NEW agent may be minted at all is the one thing about the agent
//! surface that is not a property of a record in the
//! [`agents`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::agents)
//! map but of the ACCOUNT making the call: that surface is closed to accounts
//! with no prior usage of it, which the
//! [`agents_classic_usage`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::agents_classic_usage)
//! record holds and the `agents_classic` module beside this one states.

/// The vector index a knowledge base is BOUND to, whichever store holds it: one
/// ingested chunk, and the read, the write and the removal every plane here performs
/// on that index.
mod bound_vector_index;

/// The flow-alias slice of this state: its records, their projections, and the
/// resolution of an alias identifier and of an alias's routing target.
mod flow_aliases;

/// What `ListFlows` enumerates — the account's flows — the order it reports them
/// in, and the page it hands out.
mod flow_listing;

/// What an ingestion job does over a data source's source, and the accounting
/// it reports for it.
mod ingestion;

/// The knowledge base's own record of what it ingested, read for a knowledge
/// base whose vector store is not one this emulator serves.
mod ingestion_record;

/// The OpenSearch Serverless vector store a knowledge base is bound to, resolved in
/// the service that owns that collection.
mod opensearch_serverless_store;

/// What a document pushed straight into a data source puts into the knowledge
/// base's bound vector index, and what takes it back out again.
mod pushed_documents;

/// How many documents ONE `IngestKnowledgeBaseDocuments` request may carry, and
/// how a refusal of a batch outside that bound quotes the documents it carried.
mod document_batch;

/// The service role a knowledge base acts as, resolved against the account's own
/// IAM records.
mod service_role;

/// The vector store a knowledge base is bound to, and the embedding model that
/// writes into it.
mod vector_store;

/// Which stored resource a tagging operation's `resourceArn` names, and the tags that
/// resource holds.
mod tagged_resources;

/// The order `ListDataSources` reports one knowledge base's data sources in, and the
/// page it hands out.
mod data_source_listing;

/// Which of one data source's ingestion jobs `ListIngestionJobs` reports, the order it
/// reports them in, and the page it hands out.
mod ingestion_job_listing;

/// A knowledge base's name as the account's own namespace: who holds one, and the
/// refusal a second knowledge base under it gets.
mod knowledge_base_names;

/// The resource-based policy a knowledge base carries: which knowledge base a
/// resource-policy operation's ARN names, and which kind of knowledge base carries
/// a policy at all.
mod resource_policies;

/// The prompt-library slice of this state: the records `CreatePrompt` stores and
/// `GetPrompt` reports, their projections, and the resolution of a prompt identifier.
mod prompts;

/// What `ListPrompts` enumerates — the account's prompts, or one prompt's versions —
/// the order it reports them in, and the page it hands out.
mod prompt_listing;

/// WHICH values this service's front door states that an enumerated type admits, and
/// in which order — the one listing both the request boundary's refusals and a
/// handler-stated discriminator clause are built from.
mod published_enum_value_sets;

pub use bound_vector_index::*;
pub use data_source_listing::*;
pub use document_batch::*;
pub use flow_aliases::*;
pub use flow_listing::*;
pub use ingestion::*;
pub use ingestion_job_listing::*;
pub use ingestion_record::*;
pub use knowledge_base_names::*;
pub use opensearch_serverless_store::*;
pub use prompt_listing::*;
pub use prompts::*;
pub use published_enum_value_sets::*;
pub use pushed_documents::*;
pub use resource_policies::*;
pub use service_role::*;
pub use tagged_resources::*;
pub use vector_store::*;

/// What an ingestion records in the bound vector index beside the vector: the published
/// knowledge-base metadata fields a retrieval reads a chunk back from.
pub mod ingestion_index;

/// Which accounts may mint a NEW agent on this plane — the surface is closed to accounts
/// with no prior usage of it — and the refusal an account outside them gets.
pub mod agents_classic;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::Document;

use crate::ops::types::{
    Agent, AgentSummary, CreateFlowOutput, CreateFlowVersionOutput, CustomDocumentIdentifier,
    CustomOrchestration, DataSource, DataSourceConfiguration, DataSourceSummary, DocumentContent,
    DocumentIdentifier,
    DocumentMetadata, FlowDefinition, FlowValidation, FlowVersionSummary, GetFlowOutput,
    GetFlowVersionOutput, GuardrailConfiguration, ImageExtractionConfiguration, IngestionJob, IngestionJobStatistics,
    KnowledgeBase, KnowledgeBaseConfiguration, KnowledgeBaseDocumentDetail, KnowledgeBaseSummary,
    MediaExtractionConfiguration, MemoryConfiguration, MetadataConfiguration,
    PromptOverrideConfiguration, S3Content,
    S3Location, ServerSideEncryptionConfiguration, StorageConfiguration, UpdateFlowOutput,
    VectorIngestionConfiguration,
};

/// The region an ARN carries when the request is unsigned, so it names no
/// credential-scope region. Matches the `agentArn` pattern's `[a-z0-9-]{1,20}`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The only agent version `CreateAgent` mints, the only flow version `CreateFlow`
/// mints and the only prompt version `CreatePrompt` mints: both the
/// `Agent$agentVersion` and the `DraftVersion` shapes are modelled
/// `pattern ^DRAFT$`, and `CreatePrompt$version` is documented "When you create a
/// prompt, the version created is the DRAFT version", so a stored agent, flow or
/// prompt has no other version until something cuts one.
pub const DRAFT_VERSION: &str = "DRAFT";

/// `CreateAgent`'s default when the request omits `idleSessionTTLInSeconds`
/// (within the member's modelled `range 60..5400`).
///
/// CHOSEN, not documented: the AWS API reference publishes only the member's
/// valid range, so this is the emulator's own default session lifetime.
pub const DEFAULT_IDLE_SESSION_TTL_SECONDS: i32 = 3600;

/// How long after `PrepareAgent` the agent leaves `PREPARING` for `PREPARED`.
///
/// CHOSEN, not documented: AWS publishes no duration for the preparation of an
/// agent. It is deliberately non-zero so a caller that reads the agent back
/// immediately still observes the transient `PREPARING` status the model
/// declares, and short enough that a client script converges promptly.
pub const PREPARE_SPAN: Duration = Duration::from_millis(500);

/// The most agents one `ListAgents` page can carry: the response member
/// `agentSummaries` is modelled `length ..10`, so a page never exceeds it however
/// large `maxResults` is.
pub const MAX_AGENTS_PER_PAGE: usize = 10;

/// The most agents this store holds; past it `CreateAgent` answers the modelled
/// `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on agents per account per Region,
/// and this emulator models neither Service Quotas nor an account's adjustments —
/// so this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it is
/// what stops the state growing without end: every record it holds is released by
/// `DeleteAgent`, which also forgets that agent's name and client tokens, so
/// neither index can outgrow this bound either.
pub const AGENT_QUOTA: usize = 1_000;

/// How many ids [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::mint_agent_id`]
/// may draw before giving up, so its termination is enforced rather than argued.
/// With a 62^10 id space and a store bounded by [`AGENT_QUOTA`] a collision is
/// already vanishingly unlikely; this caps the work regardless.
const MINT_ATTEMPTS: usize = 64;

/// The `AgentStatus` values this slice of the service can hold, named as the
/// model's `AgentStatus` enum names them on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Created but never prepared — `CreateAgent`'s result.
    NotPrepared,
    /// `PrepareAgent` accepted; the agent reaches [`AgentStatus::Prepared`] once
    /// [`PREPARE_SPAN`] has elapsed.
    Preparing,
    /// Preparation finished; the DRAFT version is servable.
    Prepared,
    /// `DeleteAgent` accepted.
    Deleting,
}

impl AgentStatus {
    /// The member value for the modelled `AgentStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::NotPrepared => "NOT_PREPARED",
            Self::Preparing => "PREPARING",
            Self::Prepared => "PREPARED",
            Self::Deleting => "DELETING",
        }
    }
}

/// The `ResourceNotFoundException` message an agent operation answers when the
/// agent the request addresses by its `agentId` is not stored: the id the request
/// carried is the one variable part, worded through the one helper that states this
/// error class ([`identified_resource_not_found_message`]).
pub fn agent_not_found_message(agent_id: &str) -> String {
    identified_resource_not_found_message("Agent", agent_id)
}

/// One stored agent: the single source of truth for every agent operation.
#[derive(Debug, Clone)]
pub struct AgentRecord {
    pub agent_id: String,
    pub agent_name: String,
    pub agent_arn: String,
    pub agent_version: String,
    pub agent_status: AgentStatus,
    /// A status the CLOCK owes this agent, and the instant it comes due. A
    /// reporting operation may materialise it once that instant has passed; it
    /// never invents a transition of its own.
    pub pending_status: Option<(SystemTime, AgentStatus)>,
    pub instruction: Option<String>,
    pub foundation_model: Option<String>,
    pub description: Option<String>,
    pub idle_session_ttl_in_seconds: i32,
    pub agent_resource_role_arn: String,
    pub customer_encryption_key_arn: Option<String>,
    pub orchestration_type: Option<String>,
    pub custom_orchestration: Option<CustomOrchestration>,
    pub agent_collaboration: Option<String>,
    pub prompt_override_configuration: Option<PromptOverrideConfiguration>,
    pub guardrail_configuration: Option<GuardrailConfiguration>,
    pub memory_configuration: Option<MemoryConfiguration>,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub prepared_at: Option<SystemTime>,
}

impl AgentRecord {
    /// Whether a preparation this agent has already accepted is still UNFINISHED:
    /// the DRAFT version is being prepared and has not reached its outcome.
    ///
    /// Asked after what the clock owes has been materialised, so a preparation
    /// whose instant has passed no longer counts as in flight.
    pub fn preparation_in_flight(&self) -> bool {
        self.agent_status == AgentStatus::Preparing
            || matches!(self.pending_status, Some((_, AgentStatus::Prepared)))
    }

    /// This record as the modelled `Agent` shape: every required member is
    /// populated from the stored record, never from the request being served.
    pub fn to_agent(&self) -> Agent {
        Agent {
            agent_id: self.agent_id.clone(),
            agent_name: self.agent_name.clone(),
            agent_arn: self.agent_arn.clone(),
            agent_version: self.agent_version.clone(),
            client_token: self.client_token.clone(),
            instruction: self.instruction.clone(),
            agent_status: self.agent_status.as_wire().to_string(),
            foundation_model: self.foundation_model.clone(),
            description: self.description.clone(),
            orchestration_type: self.orchestration_type.clone(),
            custom_orchestration: self.custom_orchestration.clone(),
            idle_session_ttl_in_seconds: self.idle_session_ttl_in_seconds,
            agent_resource_role_arn: self.agent_resource_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            prepared_at: self.prepared_at.map(to_date_time),
            failure_reasons: None,
            recommended_actions: None,
            prompt_override_configuration: self.prompt_override_configuration.clone(),
            guardrail_configuration: self.guardrail_configuration.clone(),
            memory_configuration: self.memory_configuration.clone(),
            agent_collaboration: self.agent_collaboration.clone(),
        }
    }

    /// This record projected onto the modelled `AgentSummary` shape, the
    /// `ListAgents` item.
    pub fn to_summary(&self) -> AgentSummary {
        AgentSummary {
            agent_id: self.agent_id.clone(),
            agent_name: self.agent_name.clone(),
            agent_status: self.agent_status.as_wire().to_string(),
            description: self.description.clone(),
            updated_at: to_date_time(self.updated_at),
            latest_agent_version: Some(self.agent_version.clone()),
            guardrail_configuration: self.guardrail_configuration.clone(),
        }
    }

    /// This record's position in the `ListAgents` ordering, as an opaque resume
    /// cursor: creation instant then id, so a page resumes exactly where the
    /// previous one stopped even if the agent it named has since been deleted.
    pub fn page_cursor(&self) -> String {
        format!("{}-{}", epoch_nanos(self.created_at), self.agent_id)
    }

    /// The sort key `ListAgents` orders by: creation instant, ties broken by id.
    fn sort_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.agent_id.as_str())
    }
}

/// The most flows this store holds; past it `CreateFlow` answers the modelled
/// `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on flows per account per Region,
/// and this emulator models neither Service Quotas nor an account's adjustments —
/// so this is the emulator's OWN bound on the store, set far above any workflow
/// an emulator serves, and it is what stops the flow state growing without end.
pub const FLOW_QUOTA: usize = 1_000;

/// The `includedData` value that asks for resource metadata only: the member is
/// documented "Set to METADATA_ONLY to return only resource metadata. Set to
/// ALL_DATA or omit this field to return the full response", and the flow
/// definition is the one non-metadata part of that response.
pub const INCLUDED_DATA_METADATA_ONLY: &str = "METADATA_ONLY";

/// Whether a read carrying this `includedData` is asking for the flow
/// definition: every value but [`INCLUDED_DATA_METADATA_ONLY`] is, an omitted
/// member included. One rule for every operation the member is modelled on.
pub fn included_data_carries_definition(included_data: Option<&str>) -> bool {
    included_data != Some(INCLUDED_DATA_METADATA_ONLY)
}

/// How long after `PrepareFlow` a flow leaves [`FlowStatus::Preparing`] for the
/// outcome that preparation reached.
///
/// CHOSEN, not documented: AWS publishes no duration for the preparation of a
/// flow. It is deliberately non-zero so a caller that reads the flow back
/// immediately still observes the transient `Preparing` status the modelled
/// `FlowStatus` declares ("Preparing – The flow is being prepared so that the
/// DRAFT version contains the latest changes for testing"), and short enough
/// that a client script polling the flow converges promptly.
pub const FLOW_PREPARATION_SPAN: Duration = Duration::from_secs(3);

/// The `FlowStatus` values this slice of the service can hold, named as the
/// model's `FlowStatus` enum names them on the wire.
///
/// `CreateFlow` mints only [`FlowStatus::NotPrepared`] — the operation's own
/// contract says "When you submit this request, the status will be NotPrepared" —
/// and a flow leaves it only when a later call (`PrepareFlow`, `UpdateFlow`) moves
/// it, never because somebody read it. The one status the CLOCK moves a flow to is
/// the outcome of a preparation already accepted, held in
/// [`FlowRecord::pending_preparation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStatus {
    /// Created or updated, but not prepared: `CreateFlow`'s result.
    NotPrepared,
    /// `PrepareFlow` accepted; the DRAFT version is being packaged.
    Preparing,
    /// The DRAFT version carries the latest changes.
    Prepared,
    /// The last operation invoked on the flow failed; `validations` says why.
    Failed,
}

impl FlowStatus {
    /// The member value for the modelled `FlowStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::NotPrepared => "NotPrepared",
            Self::Preparing => "Preparing",
            Self::Prepared => "Prepared",
            Self::Failed => "Failed",
        }
    }
}

/// The one `FlowStatus` a numbered version is cut from.
///
/// `CreateFlowVersion` is documented "Creates a version of the flow that you can
/// deploy", and what makes the DRAFT deployable is the preparation `PrepareFlow`
/// performs ("Prepares the DRAFT version of a flow so that it can be invoked");
/// so a flow whose preparation has not reached this status has nothing to cut a
/// deployable version from, and the call is refused with the operation's
/// modelled `ConflictException`. The status compared against it is the one the
/// flow's preparation SETTLED at ([`FlowRecord::settled_status`]), not the one
/// the clock is still reporting: a preparation the service has already decided
/// is not a reason to refuse the very next call.
pub const FLOW_STATUS_VERSIONS_ARE_CUT_FROM: FlowStatus = FlowStatus::Prepared;

/// The outcome of a preparation `PrepareFlow` has already accepted, and the
/// instant the clock makes it due.
///
/// Preparation is asynchronous — the operation answers 202 with the flow in
/// [`FlowStatus::Preparing`] — so its result is decided when the call is accepted
/// and reported once the clock reaches `due`. The findings travel with it because
/// `validations` reports "the last failed operation on the flow": until the
/// failure is due, nothing has failed.
#[derive(Debug, Clone)]
pub struct PendingPreparation {
    /// When the flow leaves [`FlowStatus::Preparing`] for [`Self::status`].
    pub due: SystemTime,
    /// The status preparation reached: [`FlowStatus::Prepared`] when the DRAFT
    /// definition carries no blocking defect, [`FlowStatus::Failed`] when it does.
    pub status: FlowStatus,
    /// The findings the flow reports once the outcome is due; empty for a
    /// preparation that succeeded.
    pub validations: Vec<FlowValidation>,
}

/// One stored flow: the single source of truth for every flow operation.
#[derive(Debug, Clone)]
pub struct FlowRecord {
    pub flow_id: String,
    pub name: String,
    pub arn: String,
    pub version: String,
    pub status: FlowStatus,
    pub description: Option<String>,
    pub execution_role_arn: String,
    pub customer_encryption_key_arn: Option<String>,
    pub definition: Option<FlowDefinition>,
    /// Validation findings from the last operation on the flow; empty while
    /// nothing has failed.
    pub validations: Vec<FlowValidation>,
    /// The preparation the CLOCK still owes this flow, if one is in flight. A
    /// reporting operation may materialise it once its instant has passed; it
    /// never invents a transition of its own.
    pub pending_preparation: Option<PendingPreparation>,
    /// The highest version number this flow has ever been cut at, `0` before the
    /// first `CreateFlowVersion`. It is a HIGH-WATER MARK, not a count of the
    /// versions still stored: `CreateFlowVersion$version` is documented
    /// "Versions are numbered incrementally, starting from 1", so deleting a
    /// version does not hand its number out a second time.
    pub versions_cut: u32,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl FlowRecord {
    /// Whether a preparation this flow has already accepted is still UNFINISHED:
    /// the DRAFT is being packaged and the outcome is not reported yet.
    ///
    /// A preparation whose instant the clock has reached is no longer in flight,
    /// and [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::materialise_due_transitions`]
    /// is what retires it, so an operation asks this after materialising what the
    /// clock owes.
    pub fn preparation_in_flight(&self) -> bool {
        self.pending_preparation.is_some() || self.status == FlowStatus::Preparing
    }

    /// The `FlowStatus` this flow's preparation has SETTLED at: the outcome a
    /// preparation already accepted reached, whether or not the clock has
    /// reported it yet, and otherwise the status the flow stands in.
    ///
    /// Preparation is decided when `PrepareFlow` accepts the call — the outcome
    /// is held in [`Self::pending_preparation`] and only its REPORTING waits for
    /// the clock — so an operation ordered behind preparation (one that needs a
    /// prepared DRAFT) asks this rather than the reported status: a flow whose
    /// preparation has succeeded serves such a call straight away, while the
    /// status a reader sees still moves `Preparing` -> outcome. The status a
    /// refusal NAMES stays the one the flow is reported in, which is what a
    /// caller sees and waits on.
    pub fn settled_status(&self) -> FlowStatus {
        self.pending_preparation
            .as_ref()
            .map_or(self.status, |pending| pending.status)
    }

    /// This record as the `CreateFlow` response: every member comes from the
    /// stored record, never from the request being served.
    pub fn to_create_output(&self) -> CreateFlowOutput {
        CreateFlowOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version.clone(),
            definition: self.definition.clone(),
        }
    }

    /// This record as the `UpdateFlow` response. `UpdateFlowResponse` carries the
    /// same members as `CreateFlowResponse`, and like it every one comes from the
    /// stored record the update has just written, never from the request.
    pub fn to_update_output(&self) -> UpdateFlowOutput {
        UpdateFlowOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version.clone(),
            definition: self.definition.clone(),
        }
    }

    /// This record as the `GetFlow` response. `with_definition` is false for a
    /// metadata-only read, which drops the flow definition and nothing else.
    ///
    /// `validations` is the flow's findings LIST, and an empty list is a value
    /// rather than an absence: the member reports what the last operation on the
    /// flow found, so a flow nothing has failed on reports that it carries no
    /// findings — `[]` — instead of leaving a polling client unable to tell "no
    /// problems" from "not reported".
    pub fn to_get_output(&self, with_definition: bool) -> GetFlowOutput {
        GetFlowOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version.clone(),
            definition: with_definition.then(|| self.definition.clone()).flatten(),
            validations: Some(self.validations.clone()),
        }
    }

    /// This flow's DRAFT version as a `ListFlowVersions` item.
    ///
    /// The DRAFT version is not a record of its own: it IS the stored flow —
    /// `CreateFlow` is documented "When you create a flow, the version created is
    /// the DRAFT version", and every later `UpdateFlow`/`PrepareFlow` moves that
    /// same DRAFT — so the summary is projected off the flow record and reports
    /// the flow's own version member, status and creation instant. A copy would
    /// go stale the moment the flow was prepared again.
    pub fn to_draft_version_summary(&self) -> FlowVersionSummary {
        FlowVersionSummary {
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            version: self.version.clone(),
        }
    }
}

/// The modelled `FlowIdentifier` pattern, quoted verbatim so a refusal can name
/// the constraint the value failed.
pub const FLOW_IDENTIFIER_PATTERN: &str =
    "^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$";

/// Whether `value` is shaped as an id this service MINTS: the modelled `Id`
/// pattern `^[0-9a-zA-Z]{10}$`, which every resource id in this slice carries —
/// a `flowId`, an `agentId` and a `knowledgeBaseId` alike.
fn is_minted_id(value: &str) -> bool {
    value.len() == ID_LEN && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Whether `position` is the one spelling a pagination token this service hands
/// out writes a number in: plain decimal digits, with no sign and no padding —
/// what `format!("{n}")` renders and nothing else.
///
/// A listing reports a position only in that form, so another spelling of the
/// same number is a token this service never handed out rather than a position
/// in the listing.
fn is_canonical_token_position(position: &str) -> bool {
    !position.is_empty()
        && (position.len() == 1 || !position.starts_with('0'))
        && position.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `position` is a position a listing that counts what it has already
/// reported hands out: [`is_canonical_token_position`], and at least one.
///
/// Such a listing starts at the position an absent `nextToken` already names, so
/// no page of one ever hands out a token for nothing reported.
fn is_counted_token_position(position: &str) -> bool {
    is_canonical_token_position(position) && position != "0"
}

/// Split a composite `nextToken` — the creation instant in nanoseconds of the
/// resource a page stopped at, then that resource's id — back into its two
/// halves, or `None` when the string is not one such a page hands out.
///
/// A creation instant is counted from the epoch, so no page ever stops at
/// position zero.
fn parse_composite_cursor(token: &str) -> Option<(u128, &str)> {
    let (position, resource_id) = token.split_once('-')?;
    if !is_counted_token_position(position) || !is_minted_id(resource_id) {
        return None;
    }
    Some((position.parse().ok()?, resource_id))
}

/// The `ValidationException` message a flow operation answers when
/// `flowIdentifier` is neither form the modelled shape admits: it names the
/// member and the constraint the value failed, with the value from the request.
pub fn flow_identifier_pattern_message(identifier: &str) -> String {
    format!(
        "Value '{identifier}' at 'flowIdentifier' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {FLOW_IDENTIFIER_PATTERN}"
    )
}

/// The `flowId` a modelled `FlowIdentifier` names, or `None` when the value is
/// neither form the shape admits.
///
/// `FlowIdentifier` is modelled [`FLOW_IDENTIFIER_PATTERN`]: either a flow ARN,
/// whose `flow/<id>` segment carries the id, or the bare id. Both forms name the
/// same flow, so both select the same stored record; anything else names no flow
/// at all and is a bad request rather than a missing one.
pub fn flow_id_from_identifier(identifier: &str) -> Option<&str> {
    let id = match identifier.strip_prefix("arn:") {
        None => identifier,
        Some(rest) => {
            // partition: aws | service | region | account | flow/<id>
            let segments: Vec<&str> = rest.split(':').collect();
            let [partition, service, region, account, resource] = segments[..] else {
                return None;
            };
            let region_shaped = (1..=20).contains(&region.len())
                && region.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                });
            if partition != "aws"
                || service != "bedrock"
                || !region_shaped
                || account.len() != 12
                || !account.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            resource.strip_prefix("flow/")?
        }
    };
    is_minted_id(id).then_some(id)
}

/// The `ResourceNotFoundException` message an operation answers when a resource
/// it was asked to resolve is not stored.
///
/// The sentence is this service's, with two variable parts: `operation`, the
/// operation the caller invoked, and `resource_type`, the type of the resource
/// that could not be resolved. The identifier that failed to resolve is not part
/// of it.
pub fn resource_not_found_message(operation: &str, resource_type: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. Failed to retrieve resource type \
         {resource_type} because it doesn't exist. Check the resource ID and retry \
         your request."
    )
}

/// The `ResourceNotFoundException` message a flow operation answers when the
/// flow the request names is not stored.
pub fn flow_not_found_message(operation: &str) -> String {
    resource_not_found_message(operation, "Flow")
}

/// The `ResourceNotFoundException` message an operation answers for a resource it
/// addresses by that resource's OWN identifier and cannot resolve.
///
/// The sentence has two variable parts and is fixed text otherwise: `resource_type`,
/// the type of the resource that could not be resolved, spelled as the model spells
/// the shape, and `resource_id`, the identifier the request carried for it. The
/// identifiers of the resources ENCLOSING it are no part of the sentence, however
/// many of them the request had to carry to address it. Stated once here so the
/// operations of this plane cannot come to word the same refusal differently.
pub fn identified_resource_not_found_message(resource_type: &str, resource_id: &str) -> String {
    format!("{resource_type} with id {resource_id} is not found.")
}

/// The version number a modelled `NumericalVersion` token counts, or `None`
/// when the token counts none.
///
/// The shape is `pattern ^[0-9]{1,5}$` and a version is held as the integer it
/// counts, so this is the one place the wire token and the stored number are
/// related: every lookup of a numbered version goes through it.
pub fn numerical_version(flow_version: &str) -> Option<u32> {
    flow_version.parse::<u32>().ok()
}

/// The `ConflictException` message an operation answers when the resource it
/// addresses is in a status that does not serve the operation at all.
///
/// The sentence is this service's, with two variable parts: `operation`, the
/// operation the caller invoked, and `status`, the status token the addressed
/// resource is currently stored in. Which statuses serve which operation is the
/// operation's own question; this is only how the refusal is worded.
pub fn resource_status_conflict_message(operation: &str, status: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. The resource that you specified is \
         currently in status {status}. Retry your request after the resource changes \
         to a valid status."
    )
}

/// The `ConflictException` message an operation answers when the resource it
/// addresses already has a change of its own in flight, so the work this call
/// asks for cannot be claimed as well.
///
/// The sentence is this service's, with ONE variable part: `operation`, the
/// operation the caller invoked. Nothing else in it comes from the request or
/// from state — the caller is told only that another request owns the resource
/// and that retrying after it finishes is what serves this one.
pub fn concurrent_update_conflict_message(operation: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. The resource is being updated by a \
         concurrent request. Retry your request after the other request has completed."
    )
}

/// The most flow versions one `ListFlowVersions` page can carry: the response
/// member `flowVersionSummaries` is modelled `length ..10`, so a page never
/// exceeds it however large `maxResults` is.
pub const MAX_FLOW_VERSIONS_PER_PAGE: usize = 10;

/// Digits a version number is written with on the wire: the modelled
/// `NumericalVersion` is `pattern ^[0-9]{1,5}$`.
const NUMERICAL_VERSION_DIGITS: usize = 5;

/// The widest version number the modelled `NumericalVersion`
/// (`pattern ^[0-9]{1,5}$`) can carry: past it the next version could not be
/// rendered on the wire at all.
pub const MAX_NUMERICAL_VERSION: u32 = 99_999;

/// The most numbered versions this store holds for one flow; past it
/// `CreateFlowVersion` answers the modelled `ServiceQuotaExceededException`
/// instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on versions per flow, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store, set far above any workflow an
/// emulator serves, and it is what stops one flow's version state growing
/// without end.
pub const FLOW_VERSION_QUOTA: usize = 1_000;

/// One stored numbered version of a flow: the single source of truth for every
/// operation that reports a version the flow was cut into.
///
/// A numbered version is a SNAPSHOT of the flow's DRAFT version at the instant
/// `CreateFlowVersion` cut it — `GetFlowVersion` reports the name, role,
/// encryption key and definition the version carries, none of which a later
/// change to the DRAFT may move — so the members that make up that snapshot are
/// stored here rather than read back off the flow.
///
/// The version number is held as the integer it counts rather than as the wire
/// string, so one flow's versions order by the sequence they were cut in
/// (`CreateFlowVersion$version` is documented "Versions are numbered
/// incrementally, starting from 1") instead of lexically, which would put `10`
/// before `9`. The modelled `NumericalVersion` is that integer rendered.
#[derive(Debug, Clone)]
pub struct FlowVersionRecord {
    /// The flow this version belongs to.
    pub flow_id: String,
    /// The ARN of that flow, as `FlowVersionSummary$arn` reports it.
    pub arn: String,
    pub version: u32,
    pub status: FlowStatus,
    /// The name the flow carried when this version was cut.
    pub name: String,
    /// The description of the version: the one the request gave, or the flow's
    /// own when it gave none.
    pub description: Option<String>,
    pub execution_role_arn: String,
    pub customer_encryption_key_arn: Option<String>,
    /// The definition this version packages, as the DRAFT held it when the
    /// version was cut.
    pub definition: Option<FlowDefinition>,
    pub created_at: SystemTime,
}

impl FlowVersionRecord {
    /// A version cut from `flow` as version `version` at `now`: the snapshot
    /// `CreateFlowVersion` stores.
    ///
    /// `description` is the request's, which the operation documents as "A
    /// description of the version of the flow"; a request that gives none leaves
    /// the version carrying the flow's own description, as the rest of the
    /// snapshot does.
    pub fn snapshot_of(
        flow: &FlowRecord,
        version: u32,
        description: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            flow_id: flow.flow_id.clone(),
            arn: flow.arn.clone(),
            version,
            // A numbered version is a snapshot of a DRAFT whose preparation
            // SUCCEEDED — that is what makes it the deployable package the
            // operation documents — so it carries the status that preparation
            // settled at and never a transient one: the version is immutable,
            // and nothing about it is still being decided.
            status: flow.settled_status(),
            name: flow.name.clone(),
            description: description.or_else(|| flow.description.clone()),
            execution_role_arn: flow.execution_role_arn.clone(),
            customer_encryption_key_arn: flow.customer_encryption_key_arn.clone(),
            definition: flow.definition.clone(),
            created_at: now,
        }
    }

    /// This record projected onto the modelled `FlowVersionSummary` shape, the
    /// `ListFlowVersions` item: every member comes from the stored record,
    /// never from the request being served.
    pub fn to_summary(&self) -> FlowVersionSummary {
        FlowVersionSummary {
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            version: self.version.to_string(),
        }
    }

    /// This record as the `CreateFlowVersion` response: every member comes from
    /// the snapshot just stored, never from the request being served.
    pub fn to_create_output(&self) -> CreateFlowVersionOutput {
        CreateFlowVersionOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            version: self.version.to_string(),
            definition: self.definition.clone(),
        }
    }

    /// This record as the `GetFlowVersion` response: the snapshot the version was
    /// cut as, reported from the store rather than read back off the flow, so a
    /// later change to the DRAFT never moves what a numbered version reports.
    ///
    /// `with_definition` is false for a metadata-only read, which drops the flow
    /// definition and nothing else.
    pub fn to_get_output(&self, with_definition: bool) -> GetFlowVersionOutput {
        GetFlowVersionOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            version: self.version.to_string(),
            definition: with_definition.then(|| self.definition.clone()).flatten(),
        }
    }
}

/// One position in a listing of ONE resource's versions: the resource's DRAFT
/// version, which such a listing reports first, or one of its numbered
/// versions. Shared by every version listing of this service, so no two of them
/// can order or resume their versions differently.
///
/// The derived ordering IS the listing's order — `Draft` is declared first, so
/// it precedes every numbered version, and numbered versions compare by the
/// sequence they were cut in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VersionCursor {
    /// The resource's own DRAFT version.
    Draft,
    /// The numbered version this many versions into the resource's history.
    Numbered(u32),
}

impl VersionCursor {
    /// The position a version listing's page resumes after, or `None` when the
    /// token is not one such a listing handed out.
    ///
    /// A page's `nextToken` is the version the page stopped at, so the next page
    /// starts at the first version of the same resource above it — stable even
    /// if the version the token names has since been deleted.
    ///
    /// Versions are numbered from 1, so a page never hands out a token that
    /// starts with `0`: neither `0` itself nor a zero-padded number names a
    /// position in this listing.
    pub fn parse(token: &str) -> Option<Self> {
        if token == DRAFT_VERSION {
            return Some(Self::Draft);
        }
        let numeric = (1..=NUMERICAL_VERSION_DIGITS).contains(&token.len())
            && is_counted_token_position(token);
        numeric
            .then(|| numerical_version(token))
            .flatten()
            .map(Self::Numbered)
    }

    /// This position as the `nextToken` a page hands out: the `version` member
    /// of the summary the page stopped at.
    pub fn token(self) -> String {
        match self {
            Self::Draft => DRAFT_VERSION.to_string(),
            Self::Numbered(version) => version.to_string(),
        }
    }
}

/// The most knowledge bases this store holds; past it `CreateKnowledgeBase`
/// answers the modelled `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on knowledge bases per account
/// per Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store, set far above
/// any workflow an emulator serves, and it is what stops the knowledge-base
/// state growing without end.
pub const KNOWLEDGE_BASE_QUOTA: usize = 1_000;

/// How long after `CreateKnowledgeBase` a knowledge base leaves `CREATING` for
/// `ACTIVE`.
///
/// CHOSEN, not documented: AWS publishes no duration for knowledge-base
/// creation. It is deliberately non-zero so a caller that reads the knowledge
/// base back immediately still observes the transient `CREATING` status the
/// modelled `KnowledgeBaseStatus` declares ("CREATING – The knowledge base is
/// being created"), and short enough that a client script converges promptly.
pub const KNOWLEDGE_BASE_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// How long after a delete is accepted the resource it named is removed from
/// the store, after which a read of it answers the modelled
/// `ResourceNotFoundException` rather than reporting it as `DELETING`.
///
/// CHOSEN, not documented: AWS publishes no duration for the deletion of a
/// knowledge base or of a data source. It is deliberately non-zero, and a few
/// seconds rather than a few milliseconds, so a caller that reads the resource
/// back immediately still observes the transient status both modelled shapes
/// document (`KnowledgeBase$status`: "DELETING – The knowledge base is being
/// deleted"; `DataSource$status`: "Deleting – The data source is being
/// deleted") and can act on it, while a client polling for the resource to
/// disappear still converges promptly.
pub const DELETION_SPAN: Duration = Duration::from_secs(3);

/// The `KnowledgeBaseType` discriminator of a self-managed vector knowledge
/// base: the one `CreateKnowledgeBase` documents a vector store for ("For
/// self-managed knowledge bases, provide the configuration for your vector
/// store in the storageConfiguration object").
pub const VECTOR_KNOWLEDGE_BASE_TYPE: &str = "VECTOR";

/// Every modelled `KnowledgeBaseType` value, paired with the
/// `KnowledgeBaseConfiguration` member that carries that kind's details.
///
/// Source: the `KnowledgeBaseType` enum and the `KnowledgeBaseConfiguration`
/// structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="KnowledgeBaseType,KnowledgeBaseConfiguration")`).
pub const KNOWLEDGE_BASE_CONFIGURATION_MEMBERS: [(&str, &str); 4] = [
    ("VECTOR", "vectorKnowledgeBaseConfiguration"),
    ("KENDRA", "kendraKnowledgeBaseConfiguration"),
    ("SQL", "sqlKnowledgeBaseConfiguration"),
    ("MANAGED", "managedKnowledgeBaseConfiguration"),
];

/// Why a `knowledgeBaseConfiguration` (with its `storageConfiguration`) cannot
/// back a knowledge base. Each variant is a bad request, answered with the
/// modelled `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeBaseConfigurationProblem {
    /// `type` is none of the modelled `KnowledgeBaseType` values.
    UnknownType(String),
    /// `type` names a kind whose own configuration member was not supplied.
    MissingConfigurationMember {
        knowledge_base_type: String,
        member: &'static str,
    },
    /// A self-managed vector knowledge base with no vector store configured.
    MissingStorageConfiguration,
}

impl KnowledgeBaseConfigurationProblem {
    /// The refusal's message: it names the member and the constraint the
    /// request failed, so it is built from the request rather than quoted.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownType(value) => {
                let admitted: Vec<&str> = KNOWLEDGE_BASE_CONFIGURATION_MEMBERS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect();
                format!(
                    "Value '{value}' at 'knowledgeBaseConfiguration.type' failed to satisfy \
                     constraint: Member must satisfy enum value set: [{}]",
                    stated_value_set(&admitted)
                )
            }
            Self::MissingConfigurationMember {
                knowledge_base_type,
                member,
            } => format!(
                "knowledgeBaseConfiguration.{member} is required for a \
                 knowledgeBaseConfiguration.type of {knowledge_base_type}"
            ),
            Self::MissingStorageConfiguration => format!(
                "You must provide a storage configuration if you create a \
                 {VECTOR_KNOWLEDGE_BASE_TYPE} knowledge base."
            ),
        }
    }
}

/// Whether `configuration` names a kind this service can store, and whether the
/// request supplied everything that kind needs. `None` when it did.
///
/// The `type` member is a discriminator: it names which of
/// `KnowledgeBaseConfiguration`'s sibling members carries that kind's details,
/// and for a self-managed vector knowledge base it also makes
/// `storageConfiguration` — the vector store — a requirement, which a managed,
/// Kendra or SQL knowledge base has no use for.
pub fn knowledge_base_configuration_problem(
    configuration: &KnowledgeBaseConfiguration,
    storage_configuration: Option<&StorageConfiguration>,
) -> Option<KnowledgeBaseConfigurationProblem> {
    let knowledge_base_type = configuration.r#type.as_str();
    let member = KNOWLEDGE_BASE_CONFIGURATION_MEMBERS
        .iter()
        .find(|(name, _)| *name == knowledge_base_type)
        .map(|(_, member)| *member);
    let Some(member) = member else {
        return Some(KnowledgeBaseConfigurationProblem::UnknownType(
            knowledge_base_type.to_string(),
        ));
    };

    let supplied = match knowledge_base_type {
        "VECTOR" => configuration.vector_knowledge_base_configuration.is_some(),
        "KENDRA" => configuration.kendra_knowledge_base_configuration.is_some(),
        "SQL" => configuration.sql_knowledge_base_configuration.is_some(),
        _ => configuration.managed_knowledge_base_configuration.is_some(),
    };
    if !supplied {
        return Some(
            KnowledgeBaseConfigurationProblem::MissingConfigurationMember {
                knowledge_base_type: knowledge_base_type.to_string(),
                member,
            },
        );
    }

    if knowledge_base_type == VECTOR_KNOWLEDGE_BASE_TYPE && storage_configuration.is_none() {
        return Some(KnowledgeBaseConfigurationProblem::MissingStorageConfiguration);
    }

    None
}

/// The `KnowledgeBaseStatus` values this slice of the service can hold, named as
/// the model's `KnowledgeBaseStatus` enum names them on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeBaseStatus {
    /// `CreateKnowledgeBase` accepted; the knowledge base reaches
    /// [`KnowledgeBaseStatus::Active`] once [`KNOWLEDGE_BASE_ACTIVATION_SPAN`]
    /// has elapsed.
    Creating,
    /// The knowledge base is ready to be queried.
    Active,
    /// `UpdateKnowledgeBase` accepted, the change not applied yet — the modelled
    /// `KnowledgeBase$status` documents this as the status of a knowledge base
    /// that "is being updated". It reaches [`KnowledgeBaseStatus::Active`] again
    /// once [`KNOWLEDGE_BASE_ACTIVATION_SPAN`] has elapsed.
    Updating,
    /// `DeleteKnowledgeBase` accepted; the knowledge base is removed from the
    /// store once [`DELETION_SPAN`] has elapsed.
    Deleting,
}

impl KnowledgeBaseStatus {
    /// The member value for the modelled `KnowledgeBaseStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored knowledge base: the single source of truth for every
/// knowledge-base operation.
#[derive(Debug, Clone)]
pub struct KnowledgeBaseRecord {
    pub knowledge_base_id: String,
    pub name: String,
    pub knowledge_base_arn: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub knowledge_base_configuration: KnowledgeBaseConfiguration,
    pub storage_configuration: Option<StorageConfiguration>,
    pub status: KnowledgeBaseStatus,
    /// A status the CLOCK owes this knowledge base, and the instant it comes
    /// due. A reporting operation may materialise it once that instant has
    /// passed; it never invents a transition of its own.
    pub pending_status: Option<(SystemTime, KnowledgeBaseStatus)>,
    /// The instant the CLOCK owes this knowledge base's removal, set by the
    /// delete that was accepted for it and `None` while none was.
    pub removal_due_at: Option<SystemTime>,
    /// Reasons the last API operation on the knowledge base failed; empty while
    /// nothing has failed.
    pub failure_reasons: Vec<String>,
    /// The custom metadata field configurations this knowledge base carries, and
    /// `None` while it carries none.
    ///
    /// Held as the modelled list itself rather than as a flattened one, so a read
    /// reports back exactly what the last write stored: a knowledge base no write
    /// has supplied configurations for carries none, and one a write supplied an
    /// empty list for carries an empty list.
    pub metadata_configurations: Option<Vec<MetadataConfiguration>>,
    /// The resource-based POLICY this knowledge base carries, and `None` while it
    /// carries none.
    ///
    /// It lives on the knowledge base's own record rather than in a map beside it
    /// because a resource policy is an attribute OF the knowledge base: the one a
    /// resource-policy read reports is the one the write attached to this very
    /// record, and a knowledge base that leaves the store takes its policy with it
    /// instead of leaving one behind for a later knowledge base to find. See
    /// [`resource_policies`].
    pub resource_policy: Option<ResourcePolicyRecord>,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl KnowledgeBaseRecord {
    /// Whether a delete has already been accepted for this knowledge base, so
    /// it is on its way out of the store.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// This record as the modelled `KnowledgeBase` shape: every member is
    /// populated from the stored record, never from the request being served.
    pub fn to_knowledge_base(&self) -> KnowledgeBase {
        KnowledgeBase {
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            description: self.description.clone(),
            role_arn: self.role_arn.clone(),
            knowledge_base_configuration: self.knowledge_base_configuration.clone(),
            storage_configuration: self.storage_configuration.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            failure_reasons: (!self.failure_reasons.is_empty())
                .then(|| self.failure_reasons.clone()),
            metadata_configurations: self.metadata_configurations.clone(),
        }
    }

    /// This record as the modelled `KnowledgeBaseSummary` shape, which is what
    /// `ListKnowledgeBases` reports: every member comes from the same stored
    /// value the full projection reports it from.
    pub fn to_summary(&self) -> KnowledgeBaseSummary {
        KnowledgeBaseSummary {
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            status: self.status.as_wire().to_string(),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// The total order `ListKnowledgeBases` pages over: when the knowledge base
    /// was created, ties broken by its minted id.
    ///
    /// A knowledge base's creation instant and id never change, so this order is
    /// stable — which is what lets a page resume exactly after the one before
    /// it.
    fn sort_key(&self) -> (u128, &str) {
        (
            epoch_nanos(self.created_at),
            self.knowledge_base_id.as_str(),
        )
    }

    /// The `nextToken` a page ending at this knowledge base hands out.
    pub fn page_cursor(&self) -> String {
        let (created_at_nanos, knowledge_base_id) = self.sort_key();
        format!("{created_at_nanos}-{knowledge_base_id}")
    }
}

/// The most knowledge bases one `ListKnowledgeBases` page reports when the
/// request names no `maxResults`.
///
/// CHOSEN, not documented: AWS publishes no default page size for the listing.
/// It is the upper bound of the modelled `maxResults` range (1..1000,
/// `smithy_query(query="operation",operation="ListKnowledgeBases")`), so
/// `maxResults` alone decides how short a page is, and it matches
/// [`KNOWLEDGE_BASE_QUOTA`]: an account holds no more knowledge bases than one
/// default page can carry.
pub const MAX_KNOWLEDGE_BASES_PER_PAGE: usize = KNOWLEDGE_BASE_QUOTA;

/// A `ListKnowledgeBases` resume cursor, decoded back into the position in the
/// listing's order that it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeBasePageCursor {
    created_at_nanos: u128,
    knowledge_base_id: String,
}

impl KnowledgeBasePageCursor {
    /// Decode a `nextToken` a previous page handed out. `None` when the token
    /// was not produced by [`KnowledgeBaseRecord::page_cursor`] — only the one
    /// spelling that function writes names a position.
    pub fn parse(token: &str) -> Option<Self> {
        let (created_at_nanos, knowledge_base_id) = parse_composite_cursor(token)?;
        Some(Self {
            created_at_nanos,
            knowledge_base_id: knowledge_base_id.to_string(),
        })
    }

    /// Whether `record` sorts after this cursor, i.e. belongs on the next page.
    fn precedes(&self, record: &KnowledgeBaseRecord) -> bool {
        (self.created_at_nanos, self.knowledge_base_id.as_str()) < record.sort_key()
    }
}

/// The `ResourceNotFoundException` message a knowledge-base operation answers
/// when the knowledge base the request names is not stored: the id the request
/// carried fills the one variable part.
pub fn knowledge_base_not_found_message(knowledge_base_id: &str) -> String {
    identified_resource_not_found_message("KnowledgeBase", knowledge_base_id)
}

/// The most data sources one knowledge base holds; past it `CreateDataSource`
/// answers the modelled `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on data sources per knowledge
/// base, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store, set far above
/// any workflow an emulator serves, and it is what stops the data-source state
/// growing without end.
pub const DATA_SOURCE_QUOTA: usize = 1_000;

/// The `KnowledgeBaseType` of a service-managed knowledge base: the one kind
/// whose data sources have a documented `dataDeletionPolicy` default, and the one
/// kind that carries a resource-based policy ([`resource_policies`]).
pub const MANAGED_KNOWLEDGE_BASE_TYPE: &str = "MANAGED";

/// The `dataDeletionPolicy` a data source of a service-managed knowledge base
/// takes when the request names none.
///
/// Documented: `CreateDataSource`'s `dataDeletionPolicy` says "For managed
/// knowledge bases, the only supported option is DELETE, which is also the
/// default". No default is published for any other kind, so a data source of
/// one keeps whatever the request supplied — including nothing.
pub const MANAGED_DATA_DELETION_POLICY: &str = "DELETE";

/// Every modelled `DataSourceType` value, paired with the
/// `DataSourceConfiguration` member that carries that kind's connection
/// details — `None` for the kinds the model gives no member for, which
/// therefore need none.
///
/// Source: the `DataSourceType` enum and the `DataSourceConfiguration`
/// structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="DataSourceType,DataSourceConfiguration")`).
pub const DATA_SOURCE_CONFIGURATION_MEMBERS: [(&str, Option<&str>); 8] = [
    ("S3", Some("s3Configuration")),
    ("WEB", Some("webConfiguration")),
    ("CONFLUENCE", Some("confluenceConfiguration")),
    ("SALESFORCE", Some("salesforceConfiguration")),
    ("SHAREPOINT", Some("sharePointConfiguration")),
    (
        "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
        Some("managedKnowledgeBaseConnectorConfiguration"),
    ),
    ("CUSTOM", None),
    ("REDSHIFT_METADATA", None),
];

/// Why a `dataSourceConfiguration` cannot back a data source. Each variant is a
/// bad request, answered with the modelled `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSourceConfigurationProblem {
    /// `type` is none of the modelled `DataSourceType` values.
    UnknownType(String),
    /// `type` names a kind whose own connection member was not supplied.
    MissingConfigurationMember {
        data_source_type: String,
        member: &'static str,
    },
}

impl DataSourceConfigurationProblem {
    /// The refusal's message: it names the member and the constraint the
    /// request failed, so it is built from the request rather than quoted.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownType(value) => {
                let admitted: Vec<&str> = DATA_SOURCE_CONFIGURATION_MEMBERS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect();
                format!(
                    "Value '{value}' at 'dataSourceConfiguration.type' failed to satisfy \
                     constraint: Member must satisfy enum value set: [{}]",
                    stated_value_set(&admitted)
                )
            }
            Self::MissingConfigurationMember {
                data_source_type,
                member,
            } => format!(
                "dataSourceConfiguration.{member} is required for a \
                 dataSourceConfiguration.type of {data_source_type}"
            ),
        }
    }
}

/// Whether `configuration` names a kind this service can store, and whether the
/// request supplied the connection details that kind needs. `None` when it did.
///
/// The `type` member is a discriminator: it names which of
/// `DataSourceConfiguration`'s sibling members carries that kind's connection
/// details, and the kinds the model pairs with no member need none.
pub fn data_source_configuration_problem(
    configuration: &DataSourceConfiguration,
) -> Option<DataSourceConfigurationProblem> {
    let data_source_type = configuration.r#type.as_str();
    let Some((_, member)) = DATA_SOURCE_CONFIGURATION_MEMBERS
        .iter()
        .find(|(name, _)| *name == data_source_type)
    else {
        return Some(DataSourceConfigurationProblem::UnknownType(
            data_source_type.to_string(),
        ));
    };
    // The kinds the model pairs with no sibling member need none.
    let member = (*member)?;

    let supplied = match data_source_type {
        "S3" => configuration.s3_configuration.is_some(),
        "WEB" => configuration.web_configuration.is_some(),
        "CONFLUENCE" => configuration.confluence_configuration.is_some(),
        "SALESFORCE" => configuration.salesforce_configuration.is_some(),
        "SHAREPOINT" => configuration.share_point_configuration.is_some(),
        _ => configuration
            .managed_knowledge_base_connector_configuration
            .is_some(),
    };
    if supplied {
        return None;
    }
    Some(DataSourceConfigurationProblem::MissingConfigurationMember {
        data_source_type: data_source_type.to_string(),
        member,
    })
}

/// The `connectorParameters` member naming which connector a managed knowledge
/// base connector data source uses.
pub const CONNECTOR_TYPE_PARAMETER: &str = "type";

/// The `connectorParameters` member naming the version of the parameter set the
/// rest of the document is written against.
pub const CONNECTOR_VERSION_PARAMETER: &str = "version";

/// The `connectorParameters` member carrying the coordinates of the source a
/// managed connector reads from.
pub const CONNECTOR_CONNECTION_PARAMETER: &str = "connectionConfiguration";

/// The one managed connector that reads from no source of its own: its
/// documents are pushed into the knowledge base through the service's own
/// document API instead of being taken from somewhere the service has to reach.
pub const CUSTOM_CONNECTOR: &str = "CUSTOM";

/// The `connectorParameters` member saying whether the connector carries the
/// source's access-control lists into the knowledge base.
pub const CONNECTOR_ACL_ENABLED_PARAMETER: &str = "aclEnabled";

/// The value [`CONNECTOR_ACL_ENABLED_PARAMETER`] is stored with when the
/// request leaves it unset.
///
/// RECORDED, not documented: the API reference documents `connectorParameters`
/// as connector-specific without publishing its defaults, so this is the value
/// the service itself completes the parameters with.
pub const CONNECTOR_ACL_ENABLED_DEFAULT: bool = false;

/// The `connectorParameters` member holding the connector's file filter.
pub const CONNECTOR_FILTER_PARAMETER: &str = "filterConfiguration";

/// The filter member bounding the size of a file the connector takes in.
pub const CONNECTOR_MAX_FILE_SIZE_PARAMETER: &str = "maxFileSizeInMegaBytes";

/// The bound [`CONNECTOR_MAX_FILE_SIZE_PARAMETER`] is stored with when the
/// request leaves it unset, in megabytes and spelled as the parameter document
/// spells its numbers.
///
/// RECORDED, not documented, as [`CONNECTOR_ACL_ENABLED_DEFAULT`].
pub const CONNECTOR_MAX_FILE_SIZE_DEFAULT: &str = "500";

/// The `imageExtractionStatus` a managed connector's
/// `mediaExtractionConfiguration` takes when the request configures no media
/// extraction of its own: a modelled `EnabledOrDisabledState` value
/// (`smithy_query(query="shape",shape="EnabledOrDisabledState")`).
///
/// RECORDED, not documented, as [`CONNECTOR_ACL_ENABLED_DEFAULT`].
pub const IMAGE_EXTRACTION_STATUS_DEFAULT: &str = "ENABLED";

/// Complete `configuration` with the connector defaults the service applies to
/// the members the request left unset, so what is STORED is the data source's
/// EFFECTIVE configuration and every reader reports the same one.
///
/// Only a managed knowledge base connector has them: it is the service, not the
/// caller, that runs the connector, so the service completes the caller's
/// connector parameters rather than taking them as the whole configuration. A
/// member the request did supply is left exactly as it wrote it, and a
/// self-managed configuration is not the service's to complete.
///
/// The completed parameters are stored — and so reported — as the JSON TEXT of
/// the document, which is the form the service carries them in: they are the
/// connector's own parameter document rather than a modelled structure, so what
/// crosses the wire is that document, not an inlining of its members.
pub fn complete_connector_defaults(configuration: &mut DataSourceConfiguration) {
    let Some(connector) = configuration
        .managed_knowledge_base_connector_configuration
        .as_mut()
    else {
        return;
    };

    // The connector extracts media from the files it takes in, so a request
    // that configures none still gets the configuration the connector runs
    // with.
    let media = connector
        .media_extraction_configuration
        .get_or_insert(MediaExtractionConfiguration {
            image_extraction_configuration: None,
            audio_extraction_configuration: None,
            video_extraction_configuration: None,
        });
    media
        .image_extraction_configuration
        .get_or_insert(ImageExtractionConfiguration {
            image_extraction_status: IMAGE_EXTRACTION_STATUS_DEFAULT.to_string(),
        });

    // `connectorParameters` is a free-form document, so the defaults are
    // completed member by member: nothing the request wrote is replaced, and
    // parameters are not invented for a connector the request named none for.
    let Some(parameters) = connector.connector_parameters.as_ref() else {
        return;
    };
    let Some(supplied) = connector_parameter_members(parameters) else {
        return;
    };
    connector.connector_parameters = Some(Document::String(completed_parameters_text(&supplied)));
}

/// The text a connector's parameter document is reported as: the members the
/// request supplied, completed with the defaults the service applies to the
/// ones it left unset.
///
/// The form is RECORDED, not documented: AWS publishes no serialization for
/// this document, and its responses carry the connector discriminator first and
/// the parameter-set version last, the parameters the request supplied before
/// the ones the service completed, each group in reverse member order. A
/// `Document::Object` is a `HashMap`, whose own iteration order says nothing, so
/// the text is built from that order alone — one parameter document therefore
/// has exactly one text, and a caller that diffs a read against an earlier one,
/// or hashes it to detect drift, sees a difference only where the document
/// itself differs.
fn completed_parameters_text(supplied: &HashMap<String, Document>) -> String {
    let mut members: Vec<String> = Vec::new();

    // The discriminator opens the document.
    if let Some(connector) = supplied.get(CONNECTOR_TYPE_PARAMETER) {
        members.push(member_text(
            CONNECTOR_TYPE_PARAMETER,
            document_json_text(connector),
        ));
    }

    // Then the parameters the request supplied, the file filter completed with
    // its bound.
    let edges = [CONNECTOR_TYPE_PARAMETER, CONNECTOR_VERSION_PARAMETER];
    for (name, value) in group_order(
        supplied
            .iter()
            .filter(|(name, _)| !edges.contains(&name.as_str())),
    ) {
        let text = if name == CONNECTOR_FILTER_PARAMETER {
            completed_filter_text(value)
        } else {
            document_json_text(value)
        };
        members.push(member_text(name, text));
    }

    // Then the parameters the service completes for a request that named none.
    let mut completed: Vec<(&str, String)> = Vec::new();
    if !supplied.contains_key(CONNECTOR_ACL_ENABLED_PARAMETER) {
        completed.push((
            CONNECTOR_ACL_ENABLED_PARAMETER,
            document_json_text(&Document::Bool(CONNECTOR_ACL_ENABLED_DEFAULT)),
        ));
    }
    if !supplied.contains_key(CONNECTOR_FILTER_PARAMETER) {
        completed.push((
            CONNECTOR_FILTER_PARAMETER,
            completed_filter_text(&Document::Object(HashMap::new())),
        ));
    }
    completed.sort_by(|left, right| right.0.cmp(left.0));
    for (name, text) in completed {
        members.push(member_text(name, text));
    }

    // And the parameter-set version closes it.
    if let Some(version) = supplied.get(CONNECTOR_VERSION_PARAMETER) {
        members.push(member_text(
            CONNECTOR_VERSION_PARAMETER,
            document_json_text(version),
        ));
    }

    format!("{{{}}}", members.join(","))
}

/// The connector's file filter as its text, completed with the file-size bound
/// the connector applies: the members the request supplied, and the bound
/// closing the filter whether the request named it or the service completed it.
fn completed_filter_text(filter: &Document) -> String {
    let Document::Object(supplied) = filter else {
        // A filter that is not a set of members is not the service's to
        // complete, so it is reported as the request wrote it.
        return document_json_text(filter);
    };
    let mut members: Vec<String> = group_order(
        supplied
            .iter()
            .filter(|(name, _)| name.as_str() != CONNECTOR_MAX_FILE_SIZE_PARAMETER),
    )
    .into_iter()
    .map(|(name, value)| member_text(name, document_json_text(value)))
    .collect();
    let bound = supplied
        .get(CONNECTOR_MAX_FILE_SIZE_PARAMETER)
        .cloned()
        .unwrap_or_else(|| Document::String(CONNECTOR_MAX_FILE_SIZE_DEFAULT.to_string()));
    members.push(member_text(
        CONNECTOR_MAX_FILE_SIZE_PARAMETER,
        document_json_text(&bound),
    ));
    format!("{{{}}}", members.join(","))
}

/// One group of a document's members, in the order the group is emitted in.
fn group_order<'a>(
    members: impl Iterator<Item = (&'a String, &'a Document)>,
) -> Vec<(&'a String, &'a Document)> {
    let mut members: Vec<(&String, &Document)> = members.collect();
    members.sort_by(|(left, _), (right, _)| right.cmp(left));
    members
}

/// One member of a JSON object: its name, quoted, and its value's text.
fn member_text(name: &str, value: String) -> String {
    format!("{}:{value}", ::serde_json::Value::String(name.to_string()))
}

/// The connector parameters as their member map, whether the request carried
/// them inlined as a document object or as the JSON text the service itself
/// reports them as — so a caller may put back exactly what it read. `None` when
/// they are neither, in which case there are no members to complete and the
/// request's value stands.
///
/// The text is parsed by `serde_json`, which refuses input nested past its own
/// recursion limit, so a caller's text cannot drive this walk arbitrarily deep.
fn connector_parameter_members(parameters: &Document) -> Option<HashMap<String, Document>> {
    match parameters {
        Document::Object(members) => Some(members.clone()),
        Document::String(text) => match ::serde_json::from_str::<::serde_json::Value>(text) {
            Ok(::serde_json::Value::Object(members)) => Some(
                members
                    .iter()
                    .map(|(name, value)| (name.clone(), document_of_json(value)))
                    .collect(),
            ),
            _ => None,
        },
        _ => None,
    }
}

/// A parsed JSON value as the Smithy document holding the same value.
fn document_of_json(value: &::serde_json::Value) -> Document {
    use ::aws_smithy_types::Number;

    match value {
        ::serde_json::Value::Null => Document::Null,
        ::serde_json::Value::Bool(flag) => Document::Bool(*flag),
        ::serde_json::Value::Number(number) => {
            if let Some(number) = number.as_u64() {
                Document::Number(Number::PosInt(number))
            } else if let Some(number) = number.as_i64() {
                Document::Number(Number::NegInt(number))
            } else {
                Document::Number(Number::Float(number.as_f64().unwrap_or_default()))
            }
        }
        ::serde_json::Value::String(text) => Document::String(text.clone()),
        ::serde_json::Value::Array(elements) => {
            Document::Array(elements.iter().map(document_of_json).collect())
        }
        ::serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), document_of_json(member)))
                .collect(),
        ),
    }
}

/// `document` as the compact JSON text the connector parameters are reported as.
///
/// An object's members are emitted in the order [`group_order`] gives them, and
/// nothing carries insignificant whitespace, so one document has exactly one
/// text. (A `Document::Object` is a `HashMap`, whose own iteration order says
/// nothing.)
///
/// The walk follows the document's own nesting, which is bounded before it gets
/// here: a document arrives either through the request deserializer or through
/// [`connector_parameter_members`], and both refuse input nested past a fixed
/// depth, so a caller cannot choose how deep this descends.
fn document_json_text(document: &Document) -> String {
    use ::aws_smithy_types::Number;

    match document {
        Document::Null => "null".to_string(),
        Document::Bool(flag) => flag.to_string(),
        Document::Number(Number::PosInt(number)) => number.to_string(),
        Document::Number(Number::NegInt(number)) => number.to_string(),
        Document::Number(Number::Float(number)) => ::serde_json::Value::from(*number).to_string(),
        Document::String(text) => ::serde_json::Value::String(text.clone()).to_string(),
        Document::Array(elements) => {
            let elements: Vec<String> = elements.iter().map(document_json_text).collect();
            format!("[{}]", elements.join(","))
        }
        Document::Object(members) => {
            let members: Vec<String> = group_order(members.iter())
                .into_iter()
                .map(|(name, member)| member_text(name, document_json_text(member)))
                .collect();
            format!("{{{}}}", members.join(","))
        }
    }
}

/// Whether the data source `configuration` describes has a source OUTSIDE this
/// service that the service itself has to reach before the data source can
/// serve ingestion.
///
/// A managed knowledge base connector is run BY the service: it connects the
/// knowledge base to the source its `connectorParameters` name — the
/// `connectionConfiguration` parameter carries that source's coordinates — so
/// the connection has to come up before the data source is ready. The `CUSTOM`
/// connector names no source, since its documents are pushed in through the
/// service's own document API, and a self-managed configuration is read by an
/// ingestion job rather than connected to when the data source is created.
pub fn connects_to_an_external_source(configuration: &DataSourceConfiguration) -> bool {
    let Some(connector) = configuration
        .managed_knowledge_base_connector_configuration
        .as_ref()
    else {
        return false;
    };
    let Some(parameters) = connector
        .connector_parameters
        .as_ref()
        .and_then(connector_parameter_members)
    else {
        return false;
    };
    if parameters.contains_key(CONNECTOR_CONNECTION_PARAMETER) {
        return true;
    }
    match parameters.get(CONNECTOR_TYPE_PARAMETER) {
        Some(Document::String(connector)) => connector != CUSTOM_CONNECTOR,
        _ => false,
    }
}

/// The `DataSourceType` of a data source the SERVICE connects to its source: a
/// managed knowledge base connector, whose `connectorParameters` name which
/// connector it runs.
pub const MANAGED_CONNECTOR_DATA_SOURCE_TYPE: &str = "MANAGED_KNOWLEDGE_BASE_CONNECTOR";

/// Which connector a managed knowledge base connector data source runs, from
/// the `type` member of its `connectorParameters`; `None` when its parameters
/// name none.
fn connector_kind(configuration: &DataSourceConfiguration) -> Option<String> {
    let parameters = configuration
        .managed_knowledge_base_connector_configuration
        .as_ref()?
        .connector_parameters
        .as_ref()
        .and_then(connector_parameter_members)?;
    match parameters.get(CONNECTOR_TYPE_PARAMETER) {
        Some(Document::String(connector)) => Some(connector.clone()),
        _ => None,
    }
}

/// The kind of document CONTENT a data source holds — one of the modelled
/// `ContentDataSourceType` values — or `None` for a data source that holds no
/// directly ingestible document at all.
///
/// A data source's own `DataSourceType` and a document's
/// `ContentDataSourceType` are two different enums: the content kind is the kind
/// of SOURCE the document comes from, and a managed knowledge base connector
/// data source is the connector its `connectorParameters` name rather than a
/// kind of source of its own — so its content kind is that connector's. For
/// every other data source the two enums spell the same kind, and a kind no
/// `ContentDataSourceType` value names (a crawler over a source of its own) is
/// a data source no document's content can correspond to.
///
/// Source: the `ContentDataSourceType` and `DataSourceType` enums and the
/// `DataSourceConfiguration` structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="ContentDataSourceType,DataSourceType,DataSourceConfiguration")`).
pub fn ingestible_content_type(configuration: &DataSourceConfiguration) -> Option<&'static str> {
    let kind = if configuration.r#type == MANAGED_CONNECTOR_DATA_SOURCE_TYPE {
        connector_kind(configuration)?
    } else {
        configuration.r#type.clone()
    };
    DOCUMENT_CONTENT_MEMBERS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(name, _)| *name)
}

/// How long after `CreateDataSource` a data source whose connector has a source
/// outside the service to reach leaves `CREATING` for the outcome of bringing
/// that connection up, and how long after `UpdateDataSource` a data source
/// leaves `UPDATING` for the outcome of the configuration the update stored.
///
/// CHOSEN, not documented: AWS publishes no duration for the creation or the
/// update of a data source. It is deliberately non-zero so a caller that reads
/// the data source back immediately still observes the transient `CREATING` or
/// `UPDATING` status the modelled `DataSourceStatus` declares, and it is the
/// span a knowledge base's own creation takes
/// ([`KNOWLEDGE_BASE_ACTIVATION_SPAN`]), so a client script converges just as
/// promptly.
pub const DATA_SOURCE_VALIDATION_SPAN: Duration = KNOWLEDGE_BASE_ACTIVATION_SPAN;

/// The `DataSourceStatus` values this slice of the service can hold, named as
/// the model's `DataSourceStatus` enum names them on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceStatus {
    /// The data source is stored and the connection to the source its connector
    /// names is being brought up; the clock owes it the outcome of that once
    /// [`DATA_SOURCE_VALIDATION_SPAN`] has elapsed.
    Creating,
    /// Created and ready to be ingested from — the modelled `DataSource$status`
    /// documents this as the status of a data source that "has been created and
    /// is ready for ingestion into the knowledge base", which a data source the
    /// service has no source to reach for is as soon as it is stored.
    Available,
    /// `UpdateDataSource` accepted, the change not applied yet — a modelled
    /// `DataSourceStatus` value the data source carries while the service
    /// applies it. The clock owes it the outcome of the updated configuration
    /// once [`DATA_SOURCE_VALIDATION_SPAN`] has elapsed, the same outcome the
    /// same configuration takes at create time.
    Updating,
    /// The connection to the source the data source's connector names did not
    /// come up, so the data source serves no ingestion.
    Failed,
    /// `DeleteDataSource` accepted, or the deletion of the knowledge base that
    /// contains it — the modelled `DataSource$status` documents this as the
    /// status of a data source that "is being deleted". The record is removed
    /// from the store once [`DELETION_SPAN`] has elapsed.
    Deleting,
}

impl DataSourceStatus {
    /// The member value for the modelled `DataSourceStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Updating => "UPDATING",
            Self::Failed => "FAILED",
            Self::Deleting => "DELETING",
        }
    }

    /// Whether a data source in this status serves ingestion.
    ///
    /// The modelled `DataSource$status` documents exactly one value as the
    /// status of a data source that "has been created and is ready for ingestion
    /// into the knowledge base" — `AVAILABLE` — so in every other status the
    /// data source is either not ready yet, being changed, failed or on its way
    /// out, and nothing can be ingested from it.
    pub fn serves_ingestion(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// The `ConflictException` message a call that starts an ingestion job answers
/// when the data source's own status does not serve ingestion: the data source's
/// current `DataSourceStatus`, read from the stored record, fills the one
/// variable part.
pub fn ingestion_blocked_by_status_message(status: DataSourceStatus) -> String {
    format!(
        "You cannot start an ingestion job on a data source with status {}.",
        status.as_wire()
    )
}

/// The status a data source whose EFFECTIVE configuration is `configuration`
/// settles at once the clock has made the outcome of that configuration due.
///
/// A data source the service has nothing to connect out for is ready to be
/// ingested from, which is what the modelled `DataSource$status` documents
/// `AVAILABLE` as. One whose connector has a source outside the service to
/// reach settles at `FAILED`: this emulator runs no connector, so that
/// connection never comes up, and reporting the data source ready to ingest
/// from a source nothing ever read would be a readiness it does not have.
///
/// The outcome is the configuration's, not the call's, so a create and an
/// update that store the same configuration settle at the same status.
pub fn settled_data_source_status(configuration: &DataSourceConfiguration) -> DataSourceStatus {
    if connects_to_an_external_source(configuration) {
        DataSourceStatus::Failed
    } else {
        DataSourceStatus::Available
    }
}

/// Whether a knowledge base whose `knowledgeBaseConfiguration.type` is
/// `knowledge_base_type` supports a data source whose
/// `dataSourceConfiguration.type` is `data_source_type`.
///
/// The model splits `DataSourceConfiguration`'s connection members in two by the
/// kind of knowledge base that runs them: the managed-connector configuration
/// "connects a MANAGED knowledge base to a supported data source connector" and
/// is the one to specify "when the data source type is
/// MANAGED_KNOWLEDGE_BASE_CONNECTOR", while every sibling that carries its own
/// connection details is documented as being for SELF-MANAGED knowledge bases,
/// with managed knowledge bases directed to the managed-connector configuration
/// instead
/// (`smithy_query(query="shape",shape="DataSourceConfiguration")`). So a
/// service-managed knowledge base supports the managed-connector kind and no
/// other, and a self-managed one supports every other kind and not that one.
pub fn knowledge_base_supports_data_source_type(
    knowledge_base_type: &str,
    data_source_type: &str,
) -> bool {
    (knowledge_base_type == MANAGED_KNOWLEDGE_BASE_TYPE)
        == (data_source_type == MANAGED_CONNECTOR_DATA_SOURCE_TYPE)
}

/// The `ValidationException` message a call that stores a data source answers
/// when the kind of data source it carries is not one the target knowledge
/// base's kind supports: the knowledge base's own `KnowledgeBaseType`, read from
/// the stored knowledge base, fills the one variable part.
pub fn unsupported_data_source_type_message(knowledge_base_type: &str) -> String {
    format!("Unsupported data source type for {knowledge_base_type} knowledge base type.")
}

/// One stored data source: the single source of truth for every data-source
/// operation.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    pub knowledge_base_id: String,
    pub data_source_id: String,
    pub name: String,
    pub description: Option<String>,
    pub data_source_configuration: DataSourceConfiguration,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    pub vector_ingestion_configuration: Option<VectorIngestionConfiguration>,
    pub data_deletion_policy: Option<String>,
    pub status: DataSourceStatus,
    /// A status the CLOCK owes this data source, and the instant it comes due.
    /// A reporting operation may materialise it once that instant has passed;
    /// it never invents a transition of its own.
    pub pending_status: Option<(SystemTime, DataSourceStatus)>,
    /// The instant the CLOCK owes this data source's removal, set by the delete
    /// that was accepted for it — or for the knowledge base that contains it —
    /// and `None` while none was.
    pub removal_due_at: Option<SystemTime>,
    /// Reasons the last API operation on the data source failed; empty while
    /// nothing has failed.
    pub failure_reasons: Vec<String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl DataSourceRecord {
    /// Whether a delete has already been accepted for this data source, or for
    /// the knowledge base that contains it, so it is on its way out of the
    /// store.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// This record as the modelled `DataSource` shape: every member is
    /// populated from the stored record, never from the request being served.
    pub fn to_data_source(&self) -> DataSource {
        DataSource {
            knowledge_base_id: self.knowledge_base_id.clone(),
            data_source_id: self.data_source_id.clone(),
            name: self.name.clone(),
            status: self.status.as_wire().to_string(),
            description: self.description.clone(),
            data_source_configuration: self.data_source_configuration.clone(),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            vector_ingestion_configuration: self.vector_ingestion_configuration.clone(),
            data_deletion_policy: self.data_deletion_policy.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            failure_reasons: (!self.failure_reasons.is_empty())
                .then(|| self.failure_reasons.clone()),
        }
    }
}

/// The `ResourceNotFoundException` message a data-source operation answers when
/// the data source the request names is not stored: the id the request carried
/// fills the one variable part, and the enclosing knowledge base is not part of
/// the sentence.
pub fn data_source_not_found_message(data_source_id: &str) -> String {
    identified_resource_not_found_message("DataSource", data_source_id)
}

/// The `ResourceNotFoundException` message `DeleteDataSource` answers when the
/// data source the request addresses cannot be resolved — whether it is the data
/// source or the knowledge base enclosing it that the store does not hold. The
/// missing resource is the DATA SOURCE the request names, and this refusal words
/// it with both identifiers of the pair that addresses it; each is a value taken
/// from the request path, and the rest is fixed text.
pub fn data_source_of_knowledge_base_not_found_message(
    data_source_id: &str,
    knowledge_base_id: &str,
) -> String {
    format!(
        "DataSource with id {data_source_id} in knowledgeBase with id {knowledge_base_id} \
         is not found."
    )
}

/// The most ingestion jobs one data source holds; past it `StartIngestionJob`
/// answers the modelled `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on ingestion jobs, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store, set far above any workflow an
/// emulator serves, and it is what stops the ingestion-job state growing without
/// end as one data source is ingested again and again.
pub const INGESTION_JOB_QUOTA: usize = 1_000;

/// How long after `StartIngestionJob` a job leaves `STARTING` for
/// `IN_PROGRESS`.
///
/// CHOSEN, not documented: AWS publishes no duration for an ingestion job. It is
/// deliberately non-zero so a caller that reads the job back immediately still
/// observes the `STARTING` status the modelled `IngestionJobStatus` declares, and
/// short enough that a client script polling the job converges promptly. It is
/// the longer of the two spans, so that first read has the widest window.
pub const INGESTION_JOB_START_SPAN: Duration = Duration::from_millis(450);

/// How long after a job reaches `IN_PROGRESS` it reaches `COMPLETE`.
///
/// CHOSEN, not documented, and non-zero for the same reason as
/// [`INGESTION_JOB_START_SPAN`]: `IN_PROGRESS` is a status the model declares, so
/// a poller has to be able to observe it. The two spans together keep the whole
/// progression inside three quarters of a second.
pub const INGESTION_JOB_RUN_SPAN: Duration = Duration::from_millis(300);

/// How long after `StopIngestionJob` a job leaves `STOPPING` for `STOPPED`.
///
/// CHOSEN, not documented, and non-zero for the same reason as
/// [`INGESTION_JOB_START_SPAN`]: `STOPPING` is a status the model declares, so a
/// caller that reads the job back immediately still observes it. It is the span a
/// run of the job takes ([`INGESTION_JOB_RUN_SPAN`]), so halting a job converges
/// on the same scale as finishing one.
pub const INGESTION_JOB_STOP_SPAN: Duration = INGESTION_JOB_RUN_SPAN;

/// The `IngestionJobStatus` values this slice of the service can hold, named as
/// the model's `IngestionJobStatus` enum names them on the wire.
///
/// `StartIngestionJob` mints only [`IngestionJobStatus::Starting`] and
/// `StopIngestionJob` moves a job that has not finished to
/// [`IngestionJobStatus::Stopping`]; the rest of each chain is owed by the clock.
/// The modelled enum also declares `FAILED`, which nothing in this slice reaches:
/// no job here fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestionJobStatus {
    /// `StartIngestionJob` accepted; the job reaches
    /// [`IngestionJobStatus::InProgress`] once [`INGESTION_JOB_START_SPAN`] has
    /// elapsed.
    Starting,
    /// The job is running; it reaches [`IngestionJobStatus::Complete`] once
    /// [`INGESTION_JOB_RUN_SPAN`] more has elapsed.
    InProgress,
    /// The job finished — a terminal status, so the clock owes it nothing more.
    Complete,
    /// `StopIngestionJob` accepted; the job reaches
    /// [`IngestionJobStatus::Stopped`] once [`INGESTION_JOB_STOP_SPAN`] has
    /// elapsed, and the run it was in the middle of reaches nothing else.
    Stopping,
    /// The job was halted — a terminal status, so the clock owes it nothing more
    /// and the data source it ran for serves a new job again.
    Stopped,
}

impl IngestionJobStatus {
    /// The member value for the modelled `IngestionJobStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::InProgress => "IN_PROGRESS",
            Self::Complete => "COMPLETE",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
        }
    }

    /// Whether this status is one the job never leaves: the run is over, whether
    /// it finished or was halted, so the clock owes the job nothing more.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Stopped)
    }

    /// The status the clock owes a job in this one next, and how long after it
    /// reached this status that comes due. `None` for a terminal status.
    fn owed_next(self) -> Option<(Duration, Self)> {
        match self {
            Self::Starting => Some((INGESTION_JOB_START_SPAN, Self::InProgress)),
            Self::InProgress => Some((INGESTION_JOB_RUN_SPAN, Self::Complete)),
            Self::Stopping => Some((INGESTION_JOB_STOP_SPAN, Self::Stopped)),
            Self::Complete | Self::Stopped => None,
        }
    }
}

/// The `ResourceNotFoundException` message an ingestion-job operation answers when
/// the triple the request addresses resolves to no stored job.
///
/// A job is addressed by the knowledge base and data source it ran for as well as by
/// its own id, but the resource that could not be resolved is the JOB — so the
/// refusal names the job's own id and nothing else, through the one helper that
/// words this error class ([`identified_resource_not_found_message`]).
pub fn ingestion_job_not_found_message(ingestion_job_id: &str) -> String {
    identified_resource_not_found_message("IngestionJob", ingestion_job_id)
}

/// The `ValidationException` message `StopIngestionJob` answers for a job whose
/// run is already over: the job's current `IngestionJobStatus`, read from the
/// stored record, fills the one variable part of AWS's own sentence for this error
/// class, whose text is otherwise fixed.
///
/// The operation "stops a currently running data ingestion job", so a job that has
/// reached a terminal status is not one there is a run to stop.
pub fn stop_blocked_by_status_message(status: IngestionJobStatus) -> String {
    format!(
        "Ingestion job is already in a terminal state: {}.",
        status.as_wire()
    )
}

/// One stored ingestion job: the single source of truth for every
/// ingestion-job operation.
#[derive(Debug, Clone)]
pub struct IngestionJobRecord {
    pub knowledge_base_id: String,
    pub data_source_id: String,
    pub ingestion_job_id: String,
    pub description: Option<String>,
    pub status: IngestionJobStatus,
    /// A status the CLOCK owes this job, and the instant it comes due. A
    /// reporting operation may materialise it once that instant has passed; it
    /// never invents a transition of its own.
    pub pending_status: Option<(SystemTime, IngestionJobStatus)>,
    /// What the job ingested: each count is of documents this store holds for
    /// the data source, so it stays at the modelled default of 0 for as long as
    /// the store holds none. Nothing here is a number about documents the
    /// emulator never took in.
    pub statistics: IngestionJobStatistics,
    /// The accounting the job's run measured, which it reports once it has
    /// finished.
    ///
    /// The modelled members are the service's accounting OF A RUN, and the run
    /// is over only when the job reaches [`IngestionJobStatus::Complete`] — so
    /// what a job still in flight reports is the modelled defaults, and the
    /// measured counts take their place at the instant the clock completes it.
    pub completed_statistics: IngestionJobStatistics,
    /// The reasons the job's run refused the source documents it did not take
    /// in, which it reports once it has finished.
    ///
    /// Published at the same instant [`Self::completed_statistics`] is, and for
    /// the same reason: the modelled `failureReasons` is this run's account of
    /// what it could not ingest, so a job still in flight reports none and the
    /// measured reasons take their place when the clock completes it. Every
    /// entry names a document the measured statistics count as failed.
    pub completed_failure_reasons: Vec<String>,
    /// Reasons the job failed; empty while nothing has failed.
    pub failure_reasons: Vec<String>,
    pub started_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the job's text became queryable, which the modelled `textReadyAt`
    /// documents is "not returned until text extraction is complete for all the
    /// documents" — so it stays absent until the job reaches
    /// [`IngestionJobStatus::Complete`].
    pub text_ready_at: Option<SystemTime>,
    pub client_token: Option<String>,
}

impl IngestionJobRecord {
    /// A job `StartIngestionJob` has just accepted: `STARTING`, with the first
    /// transition the clock owes it already due.
    pub fn starting(
        knowledge_base_id: String,
        data_source_id: String,
        ingestion_job_id: String,
        description: Option<String>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            knowledge_base_id,
            data_source_id,
            ingestion_job_id,
            description,
            status: IngestionJobStatus::Starting,
            pending_status: Some((now + INGESTION_JOB_START_SPAN, IngestionJobStatus::InProgress)),
            // A run that has not finished has accounted for nothing yet, so
            // every count is the default the model gives it until the clock
            // completes the job and the measured accounting takes its place.
            statistics: IngestionJobStatistics::default(),
            // What the run measures is written here once the work is done; a
            // job that measured nothing reports the defaults.
            completed_statistics: IngestionJobStatistics::default(),
            // What the run refused is written here with what it measured.
            completed_failure_reasons: Vec::new(),
            // Nothing has failed on a job this call just started.
            failure_reasons: Vec::new(),
            started_at: now,
            updated_at: now,
            text_ready_at: None,
            client_token,
        }
    }

    /// Whether the clock still owes this job a status, i.e. it has not reached a
    /// terminal one. A data source ingests one job at a time, so this is what a
    /// second `StartIngestionJob` conflicts with — and a job that was stopped is
    /// not one of them, which is what lets the caller "send a StartIngestionJob
    /// request again to ingest the rest of your data".
    pub fn is_running(&self) -> bool {
        !self.status.is_terminal()
    }

    /// Halt this job: it reports the modelled `STOPPING` status from now on and
    /// the clock owes it [`IngestionJobStatus::Stopped`], at which point the run
    /// is over.
    ///
    /// The accounting is left exactly as it stands. The modelled statistics are
    /// the service's accounting OF A RUN, published when the run finishes — and a
    /// run the caller halted never finishes, so what it had accounted for when the
    /// stop was accepted is what it keeps reporting and the completion accounting
    /// ([`Self::completed_statistics`]) is never published for it. `textReadyAt`
    /// stays absent for the same reason: text extraction over the documents never
    /// completed.
    pub fn begin_stop(&mut self, now: SystemTime) {
        self.status = IngestionJobStatus::Stopping;
        self.pending_status = Some((now + INGESTION_JOB_STOP_SPAN, IngestionJobStatus::Stopped));
        self.updated_at = now;
    }

    /// This record as the modelled `IngestionJob` shape: every member is
    /// populated from the stored record, never from the request being served.
    pub fn to_ingestion_job(&self) -> IngestionJob {
        IngestionJob {
            knowledge_base_id: self.knowledge_base_id.clone(),
            data_source_id: self.data_source_id.clone(),
            ingestion_job_id: self.ingestion_job_id.clone(),
            description: self.description.clone(),
            status: self.status.as_wire().to_string(),
            statistics: Some(self.statistics.clone()),
            failure_reasons: (!self.failure_reasons.is_empty())
                .then(|| self.failure_reasons.clone()),
            started_at: to_date_time(self.started_at),
            updated_at: to_date_time(self.updated_at),
            text_ready_at: self.text_ready_at.map(to_date_time),
        }
    }
}

/// The most documents one data source holds, and the most ingestion requests it
/// remembers by their `@idempotencyToken`; past either
/// `IngestKnowledgeBaseDocuments` answers the modelled
/// `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on documents per data source,
/// and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store, set far above
/// any workflow an emulator serves, and it is what stops the document state
/// growing without end as documents are pushed in again and again. Each record
/// is itself bounded by the modelled `@length` constraints on a document's
/// content, which the boundary validator enforces before a handler runs.
pub const DOCUMENT_QUOTA: usize = 1_000;

/// How long after `IngestKnowledgeBaseDocuments` a document leaves `STARTING`
/// for `IN_PROGRESS`.
///
/// CHOSEN, not documented: AWS publishes no duration for the ingestion of a
/// document. It is deliberately non-zero so a caller that reads the document
/// back immediately still observes the transient `STARTING` status the modelled
/// `DocumentStatus` declares, and it matches [`INGESTION_JOB_START_SPAN`] so
/// that documents and the ingestion jobs of the same data source converge on
/// the same scale.
pub const DOCUMENT_INGESTION_SPAN: Duration = INGESTION_JOB_START_SPAN;

/// How long after a document reaches `IN_PROGRESS` it reaches `INDEXED`.
///
/// CHOSEN, not documented, and non-zero for the same reason as
/// [`DOCUMENT_INGESTION_SPAN`]: `IN_PROGRESS` is a status the model declares, so
/// a poller has to be able to observe it. It matches
/// [`INGESTION_JOB_RUN_SPAN`] for the same reason.
pub const DOCUMENT_INDEXING_SPAN: Duration = INGESTION_JOB_RUN_SPAN;

/// How long after `DeleteKnowledgeBaseDocuments` a document leaves `DELETING`
/// for `DELETE_IN_PROGRESS`.
///
/// CHOSEN, not documented: AWS publishes no duration for the deletion of a
/// document. It is deliberately non-zero so a caller that reads the document
/// back immediately still observes the transient `DELETING` status the modelled
/// `DocumentStatus` declares, and it is the span the submission of an ingestion
/// takes ([`DOCUMENT_INGESTION_SPAN`]) so that both chains of one document
/// converge on the same scale.
pub const DOCUMENT_DELETION_START_SPAN: Duration = DOCUMENT_INGESTION_SPAN;

/// How long after a document reaches `DELETE_IN_PROGRESS` it reaches
/// `NOT_FOUND`.
///
/// CHOSEN, not documented, and non-zero for the same reason as
/// [`DOCUMENT_DELETION_START_SPAN`]: `DELETE_IN_PROGRESS` is a status the model
/// declares, so a poller has to be able to observe it.
pub const DOCUMENT_DELETION_SPAN: Duration = DOCUMENT_INDEXING_SPAN;

/// The `DocumentStatus` a document operation reports for an identifier this
/// store holds no document for, which the modelled member documents as
/// "NOT_FOUND – The document wasn't found."
pub const DOCUMENT_STATUS_NOT_FOUND: &str = "NOT_FOUND";

/// How many completed deletions one data source keeps a reportable detail for.
///
/// The terminal `NOT_FOUND` status of a deletion is what a client polling
/// `ListKnowledgeBaseDocuments` waits for, and the detail carrying it is what
/// names WHICH document was removed — so the record outlives the document it
/// stood for. Not without end, though: this is the emulator's OWN bound on that
/// history, and it is [`DOCUMENT_QUOTA`] because a data source holds no more
/// documents than that, so no stream of ingestions and deletions can grow the
/// store past twice the documents one data source may hold. Past it the data
/// source keeps the most recently deleted and forgets the oldest, whose
/// identity is answered from then on as one nothing was ever stored under.
pub const DELETED_DOCUMENT_RECORDS_KEPT: usize = DOCUMENT_QUOTA;

/// How long after a deletion has run to its end the data source keeps reporting
/// the document it removed in its COLLECTION, under that deletion's terminal
/// status, before the identity is enumerated no more.
///
/// CHOSEN, not documented: AWS publishes no duration for how long a removed
/// document stays reportable. It is deliberately non-zero and SECONDS wide,
/// because `NOT_FOUND` is the terminal status of a deletion that a client
/// polling the collection waits for — a window narrower than the interval such a
/// client polls at would hide the outcome it is waiting for — and it is bounded
/// because the collection reports what the data source holds: once the window
/// has passed the removal is settled and the identity is enumerated no more, so
/// a client waiting for the identity to LEAVE converges as well. The outcome
/// stays reportable to a targeted read of the identifier either way
/// ([`DELETED_DOCUMENT_RECORDS_KEPT`]).
pub const DOCUMENT_DELETED_LISTING_SPAN: Duration = Duration::from_secs(10);

/// The most documents one `ListKnowledgeBaseDocuments` page reports when the
/// request names no `maxResults`.
///
/// CHOSEN, not documented: AWS publishes no default page size for the listing.
/// It is the upper bound of the modelled `maxResults` range (1..1000,
/// `smithy_query(query="operation",operation="ListKnowledgeBaseDocuments")`), so
/// `maxResults` alone decides how short a page is, and it matches
/// [`DOCUMENT_QUOTA`]: a data source holds no more documents than one default
/// page can carry.
pub const MAX_DOCUMENTS_PER_PAGE: usize = DOCUMENT_QUOTA;

/// The `ContentDataSourceType` of a document pushed into a custom data source.
pub const CUSTOM_CONTENT_TYPE: &str = "CUSTOM";

/// The `ContentDataSourceType` of a document ingested from an S3 data source.
pub const S3_CONTENT_TYPE: &str = "S3";

/// Every modelled `ContentDataSourceType` value, paired with the member of
/// `DocumentContent` and of `DocumentIdentifier` that carries that kind's own
/// details — the member the shapes document as the one "that corresponds to"
/// the `dataSourceType` chosen.
///
/// Source: the `ContentDataSourceType` enum and the `DocumentContent` /
/// `DocumentIdentifier` structures of this service's Smithy model
/// (`smithy_query(query="shape",shape="ContentDataSourceType,DocumentContent,DocumentIdentifier")`).
pub const DOCUMENT_CONTENT_MEMBERS: [(&str, &str); 2] = [
    (CUSTOM_CONTENT_TYPE, "custom"),
    (S3_CONTENT_TYPE, "s3"),
];

/// The member a document of this kind carries its own details in, or `None`
/// when the kind is none the model declares.
pub fn document_content_member(data_source_type: &str) -> Option<&'static str> {
    DOCUMENT_CONTENT_MEMBERS
        .iter()
        .find(|(name, _)| *name == data_source_type)
        .map(|(_, member)| *member)
}

/// The `ValidationException` message a document operation answers when a
/// document names a kind of data source that is not the kind of the data source
/// the request is addressed to, which `IngestKnowledgeBaseDocuments` documents
/// as "The dataSourceType specified in the content for each document must match
/// the type of the data source that you specify in the header".
///
/// The requirement holds for every kind of data source: a document's content
/// names one of the modelled `ContentDataSourceType` values, so a data source
/// whose own type is any other kind — one no content can name — takes no
/// directly ingested document at all.
///
/// The kind the DOCUMENT names is the one variable part of the sentence; the
/// rest is fixed text for this error class.
pub fn document_data_source_type_message(document_type: &str) -> String {
    format!(
        "The dataSourceType that you specified in {document_type} doesn't match the type \
         of the data source specified in the request header. Check that both types match \
         and retry your request."
    )
}

/// The `ValidationException` message a document operation answers when a
/// document names a kind but carries none of that kind's own details — the
/// member `DocumentContent` and `DocumentIdentifier` both document as the one to
/// "include the field that corresponds to it".
pub fn document_member_required_message(path: &str, member: &str, data_source_type: &str) -> String {
    format!("{path}.{member} is required for a {path}.dataSourceType of {data_source_type}")
}

/// The `DocumentStatus` values a directly ingested document passes through,
/// named as the model's `DocumentStatus` enum names them on the wire.
///
/// `IngestKnowledgeBaseDocuments` mints only [`DocumentStatus::Starting`] and
/// `DeleteKnowledgeBaseDocuments` only [`DocumentStatus::Deleting`]; the rest of
/// each chain is owed by the clock. The modelled enum declares further values
/// this slice never reaches — nothing here fails, is ignored or is only
/// partially indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentStatus {
    /// The ingestion carrying the document was submitted; it reaches
    /// [`DocumentStatus::InProgress`] once [`DOCUMENT_INGESTION_SPAN`] has
    /// elapsed.
    Starting,
    /// The document is being ingested; it reaches
    /// [`DocumentStatus::Indexed`] once [`DOCUMENT_INDEXING_SPAN`] more has
    /// elapsed.
    InProgress,
    /// The document was indexed — the terminal status of an ingestion, so the
    /// clock owes it nothing more.
    Indexed,
    /// The deletion carrying the document was submitted; it reaches
    /// [`DocumentStatus::DeleteInProgress`] once
    /// [`DOCUMENT_DELETION_START_SPAN`] has elapsed.
    Deleting,
    /// The document is being deleted; it reaches [`DocumentStatus::NotFound`]
    /// once [`DOCUMENT_DELETION_SPAN`] more has elapsed.
    DeleteInProgress,
    /// The document is gone from the data source — the terminal status of a
    /// deletion, which the modelled member documents as the status of a document
    /// that "wasn't found", so the clock owes it nothing more. The data source
    /// HOLDS it no longer: it counts against no bound, carries no content and is
    /// no source document, and the identity is free again for a later ingestion
    /// to take. Its record stays behind so the deletion's OUTCOME is reportable
    /// — to a targeted read and to the collection the data source enumerates —
    /// for [`DOCUMENT_DELETED_LISTING_SPAN`] after the deletion completed, and
    /// is then forgotten (and bounded by [`DELETED_DOCUMENT_RECORDS_KEPT`]
    /// besides).
    NotFound,
}

impl DocumentStatus {
    /// The member value for the modelled `DocumentStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::InProgress => "IN_PROGRESS",
            Self::Indexed => "INDEXED",
            Self::Deleting => "DELETING",
            Self::DeleteInProgress => "DELETE_IN_PROGRESS",
            Self::NotFound => DOCUMENT_STATUS_NOT_FOUND,
        }
    }

    /// The status the clock owes a document in this one next, and how long after
    /// it reached this status that comes due. `None` for a terminal status.
    fn owed_next(self) -> Option<(Duration, Self)> {
        match self {
            Self::Starting => Some((DOCUMENT_INGESTION_SPAN, Self::InProgress)),
            Self::InProgress => Some((DOCUMENT_INDEXING_SPAN, Self::Indexed)),
            Self::Deleting => Some((DOCUMENT_DELETION_START_SPAN, Self::DeleteInProgress)),
            Self::DeleteInProgress => Some((DOCUMENT_DELETION_SPAN, Self::NotFound)),
            Self::Indexed | Self::NotFound => None,
        }
    }

    /// Whether a deletion has already been accepted for a document in this
    /// status: it is on its way out of the data source, or already out of it.
    fn is_being_deleted(self) -> bool {
        matches!(self, Self::Deleting | Self::DeleteInProgress | Self::NotFound)
    }

    /// Whether the data source still HOLDS a document reported in this status.
    ///
    /// Every status but the terminal one of a deletion is a document the data
    /// source holds: it counts against the data source's bound, its content can
    /// be retrieved and an ingestion job over a pushed data source scans it. A
    /// document whose deletion has run to its end is held no longer — its record
    /// stays only to report that outcome, which is a different question from
    /// what the data source holds ([`DocumentRecord::reports_deletion_until`]).
    fn is_held(self) -> bool {
        !matches!(self, Self::NotFound)
    }
}

/// Which document of one data source a request names.
///
/// The modelled `DocumentIdentifier` and `DocumentContent` are both
/// discriminated by `dataSourceType`: it names which sibling member carries the
/// document's own id, so the kind together with that id is a document's
/// identity within its data source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocumentKey {
    /// A document of a custom data source, by the id the request gave it.
    Custom(String),
    /// A document of an S3 data source, by the URI it lives at.
    S3(String),
}

impl DocumentKey {
    /// The document `identifier` names, or `None` when it carries none of the
    /// details the kind it names needs.
    pub fn from_identifier(identifier: &DocumentIdentifier) -> Option<Self> {
        match identifier.data_source_type.as_str() {
            CUSTOM_CONTENT_TYPE => identifier
                .custom
                .as_ref()
                .map(|custom| Self::Custom(custom.id.clone())),
            S3_CONTENT_TYPE => identifier.s3.as_ref().map(|s3| Self::S3(s3.uri.clone())),
            _ => None,
        }
    }

    /// The document `content` is the content of, or `None` when it carries none
    /// of the details the kind it names needs.
    pub fn from_content(content: &DocumentContent) -> Option<Self> {
        match content.data_source_type.as_str() {
            CUSTOM_CONTENT_TYPE => content
                .custom
                .as_ref()
                .map(|custom| Self::Custom(custom.custom_document_identifier.id.clone())),
            S3_CONTENT_TYPE => content
                .s3
                .as_ref()
                .map(|s3| Self::S3(s3.s3_location.uri.clone())),
            _ => None,
        }
    }

    /// The sort key one data source's documents are listed in: the kind of data
    /// source the document belongs to, ties broken by the document's own id
    /// within that kind.
    ///
    /// A document's identity never changes, so this is a total order over the
    /// documents of one data source that no status transition disturbs — which
    /// is what lets a page resume exactly after the one before it.
    fn sort_key(&self) -> (&'static str, &str) {
        match self {
            Self::Custom(id) => (CUSTOM_CONTENT_TYPE, id.as_str()),
            Self::S3(uri) => (S3_CONTENT_TYPE, uri.as_str()),
        }
    }

    /// The document's own id within its data source: the id a custom document
    /// was given, or the URI an S3 document lives at.
    pub fn identity(&self) -> &str {
        let (_, identity) = self.sort_key();
        identity
    }

    /// This identity as the modelled `DocumentIdentifier` shape, which is what
    /// every document detail reports it as.
    pub fn to_identifier(&self) -> DocumentIdentifier {
        match self {
            Self::Custom(id) => DocumentIdentifier {
                data_source_type: CUSTOM_CONTENT_TYPE.to_string(),
                s3: None,
                custom: Some(CustomDocumentIdentifier { id: id.clone() }),
            },
            Self::S3(uri) => DocumentIdentifier {
                data_source_type: S3_CONTENT_TYPE.to_string(),
                s3: Some(S3Location { uri: uri.clone() }),
                custom: None,
            },
        }
    }
}

/// One document ingested directly into a data source: the single source of
/// truth for every document operation.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    pub knowledge_base_id: String,
    pub data_source_id: String,
    /// Which document of the data source this is.
    pub key: DocumentKey,
    pub status: DocumentStatus,
    /// A status the CLOCK owes this document, and the instant it comes due. A
    /// reporting operation may materialise it once that instant has passed; it
    /// never invents a transition of its own.
    pub pending_status: Option<(SystemTime, DocumentStatus)>,
    /// The content the ingestion carried, kept so a later read reports the
    /// document the store holds rather than the request being served.
    pub content: DocumentContent,
    /// The metadata the ingestion associated with the content, if any.
    pub metadata: Option<DocumentMetadata>,
    /// The digest of the source object this document was taken FROM, for a
    /// document an ingestion job enumerated in its data source's own source;
    /// `None` for one pushed in through the service's document API, which
    /// carries its content in the request instead.
    ///
    /// It is what tells a re-crawl that a source document has not changed from
    /// one that was modified since it was last ingested.
    pub source_digest: Option<u64>,
    /// The CHUNK the ingestion recorded for this document: the text it read and
    /// the vector of that text ([`IngestedChunk`]). Recorded when the ingestion
    /// took the document in and never re-derived, so what a retrieval of this
    /// document answers does not change when its source does; `None` for a
    /// document whose bytes live nowhere this emulator holds, for which nothing
    /// invents one.
    pub chunk: Option<IngestedChunk>,
    pub updated_at: SystemTime,
    pub client_token: Option<String>,
}

impl DocumentRecord {
    /// A document an ingestion has just accepted: `STARTING`, with the first
    /// transition the clock owes it already due.
    pub fn starting(
        knowledge_base_id: String,
        data_source_id: String,
        key: DocumentKey,
        content: DocumentContent,
        metadata: Option<DocumentMetadata>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            knowledge_base_id,
            data_source_id,
            key,
            status: DocumentStatus::Starting,
            pending_status: Some((now + DOCUMENT_INGESTION_SPAN, DocumentStatus::InProgress)),
            content,
            metadata,
            source_digest: None,
            // The chunk is recorded once the ingestion has read the document's
            // bytes, which is work no store guard is held for.
            chunk: None,
            updated_at: now,
            client_token,
        }
    }

    /// A document an ingestion job took from its data source's own source:
    /// `STARTING`, with the same progression the clock owes a pushed one, and
    /// the digest of the source object it was read from.
    ///
    /// Its content is where it lives: an S3 source document IS the object at
    /// that URI, so the record points at it rather than copying its bytes, and
    /// `ListKnowledgeBaseDocuments` reports exactly the identity the job
    /// ingested.
    pub fn crawled(
        knowledge_base_id: String,
        data_source_id: String,
        key: DocumentKey,
        source_digest: u64,
        chunk: Option<IngestedChunk>,
        now: SystemTime,
    ) -> Self {
        let content = DocumentContent {
            data_source_type: S3_CONTENT_TYPE.to_string(),
            custom: None,
            s3: Some(S3Content {
                s3_location: S3Location {
                    uri: key.identity().to_string(),
                },
            }),
        };
        Self {
            knowledge_base_id,
            data_source_id,
            key,
            status: DocumentStatus::Starting,
            pending_status: Some((now + DOCUMENT_INGESTION_SPAN, DocumentStatus::InProgress)),
            content,
            metadata: None,
            source_digest: Some(source_digest),
            chunk,
            updated_at: now,
            // No request named this document: the job found it in the data
            // source's source.
            client_token: None,
        }
    }

    /// Accept the deletion of this document: it reports `DELETING` from now on,
    /// and the clock owes it the rest of the chain down to `NOT_FOUND`.
    ///
    /// The record stays in the store while the deletion is in flight, so a
    /// poller observes the statuses the model declares for it; once the chain
    /// reaches its terminal status the data source holds nothing for this
    /// document any more, while the record stays behind reporting that terminal
    /// status so the deletion's outcome is observable. The digest of the source
    /// object it was taken from goes,
    /// because the data source no longer holds this document's content — so a
    /// later crawl that finds that object again ingests it anew rather than
    /// taking it for one already held.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = DocumentStatus::Deleting;
        self.pending_status = Some((
            now + DOCUMENT_DELETION_START_SPAN,
            DocumentStatus::DeleteInProgress,
        ));
        self.source_digest = None;
        // The chunk goes with it: the knowledge base no longer holds this
        // document's content, so no retrieval reports it from here on.
        self.chunk = None;
        self.updated_at = now;
    }

    /// Whether the data source still HOLDS this document, as against holding
    /// only the record of its completed deletion
    /// ([`DocumentStatus::is_held`]).
    pub fn is_held(&self) -> bool {
        self.status.is_held()
    }

    /// The instant up to which this record reports a COMPLETED deletion, after
    /// which the identity is enumerated no more; `None` for a document the data
    /// source still holds, whose record is not a deletion's outcome at all.
    ///
    /// A document's status chain is applied at the instant each transition came
    /// due and `updated_at` carries that instant, so the window runs from when
    /// the deletion COMPLETED rather than from when a reader noticed it
    /// ([`DOCUMENT_DELETED_LISTING_SPAN`]).
    pub fn reports_deletion_until(&self) -> Option<SystemTime> {
        (!self.is_held()).then(|| self.updated_at + DOCUMENT_DELETED_LISTING_SPAN)
    }

    /// Whether the collection of this data source's documents reports this
    /// document at `now`: every document the data source holds, and one whose
    /// deletion has completed for as long as that deletion is still settling
    /// ([`Self::reports_deletion_until`]).
    pub fn is_reported_at(&self, now: SystemTime) -> bool {
        self.reports_deletion_until()
            .is_none_or(|until| now < until)
    }

    /// This record as the modelled `KnowledgeBaseDocumentDetail` shape: every
    /// member is populated from the stored record, never from the request being
    /// served.
    pub fn to_detail(&self) -> KnowledgeBaseDocumentDetail {
        KnowledgeBaseDocumentDetail {
            knowledge_base_id: self.knowledge_base_id.clone(),
            data_source_id: self.data_source_id.clone(),
            status: self.status.as_wire().to_string(),
            identifier: self.key.to_identifier(),
            // The modelled member documents a reason as appearing alongside the
            // status `IGNORED`, which nothing here reaches.
            status_reason: None,
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

/// The detail a document operation reports for an identifier this store holds
/// no document for: the identity it was asked about, and the status the model
/// gives exactly that case.
///
/// Nothing is stored for it, so it has no instant at which it last changed and
/// the `updatedAt` member stays absent.
pub fn not_found_document_detail(
    knowledge_base_id: &str,
    data_source_id: &str,
    key: &DocumentKey,
) -> KnowledgeBaseDocumentDetail {
    KnowledgeBaseDocumentDetail {
        knowledge_base_id: knowledge_base_id.to_string(),
        data_source_id: data_source_id.to_string(),
        status: DOCUMENT_STATUS_NOT_FOUND.to_string(),
        identifier: key.to_identifier(),
        status_reason: None,
        updated_at: None,
    }
}

/// A `ListKnowledgeBaseDocuments` resume cursor: how many of one data source's
/// documents the listing has already reported, in the order
/// [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::page_of_documents`]
/// lists them.
///
/// The token names a POSITION in that order rather than a document: a custom
/// document's id and an S3 document's URI are each modelled up to 2048
/// characters, the whole of the modelled `NextToken` length bound, so a token
/// carrying one could be too long to send back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentPageCursor {
    reported: usize,
}

impl DocumentPageCursor {
    /// Decode a `nextToken` a previous page handed out. `None` when the token is
    /// not one [`Self::token`] renders: a position is a plain decimal count of
    /// the documents already reported, so nothing else names a position in this
    /// listing.
    pub fn parse(token: &str) -> Option<Self> {
        if !is_counted_token_position(token) {
            return None;
        }
        token.parse().ok().map(|reported| Self { reported })
    }

    /// The cursor that resumes after `reported` documents, or `None` when no
    /// document has been reported yet — the position a listing starts at, which
    /// is what an absent `nextToken` already names.
    fn after(reported: usize) -> Option<Self> {
        (reported > 0).then_some(Self { reported })
    }

    /// This position as the opaque `nextToken` a page hands out.
    pub fn token(self) -> String {
        self.reported.to_string()
    }

    /// How many documents the listing had already reported when it handed this
    /// cursor out.
    fn reported(self) -> usize {
        self.reported
    }
}

/// A `ListAgents` resume cursor, decoded back into the ordering it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    created_at_nanos: u128,
    agent_id: String,
}

impl PageCursor {
    /// Decode a `nextToken` a previous page handed out. `None` when the token was
    /// not produced by [`AgentRecord::page_cursor`] — only the one spelling that
    /// function writes names a position.
    pub fn parse(token: &str) -> Option<Self> {
        let (created_at_nanos, agent_id) = parse_composite_cursor(token)?;
        Some(Self {
            created_at_nanos,
            agent_id: agent_id.to_string(),
        })
    }

    /// Whether `record` sorts after this cursor, i.e. belongs on the next page.
    fn precedes(&self, record: &AgentRecord) -> bool {
        (self.created_at_nanos, self.agent_id.as_str()) < record.sort_key()
    }
}

/// Per-service state for com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// Every agent that exists, keyed by its 10-character `agentId`.
    pub agents: HashMap<String, AgentRecord>,
    /// `agentName` -> `agentId`, so `CreateAgent` can refuse a duplicate name.
    pub agent_names: HashMap<String, String>,
    /// `clientToken` -> `agentId`, so a retried `CreateAgent` carrying the same
    /// `@idempotencyToken` returns the agent it already created.
    pub client_tokens: HashMap<String, String>,
    /// The accounts that used this plane's agent surface before it closed to new
    /// customers, which is what decides whether a `CreateAgent` from an account
    /// is served at all. Empty by default: see [`agents_classic`].
    pub agents_classic_usage: agents_classic::AgentsClassicUsage,
    /// Every flow that exists, keyed by its 10-character `flowId`.
    pub flows: HashMap<String, FlowRecord>,
    /// `clientToken` -> `flowId`, so a retried `CreateFlow` carrying the same
    /// `@idempotencyToken` returns the flow it already created.
    pub flow_client_tokens: HashMap<String, String>,
    /// Every numbered flow version that exists, keyed by the `flowId` it was cut
    /// from and its version number. The pair is the version's identity, so one
    /// flow never reports another flow's versions. The flow's DRAFT version is
    /// NOT here: it is the flow record itself.
    pub flow_versions: HashMap<(String, u32), FlowVersionRecord>,
    /// `(flowId, clientToken)` -> the version number that request cut, so a
    /// retried `CreateFlowVersion` carrying the same `@idempotencyToken`
    /// returns the version it already cut rather than cutting a second one. The
    /// key carries the flow the replay is read at, so one token reused for
    /// another flow cannot displace the version it already names.
    pub flow_version_client_tokens: HashMap<(String, String), u32>,
    /// Every alias of every flow, keyed by the `flowId` it was created on and
    /// its own `flowAliasId`. The pair is the alias's identity — every modelled
    /// alias operation addresses it as that pair — so one flow never reports
    /// another flow's alias.
    pub flow_aliases: HashMap<(String, String), FlowAliasRecord>,
    /// `(flowId, clientToken)` -> the `flowAliasId` that request created, so a
    /// retried `CreateFlowAlias` carrying the same `@idempotencyToken` returns
    /// the alias it already created rather than creating a second one. The key
    /// carries the flow the replay is read at, so one token reused for another
    /// flow cannot displace the alias it already names.
    pub flow_alias_client_tokens: HashMap<(String, String), String>,
    /// Every knowledge base that exists, keyed by its 10-character
    /// `knowledgeBaseId`.
    pub knowledge_bases: HashMap<String, KnowledgeBaseRecord>,
    /// `clientToken` -> `knowledgeBaseId`, so a retried `CreateKnowledgeBase`
    /// carrying the same `@idempotencyToken` returns the knowledge base it
    /// already created.
    pub knowledge_base_client_tokens: HashMap<String, String>,
    /// Every data source that exists, keyed by its 10-character `dataSourceId`.
    /// Each record names the knowledge base it belongs to, so a read of one
    /// knowledge base never reports another's data source.
    pub data_sources: HashMap<String, DataSourceRecord>,
    /// `clientToken` -> `dataSourceId`, so a retried `CreateDataSource`
    /// carrying the same `@idempotencyToken` returns the data source it already
    /// created.
    pub data_source_client_tokens: HashMap<String, String>,
    /// Every ingestion job that exists, keyed by its 10-character
    /// `ingestionJobId`. Each record names the knowledge base and data source
    /// it ran for, so a read of one data source never reports another's job.
    pub ingestion_jobs: HashMap<String, IngestionJobRecord>,
    /// `(knowledgeBaseId, dataSourceId, clientToken)` -> `ingestionJobId`, so a
    /// retried `StartIngestionJob` carrying the same `@idempotencyToken`
    /// returns the job it already started rather than starting a second one.
    /// The key carries the whole scope the replay is read at, so one token
    /// reused for another data source cannot displace the job it already names.
    pub ingestion_job_client_tokens: HashMap<(String, String, String), String>,
    /// Every document ingested directly into a data source, keyed by the
    /// knowledge base, the data source and the document's own identity: a
    /// document is identified within its data source, so the same id pushed
    /// into two data sources is two documents.
    pub documents: HashMap<(String, String, DocumentKey), DocumentRecord>,
    /// `(knowledgeBaseId, dataSourceId, clientToken)` -> the documents that
    /// request ingested, so a retried `IngestKnowledgeBaseDocuments` carrying
    /// the same `@idempotencyToken` reports those documents again rather than
    /// starting their ingestion a second time. The key carries the whole scope
    /// the replay is read at, so one token reused for another data source
    /// cannot displace the documents it already names.
    ///
    /// Bounded by [`DOCUMENT_QUOTA`] per knowledge base and data source, the
    /// same bound the documents themselves take: this index grows once per
    /// accepted request even when the request re-ingests documents the data
    /// source already holds, so the ingestion checks
    /// [`Self::document_token_count`] against that bound before it writes.
    pub document_client_tokens: HashMap<(String, String, String), Vec<DocumentKey>>,
    /// Every prompt that exists, keyed by its 10-character `promptId`. The prompt's
    /// working draft IS this record, so no copy of the DRAFT version sits beside it.
    pub prompts: HashMap<String, PromptRecord>,
    /// `clientToken` -> `promptId`, so a retried `CreatePrompt` carrying the same
    /// `@idempotencyToken` returns the prompt it already created.
    pub prompt_client_tokens: HashMap<String, String>,
    /// Every numbered prompt version that exists, keyed by the `promptId` it was cut
    /// from and its version number. The pair is the version's identity, so one prompt
    /// never reports another prompt's versions. The prompt's DRAFT version is NOT here:
    /// it is the prompt record itself.
    pub prompt_versions: HashMap<(String, u32), PromptVersionRecord>,
    /// `(promptId, clientToken)` -> the version number that request cut, so a retried
    /// `CreatePromptVersion` carrying the same `@idempotencyToken` reports the version it
    /// already cut rather than cutting a second one. The key carries the prompt the
    /// replay is read at, so one token reused for another prompt cannot displace the
    /// version it already names.
    pub prompt_version_client_tokens: HashMap<(String, String), u32>,
    /// Feeds id minting; never returned to a caller.
    id_sequence: u64,
}

/// The alphabet an `agentId` is drawn from: the modelled `Id` pattern is
/// `^[0-9a-zA-Z]{10}$`.
const ID_ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Characters in a minted `agentId`, from that same pattern.
const ID_LEN: usize = 10;

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The next candidate id drawn from the modelled `^[0-9a-zA-Z]{10}$` alphabet.
    /// Whether it is free is the caller's question: every resource family keys its
    /// own map.
    fn next_id_candidate(&mut self, now: SystemTime) -> String {
        self.id_sequence = self.id_sequence.wrapping_add(1);
        let mut hasher = DefaultHasher::new();
        self.id_sequence.hash(&mut hasher);
        epoch_nanos(now).hash(&mut hasher);
        let mut bits = hasher.finish();
        let mut id = String::with_capacity(ID_LEN);
        for _ in 0..ID_LEN {
            id.push(char::from(ID_ALPHABET[(bits % 62) as usize]));
            bits /= 62;
        }
        id
    }

    /// Mint an unused `agentId` matching the modelled `^[0-9a-zA-Z]{10}$`, or
    /// `None` when every one of [`MINT_ATTEMPTS`] draws collided — answered with a
    /// modelled server error rather than by looping.
    pub fn mint_agent_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.agents.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Mint an unused `flowId`, which the modelled `FlowId` shape constrains to the
    /// same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when every one of
    /// [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_flow_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.flows.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Mint an unused `knowledgeBaseId`, which the modelled `Id` shape
    /// constrains to the same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when
    /// every one of [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_knowledge_base_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.knowledge_bases.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Mint an unused `dataSourceId`, which the modelled `Id` shape constrains
    /// to the same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when every one of
    /// [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_data_source_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.data_sources.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// The stored data source `data_source_id` names, but only when it belongs
    /// to `knowledge_base_id`: a data source is identified by the pair, so one
    /// knowledge base never reports another's.
    pub fn data_source_of(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Option<&DataSourceRecord> {
        self.data_sources
            .get(data_source_id)
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
    }

    /// The same record as [`Self::data_source_of`], for an operation that writes
    /// it: the pair still selects it, so an update never reaches a data source of
    /// another knowledge base.
    pub fn data_source_of_mut(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Option<&mut DataSourceRecord> {
        self.data_sources
            .get_mut(data_source_id)
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
    }

    /// The stored knowledge base `knowledge_base_id` names, but only while no
    /// delete has been accepted for it: a resource on its way out of the store
    /// reports its own `DELETING` status to a read and serves no operation that
    /// acts on it, which is what it answers once the removal has happened too.
    pub fn live_knowledge_base(&self, knowledge_base_id: &str) -> Option<&KnowledgeBaseRecord> {
        self.knowledge_bases
            .get(knowledge_base_id)
            .filter(|record| !record.is_deleting())
    }

    /// The same record as [`Self::live_knowledge_base`], for an operation that
    /// writes it.
    pub fn live_knowledge_base_mut(
        &mut self,
        knowledge_base_id: &str,
    ) -> Option<&mut KnowledgeBaseRecord> {
        self.knowledge_bases
            .get_mut(knowledge_base_id)
            .filter(|record| !record.is_deleting())
    }

    /// The same record as [`Self::data_source_of`], but only while no delete has
    /// been accepted for it or for the knowledge base that contains it — the
    /// same rule as [`Self::live_knowledge_base`].
    pub fn live_data_source_of(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Option<&DataSourceRecord> {
        self.data_source_of(knowledge_base_id, data_source_id)
            .filter(|record| !record.is_deleting())
    }

    /// The same record as [`Self::live_data_source_of`], for an operation that
    /// writes it.
    pub fn live_data_source_of_mut(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Option<&mut DataSourceRecord> {
        self.data_source_of_mut(knowledge_base_id, data_source_id)
            .filter(|record| !record.is_deleting())
    }

    /// Mint an unused `ingestionJobId`, which the modelled `Id` shape
    /// constrains to the same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when
    /// every one of [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_ingestion_job_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.ingestion_jobs.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// The stored ingestion job `ingestion_job_id` names, but only when it ran
    /// for this knowledge base and data source: a job is identified by the
    /// triple, so one data source never reports another's job.
    pub fn ingestion_job_of(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        ingestion_job_id: &str,
    ) -> Option<&IngestionJobRecord> {
        self.ingestion_jobs.get(ingestion_job_id).filter(|record| {
            record.knowledge_base_id == knowledge_base_id
                && record.data_source_id == data_source_id
        })
    }

    /// The same record as [`Self::ingestion_job_of`], for an operation that
    /// writes it.
    pub fn ingestion_job_of_mut(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
        ingestion_job_id: &str,
    ) -> Option<&mut IngestionJobRecord> {
        self.ingestion_jobs
            .get_mut(ingestion_job_id)
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
            })
    }

    /// The data source's job that has not reached a terminal status, if it has
    /// one: a data source ingests one job at a time.
    pub fn running_ingestion_job(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Option<&IngestionJobRecord> {
        self.ingestion_jobs.values().find(|record| {
            record.knowledge_base_id == knowledge_base_id
                && record.data_source_id == data_source_id
                && record.is_running()
        })
    }

    /// How many ingestion jobs this data source has already run, the count the
    /// per-data-source bound is checked against.
    pub fn ingestion_job_count(&self, knowledge_base_id: &str, data_source_id: &str) -> usize {
        self.ingestion_jobs
            .values()
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
            })
            .count()
    }

    /// The stored document `key` names, but only within this knowledge base and
    /// data source: a document is identified by the triple, so one data source
    /// never reports another's document.
    ///
    /// The record of a document whose deletion has run to completion is one of
    /// them: it reports that deletion's terminal status, which is the same
    /// answer as for an identity nothing was ever stored under.
    pub fn document_of(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        key: &DocumentKey,
    ) -> Option<&DocumentRecord> {
        self.documents.get(&(
            knowledge_base_id.to_string(),
            data_source_id.to_string(),
            key.clone(),
        ))
    }

    /// The document `key` names and this data source still HOLDS: as
    /// [`Self::document_of`], minus the record of one whose deletion has
    /// already run to completion.
    ///
    /// This is what an operation asks when it needs the document itself rather
    /// than the status reported for its identity: re-ingesting such an identity
    /// adds a document the data source did not hold.
    pub fn held_document_of(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        key: &DocumentKey,
    ) -> Option<&DocumentRecord> {
        self.document_of(knowledge_base_id, data_source_id, key)
            .filter(|record| record.is_held())
    }

    /// Forget the completed deletions one data source keeps past
    /// [`DELETED_DOCUMENT_RECORDS_KEPT`], oldest first, so the record of a
    /// deletion outlives its document without the store growing for ever.
    fn forget_oldest_deleted_documents(&mut self) {
        let mut per_data_source: HashMap<(String, String), Vec<(SystemTime, DocumentKey)>> =
            HashMap::new();
        for ((knowledge_base_id, data_source_id, key), record) in &self.documents {
            if record.is_held() {
                continue;
            }
            per_data_source
                .entry((knowledge_base_id.clone(), data_source_id.clone()))
                .or_default()
                .push((record.updated_at, key.clone()));
        }
        for ((knowledge_base_id, data_source_id), mut deleted) in per_data_source {
            if deleted.len() <= DELETED_DOCUMENT_RECORDS_KEPT {
                continue;
            }
            // The instant the deletion completed decides which are oldest, ties
            // broken by the document's own identity, so the same records go
            // however the store happens to be laid out.
            deleted.sort_by(|left, right| {
                (left.0, left.1.sort_key()).cmp(&(right.0, right.1.sort_key()))
            });
            let forgotten = deleted.len() - DELETED_DOCUMENT_RECORDS_KEPT;
            for (_, key) in deleted.into_iter().take(forgotten) {
                self.documents.remove(&(
                    knowledge_base_id.clone(),
                    data_source_id.clone(),
                    key,
                ));
            }
        }
    }

    /// Accept the deletion of one document of this data source: it reports the
    /// modelled `DELETING` status from now on and the clock owes it the rest of
    /// the chain, at the end of which the data source holds nothing for this
    /// document — from there every read of the identifier answers the terminal
    /// status the model gives a document that was not found. `None` when this
    /// data source holds no such document, which is not a document to
    /// delete at all.
    ///
    /// A deletion already accepted is not accepted again: the document is
    /// already on its way out, so a repeat of the same delete reports it as it
    /// stands rather than restarting the chain.
    pub fn begin_document_deletion(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
        key: &DocumentKey,
        now: SystemTime,
    ) -> Option<KnowledgeBaseDocumentDetail> {
        let record = self.documents.get_mut(&(
            knowledge_base_id.to_string(),
            data_source_id.to_string(),
            key.clone(),
        ))?;
        if !record.status.is_being_deleted() {
            record.begin_deletion(now);
        }
        Some(record.to_detail())
    }

    /// How many documents this data source already HOLDS, the count the
    /// per-data-source bound is checked against.
    ///
    /// The record of a document whose deletion has run to completion is not one
    /// of them: the data source holds it no more, so it takes none of the room
    /// a fresh ingestion needs.
    pub fn document_count(&self, knowledge_base_id: &str, data_source_id: &str) -> usize {
        self.documents
            .values()
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
                    && record.is_held()
            })
            .count()
    }

    /// How many ingestion tokens this data source already remembers, the count
    /// the per-data-source bound on
    /// [`Self::document_client_tokens`] is checked against.
    pub fn document_token_count(&self, knowledge_base_id: &str, data_source_id: &str) -> usize {
        self.document_client_tokens
            .keys()
            .filter(|(knowledge_base, data_source, _)| {
                knowledge_base == knowledge_base_id && data_source == data_source_id
            })
            .count()
    }

    /// How many data sources `knowledge_base_id` already holds, the count the
    /// per-knowledge-base bound is checked against.
    pub fn data_source_count(&self, knowledge_base_id: &str) -> usize {
        self.data_sources
            .values()
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
            .count()
    }

    /// Apply every status transition the CLOCK has already made due.
    ///
    /// Called by the reporting operations as well as the mutating ones: a change
    /// that was owed at this instant whether or not anyone called is the only
    /// write a read may perform.
    pub fn materialise_due_transitions(&mut self, now: SystemTime) {
        for agent in self.agents.values_mut() {
            if let Some((due, next)) = agent.pending_status {
                if now >= due {
                    agent.agent_status = next;
                    agent.pending_status = None;
                }
            }
        }
        // A flow's preparation was decided when `PrepareFlow` accepted it, so
        // what the clock owes the flow is only the reporting of that outcome and
        // of the findings it reached.
        for flow in self.flows.values_mut() {
            let Some(pending) = flow.pending_preparation.take() else {
                continue;
            };
            if now < pending.due {
                flow.pending_preparation = Some(pending);
                continue;
            }
            flow.status = pending.status;
            flow.validations = pending.validations;
            // The flow changed when the clock made the change due, not when it
            // was noticed.
            flow.updated_at = pending.due;
        }
        for knowledge_base in self.knowledge_bases.values_mut() {
            if let Some((due, next)) = knowledge_base.pending_status {
                if now >= due {
                    knowledge_base.status = next;
                    knowledge_base.pending_status = None;
                }
            }
        }
        for data_source in self.data_sources.values_mut() {
            if let Some((due, next)) = data_source.pending_status {
                if now >= due {
                    data_source.status = next;
                    data_source.pending_status = None;
                }
            }
        }
        // An ingestion job's status is a CHAIN, so a job left unread for long
        // enough is owed more than one of them: each is applied at the instant
        // it came due, and the next is due that instant plus its own span. The
        // chain is finite and strictly advancing, so this ends.
        for job in self.ingestion_jobs.values_mut() {
            while let Some((due, next)) = job.pending_status {
                if now < due {
                    break;
                }
                job.status = next;
                // The job changed when the clock made the change due, not when
                // it was noticed.
                job.updated_at = due;
                if next == IngestionJobStatus::Complete {
                    job.text_ready_at = Some(due);
                    // The run is over, so what it measured is what the job
                    // reports from here on — the counters and the reasons the
                    // documents it refused were not indexed alike.
                    job.statistics = job.completed_statistics.clone();
                    job.failure_reasons = job.completed_failure_reasons.clone();
                }
                job.pending_status = next
                    .owed_next()
                    .map(|(span, after)| (due + span, after));
            }
        }
        // A document's status is a CHAIN too, applied the same way: each
        // transition at the instant it came due, and the next due that instant
        // plus its own span.
        for document in self.documents.values_mut() {
            while let Some((due, next)) = document.pending_status {
                if now < due {
                    break;
                }
                document.status = next;
                // The document changed when the clock made the change due, not
                // when it was noticed.
                document.updated_at = due;
                document.pending_status = next
                    .owed_next()
                    .map(|(span, after)| (due + span, after));
            }
        }
        // A document whose delete chain has reached its end is held no more:
        // the data source counts it against no bound, keeps none of its
        // content and offers it to no retrieval, and its identity is free for a
        // later ingestion to take. The record itself stays, reporting that
        // terminal status — to a targeted read, and to the collection for as
        // long as the deletion is still settling
        // (`DocumentRecord::reports_deletion_until`). The history it forms is
        // bounded: past `DELETED_DOCUMENT_RECORDS_KEPT` per data source the
        // oldest completed deletions are forgotten, and from then on their
        // identities answer as ones nothing was ever stored under.
        self.forget_oldest_deleted_documents();
        // A removal an accepted delete put on the clock is owed the same way:
        // once its instant has passed the resource is gone, whether or not
        // anybody called. A knowledge base takes the data sources it contains
        // with it, which the delete marked for the same instant.
        let due_data_sources: Vec<String> = self
            .data_sources
            .values()
            .filter(|record| record.removal_due_at.is_some_and(|due| now >= due))
            .map(|record| record.data_source_id.clone())
            .collect();
        for data_source_id in due_data_sources {
            self.remove_data_source(&data_source_id);
        }
        let due_knowledge_bases: Vec<String> = self
            .knowledge_bases
            .values()
            .filter(|record| record.removal_due_at.is_some_and(|due| now >= due))
            .map(|record| record.knowledge_base_id.clone())
            .collect();
        for knowledge_base_id in due_knowledge_bases {
            self.remove_knowledge_base(&knowledge_base_id);
        }
    }

    /// Accept the deletion of `knowledge_base_id`: the knowledge base, and every
    /// data source it contains, report the modelled `DELETING` status from now
    /// on, and the clock owes their removal one [`DELETION_SPAN`] later.
    ///
    /// Deleting a knowledge base deletes the data sources inside it: a data
    /// source is identified by the knowledge base it belongs to, so none of them
    /// outlives it.
    pub fn begin_knowledge_base_deletion(&mut self, knowledge_base_id: &str, now: SystemTime) {
        let removal_due_at = now + DELETION_SPAN;
        if let Some(record) = self.knowledge_bases.get_mut(knowledge_base_id) {
            record.status = KnowledgeBaseStatus::Deleting;
            // A status the clock still owed this knowledge base is not owed any
            // more: it is being deleted, not created or updated.
            record.pending_status = None;
            record.removal_due_at = Some(removal_due_at);
            record.updated_at = now;
        }
        for record in self
            .data_sources
            .values_mut()
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
        {
            record.status = DataSourceStatus::Deleting;
            // A status the clock still owed this data source is not owed any
            // more, for the same reason: it is being deleted, not created.
            record.pending_status = None;
            record.removal_due_at = Some(removal_due_at);
            record.updated_at = now;
        }
    }

    /// Accept the deletion of one data source of `knowledge_base_id`: it reports
    /// the modelled `DELETING` status from now on, and the clock owes its
    /// removal one [`DELETION_SPAN`] later.
    pub fn begin_data_source_deletion(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
        now: SystemTime,
    ) {
        let removal_due_at = now + DELETION_SPAN;
        if let Some(record) = self.data_source_of_mut(knowledge_base_id, data_source_id) {
            record.status = DataSourceStatus::Deleting;
            // As above: a status the clock still owed it is not owed any more.
            record.pending_status = None;
            record.removal_due_at = Some(removal_due_at);
            record.updated_at = now;
        }
    }

    /// Forget a data source and everything keyed to it: the ingestion jobs that
    /// ran for it, the documents ingested into it, and the indexes that let a
    /// repeated create or ingestion replay them.
    pub fn remove_data_source(&mut self, data_source_id: &str) -> Option<DataSourceRecord> {
        let record = self.data_sources.remove(data_source_id)?;
        self.ingestion_jobs
            .retain(|_, job| job.data_source_id != data_source_id);
        self.ingestion_job_client_tokens
            .retain(|(_, data_source, _), _| data_source != data_source_id);
        self.data_source_client_tokens
            .retain(|_, id| id != data_source_id);
        self.documents
            .retain(|(_, data_source, _), _| data_source != data_source_id);
        self.document_client_tokens
            .retain(|(_, data_source, _), _| data_source != data_source_id);
        Some(record)
    }

    /// Forget a knowledge base, every data source it contained and every index
    /// that pointed at any of them.
    pub fn remove_knowledge_base(
        &mut self,
        knowledge_base_id: &str,
    ) -> Option<KnowledgeBaseRecord> {
        let record = self.knowledge_bases.remove(knowledge_base_id)?;
        let contained: Vec<String> = self
            .data_sources
            .values()
            .filter(|data_source| data_source.knowledge_base_id == knowledge_base_id)
            .map(|data_source| data_source.data_source_id.clone())
            .collect();
        for data_source_id in contained {
            self.remove_data_source(&data_source_id);
        }
        self.knowledge_base_client_tokens
            .retain(|_, id| id != knowledge_base_id);
        Some(record)
    }

    /// Forget a flow, the versions cut from it, the aliases created on it and
    /// the index that let a repeated create replay it.
    pub fn remove_flow(&mut self, flow_id: &str) -> Option<FlowRecord> {
        let record = self.flows.remove(flow_id)?;
        self.remove_flow_aliases(flow_id);
        self.flow_versions
            .retain(|(versioned_flow_id, _), _| versioned_flow_id != flow_id);
        self.flow_version_client_tokens
            .retain(|(versioned_flow_id, _), _| versioned_flow_id != flow_id);
        self.flow_client_tokens.retain(|_, id| id != flow_id);
        Some(record)
    }

    /// One `ListAgents` page: the agents after `cursor` in creation order, at most
    /// `page_size` of them, plus the cursor to resume from when more remain.
    pub fn page_of_agents(
        &self,
        cursor: Option<&PageCursor>,
        page_size: usize,
    ) -> (Vec<AgentSummary>, Option<String>) {
        let mut ordered: Vec<&AgentRecord> = self
            .agents
            .values()
            .filter(|record| cursor.is_none_or(|cursor| cursor.precedes(record)))
            .collect();
        ordered.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        let page: Vec<&AgentRecord> = ordered.iter().take(page_size).copied().collect();
        let next_token = (ordered.len() > page.len())
            .then(|| page.last().map(|record| record.page_cursor()))
            .flatten();
        (page.iter().map(|record| record.to_summary()).collect(), next_token)
    }

    /// One `ListKnowledgeBases` page: the knowledge bases after `cursor` in
    /// creation order, at most `page_size` of them, plus the token to resume
    /// from when more remain.
    ///
    /// Every knowledge base this account holds is reported, whatever its
    /// status — including one a delete has been accepted for, which reports the
    /// modelled `DELETING` until the removal the clock owes comes due, exactly
    /// as a read of it does. One nothing holds is in no listing: a refused
    /// create leaves no knowledge base to report.
    pub fn page_of_knowledge_bases(
        &self,
        cursor: Option<&KnowledgeBasePageCursor>,
        page_size: usize,
    ) -> (Vec<KnowledgeBaseSummary>, Option<String>) {
        let mut ordered: Vec<&KnowledgeBaseRecord> = self
            .knowledge_bases
            .values()
            .filter(|record| cursor.is_none_or(|cursor| cursor.precedes(record)))
            .collect();
        ordered.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        let page: Vec<&KnowledgeBaseRecord> = ordered.iter().take(page_size.max(1)).copied().collect();
        let next_token = (ordered.len() > page.len())
            .then(|| page.last().map(|record| record.page_cursor()))
            .flatten();
        (
            page.iter().map(|record| record.to_summary()).collect(),
            next_token,
        )
    }

    /// One `ListFlowVersions` page: this flow's versions after `after` in
    /// listing order, at most `page_size` of them, plus the token to resume from
    /// when more remain.
    ///
    /// A flow that exists always has a version to report: its DRAFT version,
    /// which the listing reports first, followed by the numbered versions cut
    /// from it in the order they were cut. An unknown flow has nothing to
    /// enumerate — the caller refuses it as a missing resource before asking.
    pub fn page_of_flow_versions(
        &self,
        flow_id: &str,
        after: Option<VersionCursor>,
        page_size: usize,
    ) -> (Vec<FlowVersionSummary>, Option<String>) {
        let Some(flow) = self.flows.get(flow_id) else {
            return (Vec::new(), None);
        };

        let mut numbered: Vec<&FlowVersionRecord> = self
            .flow_versions
            .values()
            .filter(|record| record.flow_id == flow_id)
            .collect();
        numbered.sort_by_key(|record| record.version);

        let ordered: Vec<(VersionCursor, FlowVersionSummary)> =
            ::std::iter::once((VersionCursor::Draft, flow.to_draft_version_summary()))
                .chain(numbered.into_iter().map(|record| {
                    (
                        VersionCursor::Numbered(record.version),
                        record.to_summary(),
                    )
                }))
                .filter(|(cursor, _)| after.is_none_or(|after| *cursor > after))
                .collect();

        let page: Vec<&(VersionCursor, FlowVersionSummary)> =
            ordered.iter().take(page_size).collect();
        let next_token = (ordered.len() > page.len())
            .then(|| page.last().map(|(cursor, _)| cursor.token()))
            .flatten();
        (
            page.iter().map(|(_, summary)| summary.clone()).collect(),
            next_token,
        )
    }

    /// One `ListKnowledgeBaseDocuments` page: the documents this data source
    /// holds, in document order and from the position `cursor` names, at most
    /// `page_size` of them, plus the token to resume from when more remain.
    ///
    /// The operation "retrieves all the documents contained in a data source",
    /// and a document on its way out of one is still contained in it: a deletion
    /// in flight is reported under the status of that deletion, and one that has
    /// run to its end under its terminal status, so a client confirms from the
    /// collection alone WHICH document a delete removed and that the removal
    /// finished. A deletion stops being reported once it has settled
    /// ([`DocumentRecord::reports_deletion_until`]), which is what makes the
    /// identity leave the collection, while its outcome stays reportable through
    /// the targeted read of that identifier
    /// ([`DELETED_DOCUMENT_RECORDS_KEPT`]).
    ///
    /// No other data source's documents are reported: a document is identified
    /// within the data source it was ingested into. A data source holding none
    /// reports an empty listing — an unknown one has nothing to enumerate,
    /// which the caller refuses as a missing resource before asking.
    pub fn page_of_documents(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        cursor: Option<DocumentPageCursor>,
        page_size: usize,
        now: SystemTime,
    ) -> (Vec<KnowledgeBaseDocumentDetail>, Option<String>) {
        let mut ordered: Vec<&DocumentRecord> = self
            .documents
            .values()
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
                    && record.is_reported_at(now)
            })
            .collect();
        ordered.sort_by(|left, right| left.key.sort_key().cmp(&right.key.sort_key()));

        let reported = cursor.map_or(0, DocumentPageCursor::reported);
        let remaining = ordered.len().saturating_sub(reported);
        let page: Vec<&DocumentRecord> = ordered
            .into_iter()
            .skip(reported)
            .take(page_size.max(1))
            .collect();
        let next_token = (remaining > page.len())
            .then(|| DocumentPageCursor::after(reported + page.len()).map(DocumentPageCursor::token))
            .flatten();
        (
            page.iter()
                .map(|record| DocumentRecord::to_detail(record))
                .collect(),
            next_token,
        )
    }

    /// The stored version the modelled `NumericalVersion` token names on this
    /// flow, or `None` when the flow holds no version cut under that number.
    ///
    /// A numbered version is identified by the pair, so one flow never reports
    /// another flow's version.
    pub fn flow_version_of(&self, flow_id: &str, flow_version: &str) -> Option<&FlowVersionRecord> {
        let version = numerical_version(flow_version)?;
        self.flow_versions.get(&(flow_id.to_string(), version))
    }

    /// Forget the numbered version the modelled `NumericalVersion` token names
    /// on this flow, and the index that let a repeated cut replay it; `None`
    /// when the flow holds no version under that number, which is the one thing
    /// that makes a delete of it a missing resource.
    ///
    /// The flow's numbering is left where it stands: [`FlowRecord::versions_cut`]
    /// is a high-water mark, so the next cut takes the next number rather than
    /// the one the removed version held.
    pub fn remove_flow_version(
        &mut self,
        flow_id: &str,
        flow_version: &str,
    ) -> Option<FlowVersionRecord> {
        let version = numerical_version(flow_version)?;
        let record = self.flow_versions.remove(&(flow_id.to_string(), version))?;
        self.flow_version_client_tokens
            .retain(|(versioned_flow_id, _), cut| versioned_flow_id != flow_id || *cut != version);
        Some(record)
    }

    /// The numbered versions this flow has been cut into.
    pub fn flow_version_count(&self, flow_id: &str) -> usize {
        self.flow_versions
            .values()
            .filter(|record| record.flow_id == flow_id)
            .count()
    }

    /// The number `CreateFlowVersion` cuts this flow's next version as: one past
    /// the highest number the flow has ever reached
    /// ([`FlowRecord::versions_cut`]), so a number a deleted version held is
    /// never handed out again.
    pub fn next_flow_version(&self, flow_id: &str) -> u32 {
        self.flows
            .get(flow_id)
            .map_or(1, |flow| flow.versions_cut.saturating_add(1))
    }

    /// Forget an agent and every index that pointed at it.
    pub fn remove_agent(&mut self, agent_id: &str) -> Option<AgentRecord> {
        let record = self.agents.remove(agent_id)?;
        self.agent_names.remove(&record.agent_name);
        self.client_tokens.retain(|_, id| id != agent_id);
        Some(record)
    }
}

/// The `ValidationException` message an update answers when it would leave a
/// different value in a member its own contract documents as unchangeable once
/// the resource exists.
///
/// Each such member has its OWN sentence, entirely fixed text with nothing
/// interpolated from the request or from state: within a generic
/// `ValidationException` the message is the only discriminator a client has, so
/// a client that branches on "recreate the knowledge base" rather than
/// "re-point the storage" needs the two refusals worded apart.
/// [`IMMUTABLE_MEMBER_MESSAGES`] is the table of the sentences AWS words, and a
/// member not in it — one no recorded refusal words a sentence for — is refused
/// by naming the member path the request carried, which is the one thing known
/// about it.
pub fn immutable_member_message(member: &str) -> String {
    IMMUTABLE_MEMBER_MESSAGES
        .iter()
        .find(|(name, _)| *name == member)
        .map_or_else(
            || format!("{member} cannot be updated once created."),
            |(_, message)| (*message).to_string(),
        )
}

/// The sentence AWS words for each member an update cannot change, by the
/// member path the request carries it in.
///
/// `UpdateKnowledgeBase` documents both of its own ("You can't change the
/// knowledgeBaseConfiguration or storageConfiguration fields, so you must
/// specify the same configurations as when you created the knowledge base",
/// `smithy_query(query="doc",shape="UpdateKnowledgeBase")`) without publishing
/// the wording of either refusal, so the sentences are RECORDED: they are what
/// the service answers, and each is fixed text.
pub const IMMUTABLE_MEMBER_MESSAGES: [(&str, &str); 2] = [
    (
        "knowledgeBaseConfiguration",
        "You cannot modify the Vector knowledge base configuration once it is created.",
    ),
    (
        "storageConfiguration",
        "You cannot modify the storage configuration of the Vector knowledge base once created.",
    ),
];

/// The `ValidationException` message `UpdateKnowledgeBase` answers when the
/// update would leave a different `knowledgeBaseConfiguration.type` than the
/// knowledge base was created with.
///
/// The `type` member is the discriminator of the whole configuration rather than
/// one more value inside it — it decides which sibling member carries the
/// details and whether a vector store is needed at all — so the refusal for
/// changing it is its own sentence, and that sentence is entirely fixed text.
pub fn knowledge_base_type_immutable_message() -> String {
    "You can't modify the type of the knowledge base after creation.".to_string()
}

/// What an update does to a member that cannot change once the resource exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImmutableMemberVerdict {
    /// The update leaves the member exactly as the resource holds it, so it is
    /// served.
    Unchanged,
    /// The update would leave a different value there: a bad request, answered
    /// with the modelled `ValidationException` carrying this message.
    Changed(String),
    /// The codec could not render one of the two values, so they cannot be
    /// compared at all: a server fault rather than something to blame the
    /// request for.
    NotComparable,
}

/// One modelled shape as the JSON `Value` this service's own codec renders for
/// it.
///
/// Rendering through the codec is what makes two shapes comparable at all: the
/// generated shapes carry no `PartialEq`, and a `Debug` rendering would depend
/// on the field order of the Rust struct rather than on the model. A
/// `serde_json::Value` then supplies the equality, by member rather than by
/// text, so two values that differ only in which optional members are absent
/// still compare unequal exactly when the model says they differ.
///
/// A value the codec cannot render answers `None`, so the comparison is reported
/// impossible rather than silently false.
fn rendered_shape(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<::serde_json::Value> {
    use ::aws_smithy_schema::codec::Codec;
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer =
        ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored shape, so two configurations cannot be compared"
        );
        return None;
    }
    match ::serde_json::from_str::<::serde_json::Value>(&serializer.into_string()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            ::tracing::warn!(
                %error,
                "the JSON this service renders for a stored shape did not parse back"
            );
            None
        }
    }
}

/// Whether an update would change `member` — a member its operation documents as
/// unchangeable once the resource exists — from the value `stored` the resource
/// holds to the value `requested` the update would leave there. `member` is the
/// member's path as the request carries it, which is what the refusal names.
///
/// Absence counts: a member the resource holds and the update would drop, or one
/// the resource lacks and the update would add, is as much a change as a
/// different value, because the update writes whatever it carries.
pub fn immutable_member_verdict<T: ::aws_smithy_schema::serde::SerializableStruct>(
    member: &str,
    schema: &::aws_smithy_schema::Schema,
    stored: Option<&T>,
    requested: Option<&T>,
) -> ImmutableMemberVerdict {
    let stored = stored.map(|value| rendered_shape(schema, value));
    let requested = requested.map(|value| rendered_shape(schema, value));
    if matches!(stored, Some(None)) || matches!(requested, Some(None)) {
        return ImmutableMemberVerdict::NotComparable;
    }
    if stored == requested {
        return ImmutableMemberVerdict::Unchanged;
    }
    ImmutableMemberVerdict::Changed(immutable_member_message(member))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState>)
}

/// The region to build an ARN in: the request's SigV4 credential-scope region
/// when it names one the `agentArn` pattern admits, else [`DEFAULT_REGION`].
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 20
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled `date-time` timestamp.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = i64::try_from(epoch_nanos(at) / 1_000_000).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Nanoseconds since the Unix epoch, the total order `ListAgents` pages over.
fn epoch_nanos(at: SystemTime) -> u128 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
