// SPDX-License-Identifier: CC0-1.0

//! The DNS Firewall RULE slice of this service's state: the record a rule is held as, the
//! ids rules are minted under, and the refusals a rule's own definition settles.
//!
//! A rule lives under a DNS View — `CreateFirewallRule` declares `dnsViewId` `@required` and
//! `@references` the DNS View resource
//! (`smithy_query(query="operation",operation="CreateFirewallRule")`) — and it names what it
//! matches on either as a firewall domain list or as one of the advanced threat protections,
//! so the record holds the view it belongs to and at most one of those two targets.
//!
//! Nothing here reads the clock or the store on its own: the handlers pass both in, so a
//! rule's status settles exactly as every other resource of this service does.

use std::time::SystemTime;

use super::{field_validation, CrResourceStatus};
use crate::ops::types::{CreateFirewallRuleInput, ValidationException};

/// The leading characters of every firewall rule id this service mints.
pub const FIREWALL_RULE_ID_PREFIX: &str = "fr-";

/// How this service's own refusal messages spell the DNS Firewall rule resource kind — the
/// API's spelling, as [`super::DNS_VIEW_KIND`] is for a DNS View.
pub const FIREWALL_RULE_KIND: &str = "FirewallRule";

/// The token a quota refusal about the firewall rules of a DNS View names them by. A rule
/// carries no `arn` member in this service's model, so this is a message token only.
pub const FIREWALL_RULE_RESOURCE: &str = "firewall-rule";

/// The most firewall rules one DNS View holds at once.
///
/// A bound on how far the store can grow under repeated creates, for the reason the sibling
/// ceilings of this service state ([`super::MAX_DNS_VIEWS_PER_RESOLVER`]): a create past it
/// is refused with the modelled `ServiceQuotaExceededException` (HTTP 402). AWS publishes its
/// own quota and it is not this number; this is the emulator's own ceiling.
pub const MAX_FIREWALL_RULES_PER_DNS_VIEW: usize = 100;

/// The lowest priority a rule may carry, from `FirewallRulePriority`'s `@range` 1..10000
/// (`smithy_query(query="shape",shape="FirewallRulePriority")`).
pub const MIN_FIREWALL_RULE_PRIORITY: i64 = 1;

/// The highest priority a rule may carry, from the same `@range`.
pub const MAX_FIREWALL_RULE_PRIORITY: i64 = 10_000;

/// The `action` value that disallows a matching query, and so the one that needs a
/// `blockResponse` (`FirewallRuleAction`).
pub const ACTION_BLOCK: &str = "BLOCK";

/// The `blockResponse` value that answers a blocked query with a custom record, and so the
/// one that needs that record's three members (`FirewallBlockResponse`).
pub const BLOCK_RESPONSE_OVERRIDE: &str = "OVERRIDE";

/// The error code this service leads the message of a rule that names no target — or two —
/// with, as [`super::not_found`]'s code is for a resolved id: a bracketed `[GR-ERRnnnnn]`
/// code is how a client tells one refusal class of this service from another.
const RULE_TARGET_CODE: &str = "GR-ERR04003";

/// The error code the same service leads a BLOCK rule carrying no `blockResponse` with.
const BLOCK_RESPONSE_CODE: &str = "GR-ERR04001";

/// The `ValidationExceptionReason` a cross-field refusal of this service reports.
///
/// `FIELD_VALIDATION_FAILED` is the reason that carries a `fieldList`
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`), and a rule-level
/// requirement is not the failure of one member: it is about the combination, so there is no
/// one member to name and the reason is the catch-all the model declares beside it.
const CROSS_FIELD_REASON: &str = "OTHER";

/// A firewall rule id for sequence number `sequence`, matching `ResourceId`'s
/// `^[-.a-zA-Z0-9]+$` within its 1..64 length. Zero-padded, so id order is creation order and
/// a list page resumes deterministically.
pub fn firewall_rule_id(sequence: u64) -> String {
    format!("{FIREWALL_RULE_ID_PREFIX}{sequence:012x}")
}

/// The modelled 400 for a request that names neither of a rule's two targets, or both of
/// them: a rule matches the domains of a firewall domain list, or it matches one of the
/// advanced threat protections, and exactly one of the two makes it a rule.
pub fn rule_target_required() -> ValidationException {
    cross_field(format!(
        "[{RULE_TARGET_CODE}] Input should contain exactly one of FirewallDomainListId or \
         DnsAdvancedProtection."
    ))
}

/// The modelled 400 for a BLOCK rule that does not say what to answer a blocked query with.
pub fn block_response_required() -> ValidationException {
    cross_field(format!(
        "[{BLOCK_RESPONSE_CODE}] BlockResponse must be provided if Action is BLOCK."
    ))
}

/// A refusal of the COMBINATION the request describes: the whole sentence is the message, as
/// it is for the refusals of this service that carry a code.
fn cross_field(message: String) -> ValidationException {
    ValidationException {
        message,
        reason: CROSS_FIELD_REASON.to_string(),
        field_list: None,
    }
}

/// The modelled 400 for an `OVERRIDE` block response that does not carry the custom record it
/// needs, naming the member that is missing.
///
/// Each of the three members documents the requirement on itself ("This setting is required if
/// the BlockResponse setting is OVERRIDE" —
/// `smithy_query(query="doc",shape="CreateFirewallRuleInput",member="blockOverrideDomain")`),
/// so the refusal is the per-member one this service words with
/// [`super::field_validation`] rather than a rule-level sentence.
pub fn block_override_required(member: &str) -> ValidationException {
    field_validation(
        member,
        format!("must be provided when blockResponse is {BLOCK_RESPONSE_OVERRIDE}"),
    )
}

/// Every cross-field requirement a firewall rule's definition carries, in the order this
/// service reports them.
///
/// Called BEFORE any id the request carries is resolved, so a request that does not describe a
/// rule at all is refused for what it SAYS rather than for what it names: the target rule
/// first, then the BLOCK response, then the custom record an `OVERRIDE` response needs.
pub fn validate_rule_definition(input: &CreateFirewallRuleInput) -> Result<(), ValidationException> {
    // A rule matches on exactly one target. Naming neither leaves it matching nothing;
    // naming both leaves two rules in one.
    if input.firewall_domain_list_id.is_some() == input.dns_advanced_protection.is_some() {
        return Err(rule_target_required());
    }
    if input.action == ACTION_BLOCK && input.block_response.is_none() {
        return Err(block_response_required());
    }
    if input.block_response.as_deref() == Some(BLOCK_RESPONSE_OVERRIDE) {
        for (member, named) in [
            (
                "blockOverrideDomain",
                input.block_override_domain.is_some(),
            ),
            (
                "blockOverrideDnsType",
                input.block_override_dns_type.is_some(),
            ),
            ("blockOverrideTtl", input.block_override_ttl.is_some()),
        ] {
            if !named {
                return Err(block_override_required(member));
            }
        }
    }
    Ok(())
}

/// A DNS Firewall rule under a DNS View, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct FirewallRule {
    /// `id`.
    pub id: String,
    /// `dnsViewId` — the parent view.
    pub dns_view_id: String,
    /// `name`.
    pub name: String,
    /// `description`.
    pub description: Option<String>,
    /// `clientToken` — the caller's, or the one this service minted for it. The model
    /// declares no output member for it on this resource, so it is held for the idempotent
    /// replay alone.
    pub client_token: String,
    /// `action`.
    pub action: String,
    /// `blockResponse`.
    pub block_response: Option<String>,
    /// `blockOverrideDomain`.
    pub block_override_domain: Option<String>,
    /// `blockOverrideDnsType`.
    pub block_override_dns_type: Option<String>,
    /// `blockOverrideTtl`.
    pub block_override_ttl: Option<i32>,
    /// `confidenceThreshold`.
    pub confidence_threshold: Option<String>,
    /// `dnsAdvancedProtection` — one of the rule's two targets.
    pub dns_advanced_protection: Option<String>,
    /// `firewallDomainListId` — the other.
    pub firewall_domain_list_id: Option<String>,
    /// `priority` — the caller's, or the one this service assigned for it
    /// ([`super::ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState::next_firewall_rule_priority`]).
    pub priority: i64,
    /// `queryType`, which the create request carries as `qType`.
    pub query_type: Option<String>,
    /// `status`.
    pub status: CrResourceStatus,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `updatedAt`.
    pub updated_at: SystemTime,
    /// When the clock owes this rule its terminal status.
    pub operational_at: SystemTime,
}

impl FirewallRule {
    /// The firewall rule counterpart of `GlobalResolver::settle`: materialize the status
    /// change the clock has already made due.
    pub(super) fn settle(&mut self, now: SystemTime) {
        let pending = matches!(
            self.status,
            CrResourceStatus::Creating | CrResourceStatus::Updating
        );
        if pending && now >= self.operational_at {
            self.status = CrResourceStatus::Operational;
            self.updated_at = self.operational_at;
        }
    }

    /// Whether this record is the one `CreateFirewallRule` would mint for `input` — the
    /// comparison an idempotent replay makes before answering with it, so a `clientToken`
    /// reused with DIFFERENT parameters conflicts instead of quietly answering with the first
    /// call's rule.
    ///
    /// A request that names no `priority` asks for whichever one the service assigned, so it
    /// matches the stored rule whatever that is; one that names a priority must name the
    /// stored rule's own.
    pub fn matches_create(&self, input: &CreateFirewallRuleInput) -> bool {
        self.dns_view_id == input.dns_view_id
            && self.name == input.name
            && self.description == input.description
            && self.action == input.action
            && self.block_response == input.block_response
            && self.block_override_domain == input.block_override_domain
            && self.block_override_dns_type == input.block_override_dns_type
            && self.block_override_ttl == input.block_override_ttl
            && self.confidence_threshold == input.confidence_threshold
            && self.dns_advanced_protection == input.dns_advanced_protection
            && self.firewall_domain_list_id == input.firewall_domain_list_id
            && self.query_type == input.q_type
            && input
                .priority
                .is_none_or(|priority| priority == self.priority)
    }
}

impl super::ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState {
    /// The rules one DNS View holds, in id order — which is creation order, because every id
    /// is minted from the monotonic sequence.
    pub fn firewall_rules_of_view<'a>(
        &'a self,
        dns_view_id: &'a str,
    ) -> impl Iterator<Item = &'a FirewallRule> + 'a {
        self.firewall_rules
            .values()
            .filter(move |rule| rule.dns_view_id == dns_view_id)
    }

    /// The priority a rule whose request names none is given: one past the highest the DNS
    /// View already holds, starting at [`MIN_FIREWALL_RULE_PRIORITY`] for a view with no
    /// rules.
    ///
    /// `priority` is optional on the create and `@required` on its output
    /// (`smithy_query(query="operation",operation="CreateFirewallRule")`), so the service
    /// assigns one; rules are evaluated in priority order, so assigning the next one keeps a
    /// view's rules in the order they were created. `None` when the view already holds a rule
    /// at [`MAX_FIREWALL_RULE_PRIORITY`] and there is no next priority inside the modelled
    /// range.
    pub fn next_firewall_rule_priority(&self, dns_view_id: &str) -> Option<i64> {
        let highest = self
            .firewall_rules_of_view(dns_view_id)
            .map(|rule| rule.priority)
            .max();
        let next = highest.map_or(MIN_FIREWALL_RULE_PRIORITY, |priority| {
            priority.saturating_add(1)
        });
        (next <= MAX_FIREWALL_RULE_PRIORITY).then_some(next)
    }
}
