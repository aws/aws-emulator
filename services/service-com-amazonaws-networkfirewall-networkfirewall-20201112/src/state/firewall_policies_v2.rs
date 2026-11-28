// SPDX-License-Identifier: CC0-1.0

//! Next Generation firewall policies (the `…V2` operations), as persisted, and
//! every projection from one onto a modelled shape.
//!
//! A Next Generation policy is a firewall policy: it carries the same
//! `ResourceName` and `ResourceArn` shapes the first-generation one does, and a
//! firewall names the policy it uses through the single
//! `Firewall$FirewallPolicyArn` member whatever generation that policy belongs to
//! ("the relationship of firewall to firewall policy is many to one"). So both
//! generations share ONE `firewall-policy/<name>` ARN space and ONE name space
//! here ([`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::firewall_policy_name_taken`]):
//! an ARN names at most one policy, which it could not if one name could be taken
//! in both.
//!
//! What the two generations do NOT share is the document: a
//! `FirewallPolicyV2` is a firewall section and an IPS section of engine rule
//! groups, where a `FirewallPolicy` is stateless and stateful rule-group
//! references and default actions. Each generation therefore keeps its own
//! records, and each `…V2` operation reads and writes only the Next Generation
//! store.
//!
//! The checks a submitted document has to pass, and the capacity its sections
//! consume, live in [`documents`]; this file holds the record, its projections
//! and the store operations over it.

use std::time::SystemTime;

use super::{
    as_date_time, missing_arn_message, ComAmazonawsNetworkfirewallNetworkfirewall20201112State,
    ARN_TYPE_FIREWALL_POLICY, REFERENCED_RULE_GROUP, RULE_GROUP_ENGINE_TYPES,
};
use crate::ops::types::{
    EncryptionConfiguration, FirewallPolicyV2, FirewallPolicyV2Metadata, FirewallPolicyV2Response,
    Tag,
};

mod documents;
pub use documents::*;

/// The listing kind
/// [`ListFirewallPoliciesV2`](crate::ops::list_firewall_policies_v2) stamps on
/// its cursors. See [`super::PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_FIREWALL_POLICIES_V2: &str = "firewall-policies-v2";

/// The listing kind [`ListContextStores`](crate::ops::list_context_stores)
/// stamps on its cursors. See [`super::PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_CONTEXT_STORES: &str = "context-stores";

/// A Next Generation policy, as persisted. Every member
/// `FirewallPolicyV2Response` reports is read back from here.
#[derive(Debug, Clone)]
pub struct FirewallPolicyV2Record {
    /// `FirewallPolicyV2Response.FirewallPolicyV2Name`.
    pub policy_name: String,
    /// `FirewallPolicyV2Response.FirewallPolicyV2Arn`.
    pub policy_arn: String,
    /// `FirewallPolicyV2Response.FirewallPolicyV2Id`.
    pub policy_id: String,
    /// The policy document itself, as `DescribeFirewallPolicyV2` returns it.
    pub policy: FirewallPolicyV2,
    /// `FirewallPolicyV2Response.Description`.
    pub description: Option<String>,
    /// `FirewallPolicyV2Response.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `FirewallPolicyV2Response.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// A `ResourceStatus` member.
    pub status: String,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// `FirewallPolicyV2Response.LastModifiedTime`.
    pub last_modified_time: SystemTime,
}

impl FirewallPolicyV2Record {
    /// The `FirewallPolicyV2Response` shape for this record.
    ///
    /// `consumed` is the capacity this policy's sections consume and
    /// `associations` how many firewalls use it; both are read from other stores,
    /// so they are passed in rather than held on the record — the same way a
    /// first-generation policy's consumed capacity is.
    pub fn to_response(
        &self,
        consumed: ConsumedPolicyV2Capacity,
        associations: i32,
    ) -> FirewallPolicyV2Response {
        FirewallPolicyV2Response {
            firewall_policy_v2_name: self.policy_name.clone(),
            firewall_policy_v2_arn: self.policy_arn.clone(),
            firewall_policy_v2_id: self.policy_id.clone(),
            description: self.description.clone(),
            firewall_policy_v2_status: Some(self.status.clone()),
            tags: self.tags.clone(),
            consumed_firewall_rule_capacity: Some(consumed.firewall),
            consumed_ips_rule_capacity: Some(consumed.ips),
            consumed_ips_domain_capacity: Some(consumed.ips_domain),
            number_of_associations: Some(associations),
            encryption_configuration: self.encryption_configuration.clone(),
            last_modified_time: Some(as_date_time(self.last_modified_time)),
            // `StatusMessage` is "additional information about the current
            // status" and `Errors` are configuration errors "Network Firewall
            // detects ... asynchronously, after you create or update the
            // policy": a policy this service accepted is ACTIVE with nothing
            // further to report, so neither member carries invented text.
            ..FirewallPolicyV2Response::default()
        }
    }

    /// The `FirewallPolicyV2Metadata` entry `ListFirewallPoliciesV2` reports.
    pub fn to_metadata(&self) -> FirewallPolicyV2Metadata {
        FirewallPolicyV2Metadata {
            name: Some(self.policy_name.clone()),
            arn: Some(self.policy_arn.clone()),
        }
    }
}

/// The capacity a Next Generation policy's sections consume, as the
/// `FirewallPolicyV2Response` `Consumed*Capacity` members report it.
///
/// Each member is "the number of ... capacity units that the policy's ...
/// section currently consumes", so each is totalled from the rule groups that
/// section carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConsumedPolicyV2Capacity {
    /// `ConsumedFirewallRuleCapacity`.
    pub firewall: i32,
    /// `ConsumedIpsRuleCapacity`.
    pub ips: i32,
    /// `ConsumedIpsDomainCapacity` — the IPS section's domain list units, of
    /// which an embedded IPS rule group has none: an `IpsRulesSource` carries a
    /// Suricata rules string and nothing else, so no domain list can be
    /// expressed there.
    pub ips_domain: i32,
}

impl ComAmazonawsNetworkfirewallNetworkfirewall20201112State {
    /// Whether a firewall policy of EITHER generation already holds the name
    /// `name`.
    ///
    /// One name space, because one ARN space: see this module's documentation.
    pub fn firewall_policy_name_taken(&self, name: &str) -> bool {
        self.firewall_policies.contains_key(name) || self.firewall_policies_v2.contains_key(name)
    }

    /// The `FirewallPolicyV2Name` the given name and/or ARN identifies.
    pub fn resolve_firewall_policy_v2(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, super::ResolveError> {
        super::resolve(
            &self.firewall_policy_v2_arn_index,
            name,
            name,
            arn,
            &[ARN_TYPE_FIREWALL_POLICY],
            |key| self.firewall_policies_v2.contains_key(key),
        )
    }

    /// Why a `FirewallPolicyV2` document's rule-group REFERENCES cannot be
    /// served, as a message naming the first offending reference, or `None` when
    /// the document references nothing this service cannot resolve.
    ///
    /// A reference is an existing engine rule group's ARN
    /// (`FirewallRuleGroupEntry$ResourceArn` is "the Amazon Resource Name (ARN)
    /// of an existing firewall engine rule group"), so an ARN no stored engine
    /// rule group carries names no resource at all — which is what
    /// `InvalidRequestException` documents: "your request references an ARN that
    /// is malformed, or corresponds to a resource that isn't valid in the
    /// context of the request".
    ///
    /// This service writes no firewall-engine or IPS-engine rule group — the
    /// rule-group writes refuse those kinds rather than storing one they cannot
    /// serve — so every such reference names a rule group that does not exist,
    /// and a policy that embeds its rule groups references none. It reads the
    /// store, so a caller runs it in the same critical section the record is
    /// written in.
    pub fn policy_v2_reference_breach(&self, policy: &FirewallPolicyV2) -> Option<String> {
        policy_v2_references(policy)
            .find(|arn| !self.holds_engine_rule_group(arn))
            .map(|arn| missing_arn_message(REFERENCED_RULE_GROUP, arn))
    }

    /// Whether `arn` names a stored firewall-engine or IPS-engine rule group, of
    /// which this service holds none (see [`Self::policy_v2_reference_breach`]).
    fn holds_engine_rule_group(&self, arn: &str) -> bool {
        self.rule_group_arn_index
            .get(arn)
            .and_then(|key| self.rule_groups.get(key))
            .is_some_and(|record| RULE_GROUP_ENGINE_TYPES.contains(&record.rule_group_type.as_str()))
    }

    /// The capacity the sections of `policy` consume.
    ///
    /// An entry that EMBEDS its rule group consumes what that rule group's rules
    /// consume ([`firewall_rule_group_capacity`], [`ips_rule_group_capacity`]);
    /// an entry that REFERENCES one consumes the capacity that rule group
    /// reserves, which for a rule group this service does not hold is nothing —
    /// the same way a first-generation policy's consumed capacity treats a
    /// reference it cannot resolve.
    pub fn consumed_policy_v2_capacity(
        &self,
        policy: &FirewallPolicyV2,
    ) -> ConsumedPolicyV2Capacity {
        let mut consumed = ConsumedPolicyV2Capacity::default();
        for entry in policy.firewall_rule_group_entries.iter().flatten() {
            if let Some(group) = entry.inline_rule_group.as_ref() {
                consumed.firewall = consumed
                    .firewall
                    .saturating_add(firewall_rule_group_capacity(group));
            } else {
                consumed.firewall = consumed
                    .firewall
                    .saturating_add(self.reserved_capacity(entry.resource_arn.as_deref()));
            }
        }
        for entry in policy.ips_rule_group_entries.iter().flatten() {
            if let Some(group) = entry.inline_rule_group.as_ref() {
                consumed.ips = consumed.ips.saturating_add(ips_rule_group_capacity(group));
            } else {
                consumed.ips = consumed
                    .ips
                    .saturating_add(self.reserved_capacity(entry.resource_arn.as_deref()));
            }
        }
        consumed
    }

    /// The capacity the rule group `arn` names reserves — its declared
    /// `Capacity` ("when you reference a rule group from a firewall policy,
    /// Network Firewall reserves this capacity for the rule group") — and
    /// nothing for a reference to a rule group this service does not hold.
    fn reserved_capacity(&self, arn: Option<&str>) -> i32 {
        arn.and_then(|arn| self.rule_group_arn_index.get(arn))
            .and_then(|key| self.rule_groups.get(key))
            .map_or(0, |record| record.capacity)
    }

    /// How many stored firewalls use the firewall policy `policy_arn` names.
    ///
    /// `FirewallPolicyV2Response$NumberOfAssociations` is "the number of
    /// firewalls that are associated with the Next Generation policy", and a
    /// firewall names the policy it uses in `Firewall$FirewallPolicyArn` — so the
    /// count is READ from the firewall store rather than kept as a counter
    /// beside the policy, which cannot then drift from the firewalls it counts.
    ///
    /// READ ONLY: it reports what the firewall writes left behind and changes
    /// nothing a later caller sees.
    pub fn firewall_policy_associations(&self, policy_arn: &str) -> i32 {
        let using = self
            .firewalls
            .values()
            .filter(|firewall| firewall.firewall_policy_arn == policy_arn)
            .count();
        i32::try_from(using).unwrap_or(i32::MAX)
    }
}

/// Every engine rule-group ARN a Next Generation policy document references,
/// the firewall section's first.
fn policy_v2_references(policy: &FirewallPolicyV2) -> impl Iterator<Item = &str> {
    policy
        .firewall_rule_group_entries
        .iter()
        .flatten()
        .filter(|entry| entry.inline_rule_group.is_none())
        .filter_map(|entry| entry.resource_arn.as_deref())
        .chain(
            policy
                .ips_rule_group_entries
                .iter()
                .flatten()
                .filter(|entry| entry.inline_rule_group.is_none())
                .filter_map(|entry| entry.resource_arn.as_deref()),
        )
}
