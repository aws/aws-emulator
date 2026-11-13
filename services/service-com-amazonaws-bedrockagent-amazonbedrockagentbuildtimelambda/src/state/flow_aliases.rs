// SPDX-License-Identifier: CC0-1.0

//! The flow-alias slice of this service's state: the records `CreateFlowAlias`
//! stores and `GetFlowAlias`/`ListFlowAliases` report, the single projections
//! off those records, and the one place an alias identifier is resolved and a
//! routing target is checked against the versions its flow actually holds.
//!
//! An alias belongs to the flow it was created on — the modelled operations all
//! address it as `(flowIdentifier, aliasIdentifier)` — so the records are keyed
//! by that pair in
//! [`flow_aliases`](super::ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::flow_aliases)
//! and no flow can ever report another flow's alias.

use super::*;

use crate::ops::types::{
    CreateFlowAliasOutput, FlowAliasConcurrencyConfiguration, FlowAliasRoutingConfigurationListItem,
    FlowAliasSummary, GetFlowAliasOutput,
};

/// The most flow aliases one `ListFlowAliases` page can carry: the response
/// member `flowAliasSummaries` is modelled `length ..10`, so a page never
/// exceeds it however large `maxResults` is.
pub const MAX_FLOW_ALIASES_PER_PAGE: usize = 10;

/// The most aliases this store holds for one flow; past it `CreateFlowAlias`
/// answers the modelled `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on aliases per flow, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store, set far above any workflow an
/// emulator serves, and it is what stops one flow's alias state growing without
/// end.
pub const FLOW_ALIAS_QUOTA: usize = 1_000;

/// One stored alias of a flow: the single source of truth for every operation
/// that reports an alias.
#[derive(Debug, Clone)]
pub struct FlowAliasRecord {
    /// The flow this alias belongs to.
    pub flow_id: String,
    /// The alias's own identifier, as the modelled `FlowAliasId` carries it.
    pub alias_id: String,
    /// The ARN of the alias: its flow's ARN with the alias segment appended, as
    /// the modelled `FlowAliasArn` pattern spells it.
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The versions the alias routes to, exactly as the stored request left
    /// them: the modelled `FlowAliasRoutingConfiguration` is `length 1..1`, so
    /// there is one, and it is reported back member for member.
    pub routing_configuration: Vec<FlowAliasRoutingConfigurationListItem>,
    pub concurrency_configuration: Option<FlowAliasConcurrencyConfiguration>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl FlowAliasRecord {
    /// This record as the `CreateFlowAlias` response: every member comes from
    /// the record just stored, never from the request being served.
    pub fn to_create_output(&self) -> CreateFlowAliasOutput {
        CreateFlowAliasOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            routing_configuration: self.routing_configuration.clone(),
            concurrency_configuration: self.concurrency_configuration.clone(),
            flow_id: self.flow_id.clone(),
            id: self.alias_id.clone(),
            arn: self.arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// This record as the `GetFlowAlias` response. `GetFlowAliasResponse`
    /// carries the same members as `CreateFlowAliasResponse`, and like it every
    /// one comes from the stored record.
    pub fn to_get_output(&self) -> GetFlowAliasOutput {
        GetFlowAliasOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            routing_configuration: self.routing_configuration.clone(),
            concurrency_configuration: self.concurrency_configuration.clone(),
            flow_id: self.flow_id.clone(),
            id: self.alias_id.clone(),
            arn: self.arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// This record projected onto the modelled `FlowAliasSummary` shape, the
    /// `ListFlowAliases` item.
    pub fn to_summary(&self) -> FlowAliasSummary {
        FlowAliasSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            routing_configuration: self.routing_configuration.clone(),
            concurrency_configuration: self.concurrency_configuration.clone(),
            flow_id: self.flow_id.clone(),
            id: self.alias_id.clone(),
            arn: self.arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// The total order `ListFlowAliases` reports one flow's aliases in: the
    /// instant each was created, and its id to break a tie between two created
    /// in the same instant.
    pub fn sort_key(&self) -> (u128, String) {
        (epoch_nanos(self.created_at), self.alias_id.clone())
    }
}

/// The modelled `FlowAliasIdentifier` pattern, quoted verbatim so a refusal can
/// name the constraint the value failed.
pub const FLOW_ALIAS_IDENTIFIER_PATTERN: &str =
    "^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10})|(TSTALIASID|[0-9a-zA-Z]{10})$";

/// The `ValidationException` message an alias operation answers when
/// `aliasIdentifier` is neither form the modelled shape admits: it names the
/// member and the constraint the value failed, with the value from the request.
pub fn flow_alias_identifier_pattern_message(identifier: &str) -> String {
    format!(
        "Value '{identifier}' at 'aliasIdentifier' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {FLOW_ALIAS_IDENTIFIER_PATTERN}"
    )
}

/// The test alias every flow carries on AWS, as the modelled `FlowAliasId`
/// (`^(TSTALIASID|[0-9a-zA-Z]{10})$`) spells it. It is admitted as an identifier
/// form, and names an alias this store holds only once something has created
/// one under it.
pub const TEST_ALIAS_ID: &str = "TSTALIASID";

/// The flow and alias a modelled `FlowAliasIdentifier` names, or `None` when the
/// value is neither form the shape admits.
///
/// `FlowAliasIdentifier` is modelled
/// [`FLOW_ALIAS_IDENTIFIER_PATTERN`]: either an alias ARN, whose
/// `flow/<flowId>/alias/<aliasId>` segment carries both ids, or the bare alias
/// id, which carries only its own. The flow is reported when the form carries
/// one, so the caller can tell an alias of ANOTHER flow — a resource the
/// addressed flow does not hold — from a bad request.
pub fn flow_alias_from_identifier(identifier: &str) -> Option<(Option<&str>, &str)> {
    let Some(rest) = identifier.strip_prefix("arn:") else {
        let bare = identifier == TEST_ALIAS_ID || is_minted_id(identifier);
        return bare.then_some((None, identifier));
    };
    // partition: aws | service | region | account | flow/<flowId>/alias/<aliasId>
    let segments: Vec<&str> = rest.split(':').collect();
    let [partition, service, region, account, resource] = segments[..] else {
        return None;
    };
    let region_shaped = (1..=20).contains(&region.len())
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if partition != "aws"
        || service != "bedrock"
        || !region_shaped
        || account.len() != 12
        || !account.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (flow_id, alias_id) = resource.strip_prefix("flow/")?.split_once("/alias/")?;
    (is_minted_id(flow_id) && is_minted_id(alias_id)).then_some((Some(flow_id), alias_id))
}

/// The `ResourceNotFoundException` message an operation answers when the alias
/// the request names is not stored on the flow it addressed.
pub fn flow_alias_not_found_message(operation: &str) -> String {
    resource_not_found_message(operation, "FlowAlias")
}

/// The `ResourceNotFoundException` message an operation answers when a flow
/// version it was asked to resolve — the version an alias routes to — is not one
/// the addressed flow holds.
pub fn flow_version_not_found_message(operation: &str) -> String {
    resource_not_found_message(operation, "FlowVersion")
}

/// A `ListFlowAliases` resume cursor: how many of one flow's aliases the listing
/// has already reported, in the order
/// [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::page_of_flow_aliases`]
/// lists them.
///
/// The token names a POSITION in that order rather than an alias, so a page
/// resumes even if the alias it stopped at has since been deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowAliasPageCursor {
    reported: usize,
}

impl FlowAliasPageCursor {
    /// Decode a `nextToken` a previous page handed out. `None` when the token is
    /// not one [`Self::token`] renders: a position is a plain decimal count of
    /// the aliases already reported, so nothing else names a position in this
    /// listing.
    pub fn parse(token: &str) -> Option<Self> {
        if !is_counted_token_position(token) {
            return None;
        }
        token.parse().ok().map(|reported| Self { reported })
    }

    /// The cursor that resumes after `reported` aliases, or `None` when none has
    /// been reported yet — the position a listing starts at, which is what an
    /// absent `nextToken` already names.
    fn after(reported: usize) -> Option<Self> {
        (reported > 0).then_some(Self { reported })
    }

    /// This position as the opaque `nextToken` a page hands out.
    pub fn token(self) -> String {
        self.reported.to_string()
    }
}

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// Mint an unused `flowAliasId` for this flow, which the modelled
    /// `FlowAliasId` shape constrains to the same `^[0-9a-zA-Z]{10}$` as a
    /// `flowId` (beside the reserved test-alias token). `None` when every one of
    /// `MINT_ATTEMPTS` draws collided.
    pub fn mint_flow_alias_id(&mut self, flow_id: &str, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if id != TEST_ALIAS_ID
                && !self
                    .flow_aliases
                    .contains_key(&(flow_id.to_string(), id.clone()))
            {
                return Some(id);
            }
        }
        None
    }

    /// The stored alias `alias_id` names, but only when it belongs to
    /// `flow_id`: an alias is identified by the pair, so one flow never reports
    /// another's alias.
    pub fn flow_alias_of(&self, flow_id: &str, alias_id: &str) -> Option<&FlowAliasRecord> {
        self.flow_aliases
            .get(&(flow_id.to_string(), alias_id.to_string()))
    }

    /// How many aliases this flow already holds, the count the per-flow bound is
    /// checked against.
    pub fn flow_alias_count(&self, flow_id: &str) -> usize {
        self.flow_aliases
            .values()
            .filter(|record| record.flow_id == flow_id)
            .count()
    }

    /// Whether `flow_id` holds the version the modelled
    /// `FlowAliasRoutingConfigurationListItem$flowVersion` token names.
    ///
    /// The member's shape is `pattern ^(DRAFT|[0-9]{0,4}[1-9][0-9]{0,4})$`, so a
    /// token is either the flow's DRAFT version — which every stored flow has,
    /// because the DRAFT version IS the flow record — or a numbered version,
    /// which exists only while this flow holds the version that number was cut
    /// as. Anything else names no version of this flow at all.
    pub fn flow_version_exists(&self, flow_id: &str, flow_version: &str) -> bool {
        if flow_version == DRAFT_VERSION {
            return self.flows.contains_key(flow_id);
        }
        self.flow_version_of(flow_id, flow_version).is_some()
    }

    /// One `ListFlowAliases` page: this flow's aliases in creation order and
    /// from the position `cursor` names, at most `page_size` of them, plus the
    /// token to resume from when more remain.
    ///
    /// Only the addressed flow's aliases are reported: a flow holding none
    /// reports an empty listing, and an unknown flow has nothing to enumerate —
    /// which the caller refuses as a missing resource before asking.
    pub fn page_of_flow_aliases(
        &self,
        flow_id: &str,
        cursor: Option<FlowAliasPageCursor>,
        page_size: usize,
    ) -> (Vec<FlowAliasSummary>, Option<String>) {
        let mut ordered: Vec<&FlowAliasRecord> = self
            .flow_aliases
            .values()
            .filter(|record| record.flow_id == flow_id)
            .collect();
        ordered.sort_by_key(|record| record.sort_key());

        let reported = cursor.map_or(0, |cursor| cursor.reported);
        let remaining = ordered.len().saturating_sub(reported);
        let page: Vec<&FlowAliasRecord> = ordered
            .into_iter()
            .skip(reported)
            .take(page_size.max(1))
            .collect();
        let next_token = (remaining > page.len())
            .then(|| {
                FlowAliasPageCursor::after(reported + page.len()).map(FlowAliasPageCursor::token)
            })
            .flatten();
        (
            page.iter().map(|record| record.to_summary()).collect(),
            next_token,
        )
    }

    /// Forget every alias of `flow_id`, and the index that let a repeated create
    /// replay one: an alias is identified by the flow it belongs to, so none of
    /// them outlives it.
    pub fn remove_flow_aliases(&mut self, flow_id: &str) {
        self.flow_aliases
            .retain(|(aliased_flow_id, _), _| aliased_flow_id != flow_id);
        self.flow_alias_client_tokens
            .retain(|(aliased_flow_id, _), _| aliased_flow_id != flow_id);
    }
}
