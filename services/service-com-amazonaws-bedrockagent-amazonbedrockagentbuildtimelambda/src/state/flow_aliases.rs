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
//!
//! Not every alias is a caller's: each flow also carries the reserved
//! [`TEST_ALIAS_ID`] alias, which routes to the flow's DRAFT version and is
//! stored with the flow itself
//! ([`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::store_flow`]),
//! so a client can reach the DRAFT for testing before any version is cut. It is
//! an alias like any other to every reader, and out of the caller-settable alias
//! namespace to every writer.

use super::*;

use crate::ops::types::{
    CreateFlowAliasOutput, FlowAliasConcurrencyConfiguration, FlowAliasRoutingConfigurationListItem,
    FlowAliasSummary, GetFlowAliasOutput, UpdateFlowAliasOutput,
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
    /// The reserved test alias of `flow`, as the service creates it with the
    /// flow itself.
    ///
    /// It is an alias like any other — the same record, reported through the
    /// same projections — pinned to the one thing that makes it the TEST alias:
    /// it routes to the flow's DRAFT version, the version `PrepareFlow` prepares
    /// for testing, so it is reachable before any numbered version has been cut.
    /// Its id, name and description are all the reserved
    /// [`TEST_ALIAS_ID`] token, and its ARN is the flow's own ARN with that
    /// alias segment appended, as the modelled `FlowAliasArn` admits. It carries
    /// the flow's creation instant, because that is when it came into being.
    pub fn test_alias_of(flow: &FlowRecord) -> Self {
        Self {
            flow_id: flow.flow_id.clone(),
            alias_id: TEST_ALIAS_ID.to_string(),
            arn: format!("{}/alias/{TEST_ALIAS_ID}", flow.arn),
            name: TEST_ALIAS_ID.to_string(),
            description: Some(TEST_ALIAS_ID.to_string()),
            routing_configuration: vec![FlowAliasRoutingConfigurationListItem {
                flow_version: Some(DRAFT_VERSION.to_string()),
            }],
            concurrency_configuration: None,
            tags: HashMap::new(),
            created_at: flow.created_at,
            updated_at: flow.created_at,
        }
    }

    /// Whether this is the reserved test alias the service creates with the
    /// flow, rather than one a caller created on it.
    pub fn is_test_alias(&self) -> bool {
        self.alias_id == TEST_ALIAS_ID
    }

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

    /// This record as the `UpdateFlowAlias` response. `UpdateFlowAliasResponse`
    /// carries the same members as `CreateFlowAliasResponse`, and like it every
    /// one comes from the stored record — so the answer to an update is the
    /// alias as it now stands, not the request that changed it.
    pub fn to_update_output(&self) -> UpdateFlowAliasOutput {
        UpdateFlowAliasOutput {
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

/// The modelled `FlowAliasIdentifier` pattern, as the model writes it. A refusal
/// quotes it through [`stated_pattern`], which drops the whole-value anchors.
pub const FLOW_ALIAS_IDENTIFIER_PATTERN: &str =
    "^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10})|(TSTALIASID|[0-9a-zA-Z]{10})$";

/// The `ValidationException` message an alias operation answers when
/// `aliasIdentifier` is neither form the modelled shape admits: it names the
/// member and the constraint the value failed, with the value from the request.
pub fn flow_alias_identifier_pattern_message(identifier: &str) -> String {
    format!(
        "Value '{identifier}' at 'aliasIdentifier' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {}",
        stated_pattern(FLOW_ALIAS_IDENTIFIER_PATTERN)
    )
}

/// The test alias every flow carries on AWS, as the modelled `FlowAliasId`
/// (`^(TSTALIASID|[0-9a-zA-Z]{10})$`) spells it — the one alias id this service
/// reserves, admitted as an identifier form and as a segment of the modelled
/// `FlowAliasArn`.
///
/// It is the service's OWN alias of the flow, not a caller's: every stored flow
/// carries it from creation ([`FlowAliasRecord::test_alias_of`]), it is never
/// minted for a caller ([`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::mint_flow_alias_id`]),
/// and it is how a client invokes the flow's DRAFT version for testing before
/// any version has been cut.
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

/// The `ConflictException` message a delete answers when the in-use check its
/// `skipResourceInUseCheck` member governs finds an alias still routing to the
/// resource the request names.
///
/// The sentence is this service's, with two variable parts: `operation`, the
/// operation the caller invoked, and `alias_id`, the alias in state that still
/// references the resource — the one thing the caller has to act on to unblock
/// the delete.
pub fn flow_in_use_by_alias_conflict_message(operation: &str, alias_id: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. The Flow that you specified is currently \
         in use by the following alias: {alias_id}. Retry your request after the alias has \
         finished using the Flow."
    )
}

/// The `ConflictException` message a create or an update answers when the alias
/// name it asks for is one an alias of the same flow already holds.
///
/// Two variable parts beside the operation the caller invoked: `name`, the name
/// the request asked for, and `alias_id`, the alias in state that already holds
/// it — the one thing that tells the caller which alias it collided with.
pub fn flow_alias_name_conflict_message(operation: &str, name: &str, alias_id: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. The name {name} already exists for id \
         {alias_id}. Retry your request with a different name."
    )
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
    /// Store `flow`, together with the reserved test alias it carries from
    /// creation.
    ///
    /// A flow and its test alias come into being TOGETHER — the alias is the
    /// service's own, created with no caller action and routing to the DRAFT
    /// version the flow is created with — so one place stores both and no stored
    /// flow is ever without it. The aliases go with the flow when it is removed,
    /// this one included
    /// ([`Self::remove_flow_aliases`]).
    pub fn store_flow(&mut self, flow: FlowRecord) {
        let test_alias = FlowAliasRecord::test_alias_of(&flow);
        self.flow_aliases.insert(
            (flow.flow_id.clone(), test_alias.alias_id.clone()),
            test_alias,
        );
        self.flows.insert(flow.flow_id.clone(), flow);
    }

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

    /// The stored alias `alias_id` names on `flow_id`, to be CHANGED in place:
    /// the same lookup as [`Self::flow_alias_of`], so an update writes to the
    /// one record every reader reports.
    pub fn flow_alias_of_mut(
        &mut self,
        flow_id: &str,
        alias_id: &str,
    ) -> Option<&mut FlowAliasRecord> {
        self.flow_aliases
            .get_mut(&(flow_id.to_string(), alias_id.to_string()))
    }

    /// How many aliases this flow already holds, the count the per-flow bound is
    /// checked against.
    pub fn flow_alias_count(&self, flow_id: &str) -> usize {
        self.flow_aliases
            .values()
            .filter(|record| record.flow_id == flow_id)
            .count()
    }

    /// The alias of `flow_id` that holds `name`, or `None` when no alias of this
    /// flow carries it.
    ///
    /// An alias's NAME is the caller's to choose and the flow's to hold once: a
    /// name is unique among the aliases of ONE flow, so the search is over that
    /// flow's aliases alone and two flows may each have an alias of the same
    /// name. When several already shared a name the earliest-created one is
    /// reported, so a refusal naming it names the same alias however the store
    /// happens to be iterated.
    pub fn flow_alias_named(&self, flow_id: &str, name: &str) -> Option<&FlowAliasRecord> {
        self.flow_aliases
            .values()
            .filter(|record| record.flow_id == flow_id && record.name == name)
            .min_by_key(|record| record.sort_key())
    }

    /// The same, for a rename: the OTHER alias of this flow that holds `name`,
    /// which is what stops an update from leaving two aliases of one flow under
    /// a single name. An alias is never its own obstacle, so re-asserting the
    /// name it already carries is no conflict.
    pub fn flow_alias_named_other_than(
        &self,
        flow_id: &str,
        name: &str,
        alias_id: &str,
    ) -> Option<&FlowAliasRecord> {
        self.flow_aliases
            .values()
            .filter(|record| {
                record.flow_id == flow_id && record.name == name && record.alias_id != alias_id
            })
            .min_by_key(|record| record.sort_key())
    }

    /// The alias that still references `flow_id`, or `None` when the flow has
    /// none.
    ///
    /// What USES a flow is an alias a CALLER created on it, whichever version of
    /// it the alias routes to: such an alias is how a caller reaches the flow,
    /// so every one of them is a live reference. The reserved test alias is not
    /// one: the service created it with the flow and it goes with the flow, so a
    /// caller has nothing to act on for it. When the flow has several the
    /// earliest-created one is reported, so a refusal naming it names the same
    /// alias however the store happens to be iterated.
    pub fn flow_alias_referencing_flow(&self, flow_id: &str) -> Option<&FlowAliasRecord> {
        self.flow_aliases
            .values()
            .filter(|record| record.flow_id == flow_id && !record.is_test_alias())
            .min_by_key(|record| record.sort_key())
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

    /// The alias of `flow_id` whose routing configuration still points at the
    /// version the modelled `NumericalVersion` token names, or `None` when no
    /// alias of the flow routes there.
    ///
    /// An alias routes to a VERSION of its flow, so the comparison is on the
    /// number the token counts rather than on how it is spelled, and the flow's
    /// DRAFT version — which is no numbered version at all — matches nothing
    /// here. When several aliases route to the same version the earliest-created
    /// one is reported, so a refusal naming it names the same alias however the
    /// store happens to be iterated.
    pub fn flow_alias_routing_to_version(
        &self,
        flow_id: &str,
        flow_version: &str,
    ) -> Option<&FlowAliasRecord> {
        let version = numerical_version(flow_version)?;
        self.flow_aliases
            .values()
            .filter(|record| {
                record.flow_id == flow_id
                    && record.routing_configuration.iter().any(|item| {
                        item.flow_version.as_deref().and_then(numerical_version) == Some(version)
                    })
            })
            .min_by_key(|record| record.sort_key())
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

    /// Forget the alias `alias_id` names on `flow_id`, and the index that let a
    /// repeated create replay it; `None` when the flow holds no alias under that
    /// id, which is the one thing that makes a delete of it a missing resource.
    ///
    /// Only the alias goes: the flow it pointed at and the version it routed to
    /// are resources of their own, which an alias never owned.
    pub fn remove_flow_alias(
        &mut self,
        flow_id: &str,
        alias_id: &str,
    ) -> Option<FlowAliasRecord> {
        let record = self
            .flow_aliases
            .remove(&(flow_id.to_string(), alias_id.to_string()))?;
        self.flow_alias_client_tokens
            .retain(|(aliased_flow_id, _), aliased| {
                aliased_flow_id != flow_id || aliased != alias_id
            });
        Some(record)
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
