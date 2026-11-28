// SPDX-License-Identifier: CC0-1.0

//! What a submitted `FirewallPolicyV2` document has to satisfy, and what its
//! sections consume.
//!
//! Every check here reads ONLY the submitted document — no store, no lock — so a
//! handler runs them before it reaches this service's state, and each walks the
//! document's two sections and their entries once, with no recursion: a Next
//! Generation policy nests a section, an entry and an embedded rule group, and
//! nothing below that carries entries of its own.

use std::collections::HashSet;

use crate::ops::types::{FirewallPolicyV2, FirewallRuleGroup, IpsRuleGroup};
use crate::state::{
    context_message, parameter_context_message, suricata_rule_lines, MAX_POLICY_LIST_ENTRIES,
};

/// The capacity the rules of one embedded firewall engine rule group consume:
/// one unit per structured rule and one per rule line of a Suricata rules
/// string, which is the count-of-individual-rules rule
/// `CreateRuleGroupRequest$Capacity` states for every rule group whose rules are
/// stateful.
///
/// An embedded firewall rule group carries no domain list (see
/// [`inline_rule_group_breach`]), so there is no domain specification to count.
pub fn firewall_rule_group_capacity(group: &FirewallRuleGroup) -> i32 {
    let from_rules = group.rules_source.firewall_rules.as_ref().map_or(0, Vec::len);
    let from_string = group
        .rules_source
        .rules_string
        .as_deref()
        .map_or(0, |rules| suricata_rule_lines(rules).count());
    i32::try_from(from_rules.saturating_add(from_string)).unwrap_or(i32::MAX)
}

/// The capacity the rules of one embedded IPS engine rule group consume: one
/// unit per rule line of its Suricata rules string, the only rules source an
/// `IpsRuleGroup` has.
pub fn ips_rule_group_capacity(group: &IpsRuleGroup) -> i32 {
    let from_string = group
        .rules_source
        .rules_string
        .as_deref()
        .map_or(0, |rules| suricata_rule_lines(rules).count());
    i32::try_from(from_string).unwrap_or(i32::MAX)
}

/// The first list in `policy` that exceeds [`MAX_POLICY_LIST_ENTRIES`], named
/// with its length, or `None` when the document is within bounds.
///
/// The same ceiling, and for the same reason, as the one a first-generation
/// policy document is held to ([`crate::state::policy_document_breach`]): the model
/// puts no length constraint on a section's entries or on an embedded rule
/// group's structured rules, so this is what bounds what one request can make
/// this emulator retain.
pub fn policy_document_breach(policy: &FirewallPolicyV2) -> Option<String> {
    let firewall_entries = policy
        .firewall_rule_group_entries
        .as_ref()
        .map_or(0, Vec::len);
    let ips_entries = policy.ips_rule_group_entries.as_ref().map_or(0, Vec::len);
    let embedded_rules = policy
        .firewall_rule_group_entries
        .iter()
        .flatten()
        .filter_map(|entry| entry.inline_rule_group.as_ref())
        .map(|group| group.rules_source.firewall_rules.as_ref().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    [
        ("FirewallRuleGroupEntries", firewall_entries),
        ("IpsRuleGroupEntries", ips_entries),
        (
            "FirewallRuleGroupEntries.InlineRuleGroup.RulesSource.FirewallRules",
            embedded_rules,
        ),
    ]
    .into_iter()
    .find(|(_, length)| *length > MAX_POLICY_LIST_ENTRIES)
    .map(|(member, length)| {
        format!(
            "FirewallPolicyV2.{member} has {length} entries; at most \
             {MAX_POLICY_LIST_ENTRIES} are allowed."
        )
    })
}

/// Why the SECTION ENTRIES of `policy` cannot be served, as a message naming the
/// first offending entry, or `None` when every entry is well formed.
///
/// Three documented rules, each of them a defect of the request
/// (`InvalidRequestException` covers "an unsupported parameter name or value"):
///
/// * an entry names its rule group exactly one way — "provide either
///   `ResourceArn` to reference an existing rule group or `InlineRuleGroup` to
///   embed a rule group in the policy", so naming both, or neither, names no one
///   rule group;
/// * "an embedded rule group can't use a domain list rules source; to filter on
///   a domain list, reference a standalone rule group by ARN instead"; and
/// * "each entry in the firewall section must have a unique priority"
///   (`FirewallRuleGroupEntry$Priority`), which is what orders that section's
///   evaluation — two entries that share one priority have no order between
///   them, and so do two that state none.
///
/// It reads only the submitted document, so a caller runs it before the store is
/// reached at all.
pub fn section_entry_breach(policy: &FirewallPolicyV2) -> Option<String> {
    let mut priorities: HashSet<Option<i32>> = HashSet::new();
    for entry in policy.firewall_rule_group_entries.iter().flatten() {
        if let Some(breach) = one_rule_group_breach(
            FIREWALL_SECTION,
            entry.resource_arn.as_deref(),
            entry.inline_rule_group.is_some(),
        ) {
            return Some(breach);
        }
        if let Some(group) = entry.inline_rule_group.as_ref() {
            if let Some(breach) = inline_rule_group_breach(group) {
                return Some(breach);
            }
        }
        if !priorities.insert(entry.priority) {
            return Some(context_message(
                "Each entry in the firewall section must have a unique priority",
                FIREWALL_SECTION,
            ));
        }
    }
    for entry in policy.ips_rule_group_entries.iter().flatten() {
        if let Some(breach) = one_rule_group_breach(
            IPS_SECTION,
            entry.resource_arn.as_deref(),
            entry.inline_rule_group.is_some(),
        ) {
            return Some(breach);
        }
    }
    None
}

/// The input path the firewall section's entries arrive on, which is the context
/// a refusal about one of them reports.
const FIREWALL_SECTION: &str = "FirewallPolicyV2.FirewallRuleGroupEntries";

/// The input path the IPS section's entries arrive on. See [`FIREWALL_SECTION`].
const IPS_SECTION: &str = "FirewallPolicyV2.IpsRuleGroupEntries";

/// Why one entry does not name exactly one rule group, or `None` when it does.
fn one_rule_group_breach(section: &str, arn: Option<&str>, inline: bool) -> Option<String> {
    match (arn, inline) {
        (Some(arn), true) => Some(parameter_context_message(
            "Provide either ResourceArn or InlineRuleGroup, not both",
            arn,
            section,
        )),
        (None, false) => Some(context_message(
            "Provide either ResourceArn or InlineRuleGroup",
            section,
        )),
        _ => None,
    }
}

/// Why an embedded firewall engine rule group cannot be served, or `None` when
/// it can: "an embedded rule group can't use a domain list rules source"
/// (`FirewallRuleGroupEntry$InlineRuleGroup`).
fn inline_rule_group_breach(group: &FirewallRuleGroup) -> Option<String> {
    group.rules_source.rules_source_list.as_ref().map(|_| {
        context_message(
            "An embedded rule group can't use a domain list rules source",
            "FirewallPolicyV2.FirewallRuleGroupEntries.InlineRuleGroup.RulesSource.RulesSourceList",
        )
    })
}

/// The `RuleOutputFormat` member that asks for structured `FirewallRule`
/// objects (`smithy_query(query="shape",shape="RuleOutputFormat")`).
pub const RULE_OUTPUT_FIREWALL_RULES: &str = "FIREWALL_RULES";

/// The `RuleOutputFormat` member that asks for a Suricata-compatible
/// firewall-mode rule string. See [`RULE_OUTPUT_FIREWALL_RULES`].
pub const RULE_OUTPUT_RULES_STRING: &str = "RULES_STRING";

/// Whether every firewall engine rule group EMBEDDED in `policy` already holds
/// its rules in `format`.
///
/// `DescribeFirewallPolicyV2$RuleOutputFormat` is "the format in which to return
/// the rules of any firewall engine rule groups that are embedded in the policy
/// ... If you don't specify a format, Network Firewall returns each embedded rule
/// group in the format you provided it in. Referenced rule groups aren't
/// affected." A request that asks for the form the rules are already stored in
/// therefore asks for the stored document, and only one that asks for the OTHER
/// form asks for a conversion.
pub fn embedded_rules_already_in(policy: &FirewallPolicyV2, format: &str) -> bool {
    policy
        .firewall_rule_group_entries
        .iter()
        .flatten()
        .filter_map(|entry| entry.inline_rule_group.as_ref())
        .all(|group| match format {
            RULE_OUTPUT_FIREWALL_RULES => group.rules_source.rules_string.is_none(),
            RULE_OUTPUT_RULES_STRING => group.rules_source.firewall_rules.is_none(),
            // A format the model does not declare never reaches here: the
            // boundary refuses a `RuleOutputFormat` outside the enumerated set
            // before any handler runs.
            _ => true,
        })
}
