// SPDX-License-Identifier: CC0-1.0

//! The coverage view: which protected resources this service's entities cover, and through
//! which deployments.
//!
//! Nothing here is stored. A coverage row is COMPUTED from the records the entity
//! operations persist — a deployment, the scope it names and the policies it applies — so
//! `ListEntityCoverages` and `ListProtectedResourceCoverages` answer two questions about
//! one derivation and can never disagree: the first asks which resources one entity
//! covers, the second which entities cover one resource.
//!
//! Kept beside the entity records rather than in either `src/ops/*.rs` so both operations
//! read the same derivation, exactly as the two read paths of an entity share its record.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use super::{
    ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState as State,
    FIREWALL_TYPE_SHIELD_ADVANCED, FIREWALL_TYPE_WAF, RESOURCE_TYPE_POLICY, STATUS_ACTIVE,
    encode_next_token, to_date_time, version_qualified,
};
use crate::ops::types::ResourceCoverageSummary;

/// `CoverageStatus` for an entity that contributes to a resource's effective configuration
/// — the model: "For firewall types that combine policies, such as AWS WAF and AWS Shield
/// Advanced, every policy that covers the resource is IN_EFFECT."
pub const COVERAGE_STATUS_IN_EFFECT: &str = "IN_EFFECT";

/// `CoverageStatus` for an entity a higher-priority one displaces — the model: "For
/// firewall types that apply a single policy, only the highest-priority policy is IN_EFFECT
/// and the others are SUPERSEDED."
pub const COVERAGE_STATUS_SUPERSEDED: &str = "SUPERSEDED";

/// The largest page either coverage listing can answer with, and so the page size used
/// when the caller sends no `maxResults`: `ResourceCoverageSummaryList` is modelled
/// `@length(max: 100)`, so a larger page could not be serialized at all.
pub const MAX_RESOURCE_COVERAGE_PAGE: usize = 100;

/// The most coverage rows this emulator derives for one request, after which the
/// derivation stops rather than materialising a row per (policy, resource) pair for as many
/// pairs as the stores can be filled with.
///
/// A CHOSEN emulator bound, not a quota this service documents: every store the derivation
/// walks is itself bounded, but the row count is their PRODUCT — deployments times the
/// policies each applies times the resources its scope names — so the product needs a
/// ceiling of its own. It sits far above what those stores hold through ordinary use.
pub const MAX_RESOURCE_COVERAGE_ROWS: usize = 10_000;

/// `@length` of the modelled `AccountId`, which is `^(?:[0-9]{12})$`: the account segment
/// of a resource ARN is reported as `resourceOwnerAccountId` only when it is one, since a
/// value outside that pattern is one no answer could serialize.
const ACCOUNT_ID_LENGTH: usize = 12;

/// Separates the two identifiers of a coverage row's key ([`coverage_key`]). A control
/// character, so the key of one row sorts against another's exactly as the pair
/// `(resourceArn, entityArn)` does: no ARN carries one.
const COVERAGE_KEY_SEPARATOR: char = '\u{1}';

/// One coverage row: one entity covering one protected resource.
///
/// The entity is a POLICY — the model, of the `entityType` filter: "The only supported
/// value is Policy" — so a row names the policy that covers the resource, the firewall type
/// it enforces and every deployment through which it reaches the resource.
#[derive(Debug, Clone)]
pub struct ResourceCoverage {
    /// The protected resource covered: an ARN a scope's resource selection names.
    pub resource_arn: String,
    /// The covering entity's ARN — a `policyArn`.
    pub entity_arn: String,
    /// `ScopeResourceType` — the resource-type key the scope selected this resource under.
    pub resource_type: String,
    /// `PolicyFirewallType` — the covering policy's firewall type.
    pub firewall_type: String,
    /// The policy's `priority`, which decides which row is in effect for a firewall type
    /// that applies a single policy ("A lower number indicates a higher priority").
    /// Not a member of the answer: it is what [`ResourceCoverage::coverage_status`] is
    /// computed from.
    priority: i32,
    /// `CoverageStatus`, computed by [`State::resource_coverages`] once every row for the
    /// resource and firewall type is known.
    coverage_status: String,
    /// The ARNs of the deployments through which the entity covers the resource.
    pub deployment_arns: BTreeSet<String>,
    /// When this coverage was last evaluated: the most recent change among the records it
    /// is derived from, so a read reports the same instant until one of them changes.
    pub updated_at: SystemTime,
}

impl ResourceCoverage {
    /// `CoverageStatus` as this row reports it.
    pub fn coverage_status(&self) -> &str {
        &self.coverage_status
    }

    /// This row as the `ResourceCoverageSummary` both coverage listings return.
    pub fn summary(&self) -> ResourceCoverageSummary {
        ResourceCoverageSummary {
            resource_arn: Some(self.resource_arn.clone()),
            entity_type: Some(RESOURCE_TYPE_POLICY.to_string()),
            entity_arn: Some(self.entity_arn.clone()),
            resource_owner_account_id: account_id_in(&self.resource_arn).map(str::to_string),
            resource_type: Some(self.resource_type.clone()),
            firewall_type: Some(self.firewall_type.clone()),
            coverage_status: Some(self.coverage_status.clone()),
            deployment_arns: Some(self.deployment_arns.iter().cloned().collect()),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

impl State {
    /// Every coverage row this store's records imply, keyed by [`coverage_key`] and so
    /// walked in a stable order — which is what lets either listing resume exactly where
    /// its previous page stopped.
    ///
    /// A row exists for a (policy, resource) pair when a PUBLISHED deployment applies that
    /// published policy to a scope whose resource selection names that resource: the model
    /// says of `EntityStatus` that `ACTIVE` is "published, in use" and `DRAFT`
    /// "unpublished, editable", so a draft deployment or a draft policy enforces nothing
    /// and covers nothing.
    ///
    /// The resources a scope selects are the ARNs it names explicitly. A selection that
    /// instead asks for every resource of a type, or describes one by criteria, names no
    /// resource this service holds — the resources themselves belong to the services that
    /// own them — so such a selection contributes no row rather than an invented one.
    ///
    /// The derivation is bounded by [`MAX_RESOURCE_COVERAGE_ROWS`], so no filling of the
    /// stores can make one read grow this process without limit.
    pub fn resource_coverages(&self) -> BTreeMap<String, ResourceCoverage> {
        let mut rows: BTreeMap<String, ResourceCoverage> = BTreeMap::new();

        for deployment in self
            .deployments
            .values()
            .filter(|deployment| deployment.status == STATUS_ACTIVE)
        {
            // "A deployment has exactly one scope": its resource selection is what decides
            // which resources the deployment's policies reach.
            let Some(scope) = deployment
                .associated_scope_list
                .first()
                .and_then(|association| self.scope_by_identifier(&association.scope_arn))
            else {
                continue;
            };
            let selected = selected_resources(scope);
            if selected.is_empty() {
                continue;
            }

            for policy in deployment
                .associated_policy_list
                .iter()
                .filter_map(|association| self.policy_by_identifier(&association.policy_arn))
                .filter(|policy| policy.status == STATUS_ACTIVE)
            {
                for (resource_arn, resource_type) in &selected {
                    let key = coverage_key(resource_arn, &policy.policy_arn);
                    // The derivation is bounded: past its ceiling an unseen pair adds no
                    // row, so no filling of the stores can grow this walk without limit.
                    if !rows.contains_key(&key) && rows.len() >= MAX_RESOURCE_COVERAGE_ROWS {
                        continue;
                    }
                    let evaluated = [deployment.updated_at, policy.updated_at, scope.updated_at]
                        .into_iter()
                        .max()
                        .unwrap_or(policy.updated_at);
                    let row = rows.entry(key).or_insert_with(|| ResourceCoverage {
                        resource_arn: resource_arn.clone(),
                        entity_arn: policy.policy_arn.clone(),
                        resource_type: resource_type.clone(),
                        firewall_type: policy.firewall_type.clone(),
                        priority: policy.priority,
                        // Filled in below, once every row for this resource and firewall
                        // type is known.
                        coverage_status: COVERAGE_STATUS_IN_EFFECT.to_string(),
                        deployment_arns: BTreeSet::new(),
                        updated_at: evaluated,
                    });
                    row.deployment_arns
                        .insert(deployment.deployment_arn.clone());
                    row.updated_at = row.updated_at.max(evaluated);
                }
            }
        }

        apply_coverage_status(&mut rows);
        rows
    }
}

/// The resources a scope selects explicitly: each ARN it names, with the
/// `ScopeResourceType` key it was named under.
///
/// An ARN the same resource scope excludes is not selected, whatever else names it: the
/// model says of `ResourceScope` that "exactly one of includeAll, include, or exclude is
/// set", so a request carrying both is answered by honouring the exclusion.
fn selected_resources(scope: &super::Scope) -> Vec<(String, String)> {
    let mut selected: Vec<(String, String)> = Vec::new();
    // `resourceScopes` is a map, so it is walked in a stable order here rather than in
    // whatever order its hashing yields.
    let resource_types: BTreeSet<&String> = scope
        .scope_configuration
        .resource_scopes
        .keys()
        .collect();
    for resource_type in resource_types {
        let Some(resource_scope) = scope.scope_configuration.resource_scopes.get(resource_type)
        else {
            continue;
        };
        let excluded: BTreeSet<&str> = resource_scope
            .exclude
            .as_ref()
            .and_then(|set| set.explicit_arns.as_deref())
            .map(|arns| arns.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let included = resource_scope
            .include
            .as_ref()
            .and_then(|set| set.explicit_arns.as_deref())
            .unwrap_or_default();
        for arn in included {
            if excluded.contains(arn.as_str()) {
                continue;
            }
            selected.push((arn.clone(), resource_type.clone()));
        }
    }
    selected
}

/// Fill in every row's `coverageStatus`, the model's rule: "For firewall types that combine
/// policies, such as AWS WAF and AWS Shield Advanced, every policy that covers the resource
/// is IN_EFFECT. For firewall types that apply a single policy, only the highest-priority
/// policy is IN_EFFECT and the others are SUPERSEDED."
///
/// So the status of a row depends on the OTHER rows for the same resource and firewall
/// type, which is why it is computed once every row is known rather than as each is built.
fn apply_coverage_status(rows: &mut BTreeMap<String, ResourceCoverage>) {
    // For each (resource, firewall type) that applies a single policy, the key of the row
    // whose policy wins: the lowest `priority` ("A lower number indicates a higher
    // priority"), and among equal priorities the row whose key sorts first, so the answer
    // does not depend on the order the records were walked in.
    let mut in_effect: BTreeMap<(String, String), (i32, String)> = BTreeMap::new();
    for (key, row) in rows.iter() {
        if firewall_type_combines_policies(&row.firewall_type) {
            continue;
        }
        let contest = (row.resource_arn.clone(), row.firewall_type.clone());
        let candidate = (row.priority, key.clone());
        in_effect
            .entry(contest)
            .and_modify(|winner| {
                if candidate < *winner {
                    *winner = candidate.clone();
                }
            })
            .or_insert(candidate);
    }

    for (key, row) in rows.iter_mut() {
        if firewall_type_combines_policies(&row.firewall_type) {
            row.coverage_status = COVERAGE_STATUS_IN_EFFECT.to_string();
            continue;
        }
        let contest = (row.resource_arn.clone(), row.firewall_type.clone());
        let wins = in_effect
            .get(&contest)
            .is_some_and(|(_, winner)| winner == key);
        row.coverage_status = if wins {
            COVERAGE_STATUS_IN_EFFECT.to_string()
        } else {
            COVERAGE_STATUS_SUPERSEDED.to_string()
        };
    }
}

/// Whether a `PolicyFirewallType` COMBINES the policies that cover one resource, so that
/// each of them is in effect — the model names AWS WAF and AWS Shield Advanced as the
/// combining types; every other firewall type applies a single policy, and for those only
/// the highest-priority one is in effect.
pub fn firewall_type_combines_policies(firewall_type: &str) -> bool {
    firewall_type == FIREWALL_TYPE_WAF || firewall_type == FIREWALL_TYPE_SHIELD_ADVANCED
}

/// The key of the coverage row for `resource_arn` and `entity_arn`: the pair, so one row
/// exists per (resource, entity) however many deployments connect them, and so the rows
/// are walked grouped by resource.
pub fn coverage_key(resource_arn: &str, entity_arn: &str) -> String {
    format!("{resource_arn}{COVERAGE_KEY_SEPARATOR}{entity_arn}")
}

/// The AWS account that owns the resource an ARN names: the ARN's account segment, when it
/// is an `AccountId` as the model declares one (`^(?:[0-9]{12})$`) — `None` otherwise,
/// including the empty account segment an ARN of a global resource carries.
pub fn account_id_in(arn: &str) -> Option<&str> {
    let account = arn.split(':').nth(4)?;
    (account.len() == ACCOUNT_ID_LENGTH && account.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some(account)
}

/// The entity a coverage listing's `entityArn` names, for matching against a row's
/// `entityArn`: the value with the `EntityVersion` suffix a version-qualified ARN carries
/// dropped, since a coverage row is about the entity rather than one version of it.
pub fn coverage_entity_arn(entity_arn: &str) -> &str {
    version_qualified(entity_arn).map_or(entity_arn, |(unversioned, _)| unversioned)
}

/// The qualifier naming an entity's unpublished version, which the model forbids in a
/// coverage listing's `entityArn`: "The ARN must not include a :DRAFT qualifier."
const DRAFT_QUALIFIER: &str = ":DRAFT";

/// Refuse an identifier carrying the `:DRAFT` qualifier the model forbids, returning the
/// `message` the refusal states — `None` when it carries none.
///
/// The sentence is fixed text for this error class and names the offending input member;
/// nothing else about the value is echoed. One home for it, so every member documented
/// that way refuses alike.
pub fn draft_qualifier_violation(member: &str, identifier: &str) -> Option<String> {
    identifier
        .ends_with(DRAFT_QUALIFIER)
        .then(|| format!("{member} must not include a {DRAFT_QUALIFIER} qualifier."))
}

/// One page of coverage rows, already filtered by whatever the operation asks about, in the
/// order [`State::resource_coverages`] walks them: the summaries, and the `nextToken` to
/// resume after them when a further row exists.
///
/// One home for the paging, so `ListEntityCoverages` and `ListProtectedResourceCoverages`
/// page alike.
pub fn coverage_page<'a>(
    rows: impl IntoIterator<Item = (&'a String, &'a ResourceCoverage)>,
    start_after: Option<&str>,
    page_size: usize,
) -> (Vec<ResourceCoverageSummary>, Option<String>) {
    let mut remaining = rows
        .into_iter()
        .filter(|(key, _)| start_after.is_none_or(|after| key.as_str() > after));
    let page: Vec<(&String, &ResourceCoverage)> = remaining.by_ref().take(page_size).collect();
    let last_key = page.last().map(|(key, _)| (*key).clone());
    let summaries = page
        .into_iter()
        .map(|(_, row)| row.summary())
        .collect::<Vec<_>>();
    // A `nextToken` is emitted only when a further row really exists, so a caller that
    // follows it never gets an empty final page.
    let next_token = remaining
        .next()
        .and_then(|_| last_key.map(|key| encode_next_token(&key)));
    (summaries, next_token)
}
