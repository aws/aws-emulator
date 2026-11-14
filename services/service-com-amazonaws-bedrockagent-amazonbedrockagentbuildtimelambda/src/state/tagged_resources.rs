// SPDX-License-Identifier: CC0-1.0

//! The TAGGABLE resources of this plane: which ARN names which stored resource, and
//! the tags that resource holds.
//!
//! `ListTagsForResource`, `TagResource` and `UntagResource` address a resource by its
//! ARN rather than by the identifier its own operations take, and the modelled
//! `TaggableResourcesArn` enumerates the resource types this plane's tagging admits —
//! the resources this plane MINTS: an agent, an agent alias, a knowledge base, a flow,
//! a flow alias, a prompt and a retrieval instruction
//! (`smithy_query(query="shape",shape="TaggableResourcesArn")`). So one reader resolves
//! such an ARN to the record that holds the tags, and every tagging operation reports
//! the tags off the record the resource's own reads report, rather than keeping a second
//! copy of them.
//!
//! The ARN is judged HERE rather than at the request boundary (`skip_validation` on
//! `resourceArn` in `src/ops/types/`): the published `TaggableResourcesArn` expression
//! spells the account field `/d{12}` and every identifier `[A-Z0-9]{10}`, which no ARN
//! this plane mints satisfies — a `knowledgeBaseId` is the modelled `^[0-9a-zA-Z]{10}$`
//! and an account is twelve digits — and the recorded answer for a knowledge base's own
//! ARN is the tags, not a refusal. What IS enforced is the syntax that expression spells
//! out: a `bedrock` ARN of a twelve-digit account whose resource field names one of those
//! types.

use super::*;

/// One resource of this plane that a tagging operation can address, as its ARN names it.
///
/// The identifiers are the ones the stored records are keyed by, so a resolved ARN
/// selects the same record the resource's own reads report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableResource {
    /// `agent/<agentId>`.
    Agent(String),
    /// `agent-alias/<agentId>/<agentAliasId>` — an alias belongs to its agent.
    AgentAlias(String, String),
    /// `knowledge-base/<knowledgeBaseId>`.
    KnowledgeBase(String),
    /// `flow/<flowId>`.
    Flow(String),
    /// `flow/<flowId>/alias/<flowAliasId>` — an alias belongs to its flow, which is how
    /// the modelled `FlowAliasArn` spells it.
    FlowAlias(String, String),
    /// `prompt/<promptId>`, with the version the identifier may carry after a `:`.
    Prompt(String),
    /// `retrieval-instruction/<retrievalInstructionId>`.
    RetrievalInstruction(String),
}

/// The resource-type field of each taggable resource of this plane, from the modelled
/// `TaggableResourcesArn` expression.
const AGENT_TYPE: &str = "agent";
const AGENT_ALIAS_TYPE: &str = "agent-alias";
const KNOWLEDGE_BASE_TYPE: &str = "knowledge-base";
const FLOW_TYPE: &str = "flow";
const PROMPT_TYPE: &str = "prompt";
const RETRIEVAL_INSTRUCTION_TYPE: &str = "retrieval-instruction";

/// The segment a flow ALIAS's ARN carries between the flow's id and the alias's own, as
/// the modelled `FlowAliasArn` spells it (`flow/<id>/alias/<id>`).
const FLOW_ALIAS_SEGMENT: &str = "alias";

/// The resource a tagging operation's `resourceArn` names, or `None` when the value is
/// outside the syntax the modelled `TaggableResourcesArn` spells: an ARN of the
/// `bedrock` namespace, of a twelve-digit account, whose resource field names one of this
/// plane's taggable resource types.
///
/// The six ARN fields are read through `service_common::arn`, the one ARN parse of this
/// workspace (it leaves the whole resource field, colons included, as the last field);
/// what stays here is this plane's own part — the Region grammar every ARN pattern of
/// this model states, the account-id rule, and which resource fields name a taggable
/// resource.
///
/// Whether the resource EXISTS is a different question ([`taggable_resource_tags`]): an
/// ARN of the right syntax naming nothing stored is a missing resource, not a bad
/// request.
pub fn taggable_resource(arn: &str) -> Option<TaggableResource> {
    let parsed = ::service_common::arn::parse_of(arn, BEDROCK_ARN_NAMESPACE)?;
    if !is_partition(parsed.partition)
        || ::service_common::arn::region_label(parsed.region).is_none()
        || !::service_common::arn::is_account_id(parsed.account)
    {
        return None;
    }
    let mut segments = parsed.resource.split('/');
    let resource_type = segments.next()?;
    let rest: Vec<&str> = segments.collect();
    let resolved = match (resource_type, rest.as_slice()) {
        (AGENT_TYPE, [agent_id]) => TaggableResource::Agent((*agent_id).to_string()),
        (AGENT_ALIAS_TYPE, [agent_id, alias_id]) => {
            TaggableResource::AgentAlias((*agent_id).to_string(), (*alias_id).to_string())
        }
        (KNOWLEDGE_BASE_TYPE, [knowledge_base_id]) => {
            TaggableResource::KnowledgeBase((*knowledge_base_id).to_string())
        }
        (FLOW_TYPE, [flow_id]) => TaggableResource::Flow((*flow_id).to_string()),
        (FLOW_TYPE, [flow_id, FLOW_ALIAS_SEGMENT, alias_id]) => {
            TaggableResource::FlowAlias((*flow_id).to_string(), (*alias_id).to_string())
        }
        // A prompt ARN may carry the version after its id (`prompt/<id>:<version>`), and
        // the prompt the tags belong to is the one that id names.
        (PROMPT_TYPE, [prompt_id]) => TaggableResource::Prompt(
            prompt_id
                .split_once(':')
                .map_or(*prompt_id, |(id, _version)| id)
                .to_string(),
        ),
        (RETRIEVAL_INSTRUCTION_TYPE, [instruction_id]) => {
            TaggableResource::RetrievalInstruction((*instruction_id).to_string())
        }
        _ => return None,
    };
    if resolved.identifiers().iter().any(|id| id.is_empty()) {
        return None;
    }
    Some(resolved)
}

impl TaggableResource {
    /// The resource type this ARN names, as this plane's own refusals spell a resource
    /// type (`Flow` for a flow, as `flow_not_found_message` states it).
    pub fn resource_type_name(&self) -> &'static str {
        match self {
            Self::Agent(_) => "Agent",
            Self::AgentAlias(_, _) => "AgentAlias",
            Self::KnowledgeBase(_) => "KnowledgeBase",
            Self::Flow(_) => "Flow",
            Self::FlowAlias(_, _) => "FlowAlias",
            Self::Prompt(_) => "Prompt",
            Self::RetrievalInstruction(_) => "RetrievalInstruction",
        }
    }

    /// The identifiers inside this ARN, which are what has to be non-empty for it to
    /// name a resource at all.
    fn identifiers(&self) -> Vec<&str> {
        match self {
            Self::Agent(id)
            | Self::KnowledgeBase(id)
            | Self::Flow(id)
            | Self::Prompt(id)
            | Self::RetrievalInstruction(id) => vec![id.as_str()],
            Self::AgentAlias(first, second) | Self::FlowAlias(first, second) => {
                vec![first.as_str(), second.as_str()]
            }
        }
    }
}

/// The ARN namespace every Bedrock-family model shares, which an ARN this plane's tagging
/// admits carries in its vendor field (`service_common::bedrock::ARN_NAMESPACE`).
const BEDROCK_ARN_NAMESPACE: &str = ::service_common::bedrock::ARN_NAMESPACE;

/// Whether an ARN's partition field is one: `aws`, or `aws-` and a suffix, as every ARN
/// pattern of this model spells it (`arn:aws(-[^:]+)?`).
fn is_partition(partition: &str) -> bool {
    partition == "aws"
        || partition
            .strip_prefix("aws-")
            .is_some_and(|suffix| !suffix.is_empty())
}

/// The `ValidationException` message a tagging operation answers for a `resourceArn`
/// outside the syntax the modelled `TaggableResourcesArn` spells: the member and the
/// constraint the value failed, with the value from the request.
pub fn taggable_resource_arn_pattern_message(resource_arn: &str) -> String {
    format!(
        "Value '{resource_arn}' at 'resourceArn' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {TAGGABLE_RESOURCES_ARN_PATTERN}"
    )
}

/// The expression the refusal above quotes: the modelled `TaggableResourcesArn`
/// `@pattern`, as this service's model publishes it.
const TAGGABLE_RESOURCES_ARN_PATTERN: &str = concat!(
    r"(^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:",
    r"(agent|agent-alias|knowledge-base|flow|prompt)/[A-Z0-9]{10}(?:/[A-Z0-9]{10})?$",
    r"|^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:flow/([A-Z0-9]{10})/alias/([A-Z0-9]{10})$",
    r"|^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:prompt/([A-Z0-9]{10})?(?::/d+)?$",
    r"|^arn:aws(-cn|-us-gov|-eusc|-iso(-[b-f])?)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}",
    r":retrieval-instruction/[a-z0-9]{10}$)",
);

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The tags the resource `resource` names holds, or `None` when this store holds no
    /// such resource — which is the missing resource a tagging operation refuses with.
    ///
    /// Every kind is read from the record its own operations report, so the tags a
    /// tagging call reports are the ones the create stored and nothing else. A resource
    /// kind no operation of this emulator stores yet holds no record, so an ARN naming
    /// one is as absent as an identifier nothing holds.
    pub fn taggable_resource_tags(
        &self,
        resource: &TaggableResource,
    ) -> Option<&HashMap<String, String>> {
        match resource {
            TaggableResource::Agent(agent_id) => {
                self.agents.get(agent_id).map(|record| &record.tags)
            }
            TaggableResource::KnowledgeBase(knowledge_base_id) => self
                .knowledge_bases
                .get(knowledge_base_id)
                .map(|record| &record.tags),
            TaggableResource::Flow(flow_id) => self.flows.get(flow_id).map(|record| &record.tags),
            TaggableResource::FlowAlias(flow_id, alias_id) => self
                .flow_aliases
                .get(&(flow_id.clone(), alias_id.clone()))
                .map(|record| &record.tags),
            // An agent ALIAS, a prompt and a retrieval instruction are not stored by any
            // operation of this emulator yet: there is no record to read tags off.
            TaggableResource::AgentAlias(_, _)
            | TaggableResource::Prompt(_)
            | TaggableResource::RetrievalInstruction(_) => None,
        }
    }

    /// The same map, for the two operations that WRITE it (`TagResource` merges into it,
    /// `UntagResource` removes keys from it): the tags live on the resource's own record,
    /// so a tag a caller sets is read back by the same reader that reports the ones its
    /// create stored.
    pub fn taggable_resource_tags_mut(
        &mut self,
        resource: &TaggableResource,
    ) -> Option<&mut HashMap<String, String>> {
        match resource {
            TaggableResource::Agent(agent_id) => {
                self.agents.get_mut(agent_id).map(|record| &mut record.tags)
            }
            TaggableResource::KnowledgeBase(knowledge_base_id) => self
                .knowledge_bases
                .get_mut(knowledge_base_id)
                .map(|record| &mut record.tags),
            TaggableResource::Flow(flow_id) => {
                self.flows.get_mut(flow_id).map(|record| &mut record.tags)
            }
            TaggableResource::FlowAlias(flow_id, alias_id) => self
                .flow_aliases
                .get_mut(&(flow_id.clone(), alias_id.clone()))
                .map(|record| &mut record.tags),
            TaggableResource::AgentAlias(_, _)
            | TaggableResource::Prompt(_)
            | TaggableResource::RetrievalInstruction(_) => None,
        }
    }
}

/// The most tags one resource of this plane holds; past it `TagResource` answers the
/// modelled `ServiceQuotaExceededException` instead of growing.
///
/// The modelled `TagsMap` publishes no bound of its own, so the limit is AWS's documented
/// one for tagging a resource — fifty user-created tags per resource, the ceiling every AWS
/// tagging surface states — which is also what stops one resource's tag state growing
/// without end as it is tagged again and again.
pub const TAG_QUOTA: usize = 50;

/// The `ServiceQuotaExceededException` message `TagResource` answers when the tags it
/// would leave on the resource pass [`TAG_QUOTA`].
///
/// AWS's own wording for this error class, which is ENTIRELY fixed text: it names no
/// resource, no count and no limit, so there is no variable part to fill and nothing the
/// caller sent can reach it. The quota is cumulative per resource, so one sentence covers
/// both halves of it — a single request carrying too many tags, and a request whose tags
/// would take the resource past the bound. The same sentence the Bedrock family's other
/// tagging surfaces answer for it.
pub const TAG_QUOTA_MESSAGE: &str = "The number of the provided tags is beyond the limit, \
                                     or the number of total tags you are trying to attach \
                                     to the specified resource exceeds the limit.";

/// [`TAG_QUOTA_MESSAGE`], as the owned `String` the modelled error member holds.
pub fn tag_quota_message() -> String {
    TAG_QUOTA_MESSAGE.to_string()
}
