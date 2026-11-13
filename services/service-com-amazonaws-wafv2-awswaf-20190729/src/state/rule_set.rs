// SPDX-License-Identifier: CC0-1.0

//! What a saved RULE SET has to satisfy beyond its members' modelled constraints:
//! the preconditions WAF resolves across the whole rule list when a web ACL or a
//! rule group is written.
//!
//! Each of them is a judgement no single member can make on its own — two rules
//! agreeing on a name or a priority, a reference from one part of the document to
//! another, the price of the list as a whole — so each lives here, over the rule
//! list, and is run by every operation that SAVES one. They are all refusals of the
//! modelled `WAFInvalidParameterException` (client, HTTP 400), whose `Field`,
//! `Parameter` and `Reason` name the settings at fault, the value the request sent
//! and what WAF makes of it ([`invalid_parameter_exception`]).

use super::*;
use crate::ops::types::{CustomResponseBody, DefaultAction, RuleActionOverride};

/// The `ParameterExceptionField` value naming one rule of a rule list
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists `RULE`).
/// See [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE: &str = "RULE";

/// The `ParameterExceptionField` value naming a `CustomResponse`
/// (`CUSTOM_RESPONSE`, as the enum spells it). See [`FIELD_RESOURCE_ARN`].
pub const FIELD_CUSTOM_RESPONSE: &str = "CUSTOM_RESPONSE";

/// The `ErrorReason` WAF carries when one value appears twice where it may appear
/// once, as the recorded WAF responses word it.
///
/// One sentence for the whole class — the `Parameter` member carries the value that
/// was repeated, so the reason does not have to name which member repeated it.
pub const DUPLICATE_PARAMETER_REASON: &str =
    "You have duplicated some of the information in the parameter.";

/// The `ErrorReason` WAF carries when two rules of one list claim the same
/// `Priority`, as the recorded WAF responses word it.
///
/// Its own sentence rather than [`DUPLICATE_PARAMETER_REASON`], because the member
/// says what uniqueness is FOR: `Rule$Priority` is the evaluation order itself
/// ("WAF evaluates each request against the Rules in order based on the value of
/// Priority… The priorities don't need to be consecutive, but they must all be
/// different").
pub const DUPLICATE_PRIORITY_REASON: &str =
    "You have a duplicate priority. Priorities must be unique.";

/// The `ErrorReason` WAF carries when a parameter REFERENCES something that does
/// not resolve, as the recorded WAF responses word it.
///
/// One sentence for the whole class — a custom-response body key naming no entry in
/// the document's own map, an override naming no rule of the referenced rule group —
/// because the `Parameter` member carries the reference that resolved to nothing and
/// `Field` the settings it was written in.
pub const UNRESOLVED_REFERENCE_REASON: &str =
    "AWS WAF couldn't find a resource that a parameter references.";

/// The most web ACL capacity units one web ACL's rules may cost.
///
/// `WebACL$Capacity` is "the web ACL capacity units (WCUs) currently being used by
/// this web ACL", and WAF publishes a per-web-ACL ceiling on it in the quota list
/// the `WAFLimitsExceededException` shape points at ("WAF quotas in the WAF
/// Developer Guide"): the default maximum WCUs for a web ACL. A save whose rules
/// price above it is refused rather than stored, with the price the rules cost as
/// the `Parameter` a caller corrects by — the SAME figure `CheckCapacity` publishes
/// for those rules ([`web_acl_capacity`]), so a caller that sized the rule set
/// against that report can never be refused for a price it could not have seen.
pub const MAX_WEB_ACL_CAPACITY: i64 = 5_000;

/// [`invalid_parameter_exception`] for a web ACL whose rules cost more WCUs than
/// one web ACL may use: `Field` [`FIELD_WEB_ACL`] (the settings the ceiling applies
/// to), `Parameter` the price the rules cost and `Reason`
/// [`CAPACITY_LIMIT_EXCEEDED_REASON`] — the same sentence the rule-group form of
/// this refusal carries ([`capacity_limit_exceeded_exception`]), which is about the
/// capacity a rule group DECLARES.
pub fn web_acl_capacity_limit_exceeded_exception(
    required_capacity: i64,
) -> WAFInvalidParameterException {
    invalid_parameter_exception(
        FIELD_WEB_ACL,
        &required_capacity.to_string(),
        CAPACITY_LIMIT_EXCEEDED_REASON,
    )
}

/// The `Name` two rules of `rules` share, if two of them do.
///
/// A rule's name is its identity inside the list that holds it: it is what an
/// override, a metric and a sample name the rule by
/// (`SampledHTTPRequest$RuleNameWithinRuleGroup` is "the name of the `Rule` that the
/// request matched"), so a list carrying one name twice has no single rule for any
/// of those to resolve to and is refused rather than stored.
///
/// The names are compared exactly, like every other `EntityName` this service
/// resolves ("any name that doesn't exactly match the case-sensitive name of an
/// existing rule", `RuleActionOverride`'s own doc).
pub fn duplicate_rule_name(rules: &[Rule]) -> Option<&str> {
    let mut seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    rules
        .iter()
        .find(|rule| !seen.insert(rule.name.as_str()))
        .map(|rule| rule.name.as_str())
}

/// The `Priority` two rules of `rules` share, if two of them do.
///
/// `Rule$Priority` is the evaluation order of the list: "WAF evaluates each request
/// against the Rules in order based on the value of Priority. WAF processes rules
/// with lower priority first. The priorities don't need to be consecutive, but they
/// must all be different" — so two rules at one priority leave the order of those
/// two undefined, which is why the save is refused instead of stored (see
/// [`normalize_rule_order`], which holds a stored list in that order).
pub fn duplicate_rule_priority(rules: &[Rule]) -> Option<i32> {
    let mut seen: ::std::collections::BTreeSet<i32> = ::std::collections::BTreeSet::new();
    rules
        .iter()
        .find(|rule| !seen.insert(rule.priority))
        .map(|rule| rule.priority)
}

/// Every `CustomResponseBodyKey` the actions of this save reference, in the order
/// they are written.
///
/// A key can be written wherever the document configures a BLOCK: a rule's own
/// action, the web ACL's `DefaultAction`, and the action an override puts in the
/// place of a rule-group rule's own (`CustomResponse` is "a custom response to send
/// to the client. You can define a custom response for rule actions and default web
/// ACL actions that are set to `BlockAction`"). Each of them is collected here, so
/// one resolution covers every position.
fn referenced_custom_response_body_keys<'a>(
    rules: &'a [Rule],
    default_action: Option<&'a DefaultAction>,
) -> Vec<&'a str> {
    let mut keys: Vec<&str> = Vec::new();
    let mut note = |action: Option<&'a BlockAction>| {
        if let Some(key) = action
            .and_then(|block| block.custom_response.as_ref())
            .and_then(|response| response.custom_response_body_key.as_deref())
        {
            keys.push(key);
        }
    };
    note(default_action.and_then(|action| action.block.as_ref()));
    for rule in rules {
        note(rule.action.as_ref().and_then(|action| action.block.as_ref()));
        for carried in rule_action_overrides(&rule.statement) {
            note(carried.action_to_use.block.as_ref());
        }
    }
    keys
}

/// The `CustomResponseBodyKey` this save references that its own
/// `CustomResponseBodies` map does not define, if it references one.
///
/// `CustomResponse$CustomResponseBodyKey` "references the response body that you
/// want WAF to return to the web request client. … you first define the response
/// body key and value in the `CustomResponseBodies` setting for the `WebACL` or
/// `RuleGroup` where you want to use it. Then, in the rule action or web ACL default
/// action `BlockAction` setting, you reference the response body using this key" —
/// so the reference resolves inside the document being saved, against the map the
/// SAME request carries, and a save is a whole-document write (an update replaces
/// the map as well as the rules). A key that resolves to nothing has no body to
/// serve for a blocked request, so the save is refused rather than stored.
pub fn unresolved_custom_response_body_key<'a>(
    rules: &'a [Rule],
    default_action: Option<&'a DefaultAction>,
    custom_response_bodies: Option<&HashMap<String, CustomResponseBody>>,
) -> Option<&'a str> {
    referenced_custom_response_body_keys(rules, default_action)
        .into_iter()
        .find(|key| {
            custom_response_bodies.is_none_or(|bodies| !bodies.contains_key(*key))
        })
}

/// The `RuleActionOverrides` the statements of one rule carry, whichever kind of
/// rule-group statement carries them and however deeply it is nested.
///
/// Both rule-group-running statements carry the member, and each is paired with the
/// statement that holds it, because the two are resolved DIFFERENTLY: see
/// [`ComAmazonawsWafv2Awswaf20190729State::unknown_rule_group_rule_override`].
fn rule_action_overrides(statement: &Statement) -> Vec<&RuleActionOverride> {
    let mut overrides: Vec<&RuleActionOverride> = Vec::new();
    let mut pending: Vec<&Statement> = vec![statement];
    while let Some(statement) = pending.pop() {
        for carried in [
            statement
                .rule_group_reference_statement
                .as_ref()
                .and_then(|reference| reference.rule_action_overrides.as_deref()),
            statement
                .managed_rule_group_statement
                .as_ref()
                .and_then(|managed| managed.rule_action_overrides.as_deref()),
        ] {
            overrides.extend(carried.unwrap_or_default());
        }
        pending.extend(nested_statements(statement));
    }
    overrides
}

impl ComAmazonawsWafv2Awswaf20190729State {
    /// The name of a `RuleActionOverride` on a CUSTOMER-OWNED rule-group reference
    /// in `rules` that the referenced group does not declare a rule for, if one
    /// carries such a name.
    ///
    /// An override replaces the action of one rule INSIDE the referenced group, by
    /// name, and the model states what each kind of group makes of a name that is
    /// not one of its own: "with managed rule groups, WAF silently ignores any
    /// override that uses an invalid rule name. With customer-owned rule groups,
    /// invalid rule names in your overrides will cause web ACL updates to fail. An
    /// invalid rule name is any name that doesn't exactly match the case-sensitive
    /// name of an existing rule in the rule group" (`RuleActionOverride`'s own doc).
    /// So the resolution runs for a `RuleGroupReferenceStatement` alone: a group this
    /// account owns is held by this service, which knows its rules, while a managed
    /// group's rule list is its vendor's and an override naming nothing in it is
    /// accepted and ignored.
    ///
    /// A reference whose ARN names no rule group this service holds resolves to
    /// nothing at all, which is the refusal
    /// [`Self::unresolvable_rule_reference`] makes about the REFERENCE; this one
    /// judges only the overrides of a reference that does resolve.
    pub fn unknown_rule_group_rule_override<'a>(&self, rules: &'a [Rule]) -> Option<&'a str> {
        let mut pending: Vec<&'a Statement> = rules.iter().map(|rule| &rule.statement).collect();
        while let Some(statement) = pending.pop() {
            if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
                if let Some(entry) = self.rule_group_by_arn(&reference.arn) {
                    let declared: ::std::collections::BTreeSet<&str> = entry
                        .rule_group
                        .rules
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|rule| rule.name.as_str())
                        .collect();
                    if let Some(unknown) = reference
                        .rule_action_overrides
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .find(|carried| !declared.contains(carried.name.as_str()))
                    {
                        return Some(unknown.name.as_str());
                    }
                }
            }
            pending.extend(nested_statements(statement));
        }
        None
    }

    /// The refusal a rule set earns from the preconditions WAF resolves ACROSS the
    /// whole list, or `None` when it earns none.
    ///
    /// Run by every operation that SAVES a rule list, before anything is written, so
    /// a refused save stores nothing and rotates no lock token: two rules sharing a
    /// name, two sharing a priority, a custom-response reference the document's own
    /// map does not define, and an override naming no rule of the customer-owned
    /// group it overrides.
    ///
    /// `default_action` is the document's own default action where it has one (a web
    /// ACL does, a rule group does not), and `custom_response_bodies` is the map the
    /// same request carries, since a save replaces it whole.
    pub fn rule_set_refusal(
        &self,
        rules: &[Rule],
        default_action: Option<&DefaultAction>,
        custom_response_bodies: Option<&HashMap<String, CustomResponseBody>>,
    ) -> Option<WAFInvalidParameterException> {
        if let Some(name) = duplicate_rule_name(rules) {
            return Some(invalid_parameter_exception(
                FIELD_RULE,
                name,
                DUPLICATE_PARAMETER_REASON,
            ));
        }
        if let Some(priority) = duplicate_rule_priority(rules) {
            return Some(invalid_parameter_exception(
                FIELD_RULE,
                &priority.to_string(),
                DUPLICATE_PRIORITY_REASON,
            ));
        }
        if let Some(key) =
            unresolved_custom_response_body_key(rules, default_action, custom_response_bodies)
        {
            return Some(invalid_parameter_exception(
                FIELD_CUSTOM_RESPONSE,
                key,
                UNRESOLVED_REFERENCE_REASON,
            ));
        }
        if let Some(name) = self.unknown_rule_group_rule_override(rules) {
            return Some(invalid_parameter_exception(
                FIELD_RULE_GROUP_REFERENCE_STATEMENT,
                name,
                UNRESOLVED_REFERENCE_REASON,
            ));
        }
        None
    }

    /// [`Self::rule_set_refusal`] for a WEB ACL's rule set: the preconditions every
    /// saved rule list satisfies, plus the per-web-ACL WCU ceiling
    /// ([`MAX_WEB_ACL_CAPACITY`]), which is a property of a web ACL rather than of a
    /// rule list — a rule group is priced against the `Capacity` its own request
    /// declares instead ([`capacity_limit_exceeded_exception`]).
    ///
    /// The price is read with the same accounting `CheckCapacity` publishes, over the
    /// rule-group capacities of the scope being saved into.
    pub fn web_acl_rule_set_refusal(
        &self,
        scope: &str,
        rules: &[Rule],
        default_action: Option<&DefaultAction>,
        custom_response_bodies: Option<&HashMap<String, CustomResponseBody>>,
    ) -> Option<WAFInvalidParameterException> {
        if let Some(refusal) = self.rule_set_refusal(rules, default_action, custom_response_bodies)
        {
            return Some(refusal);
        }
        let required_capacity = web_acl_capacity(rules, &self.rule_group_capacities(scope));
        (required_capacity > MAX_WEB_ACL_CAPACITY)
            .then(|| web_acl_capacity_limit_exceeded_exception(required_capacity))
    }
}
