// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One rule of a DNS Firewall rule group, as stored.
///
/// The rule itself is held as the modelled shape every operation reports it as,
/// beside the two things that are not part of that shape: where the clock stands
/// on a rule whose provisioning is asynchronous, and nothing else — so a rule
/// cannot report one set of values to one operation and another set to the next.
#[derive(Debug, Clone, Default)]
pub struct FirewallRuleRecord {
    /// The rule as created, WITHOUT its lifecycle state: `Status` and
    /// `StatusMessage` are decided by [`Self::to_rule`] from the clock, so the
    /// two cannot disagree.
    pub rule: FirewallRule,
    /// The `FirewallRuleStatus` this rule reports until [`Self::settled_at`]
    /// falls due, or `None` for a rule whose provisioning needs no time at all —
    /// `FirewallRule$Status`'s doc: "For rules that do not require asynchronous
    /// provisioning, this field may be absent."
    pub status: Option<String>,
    /// The `FirewallRuleStatus` the clock owes this rule, reported from
    /// [`Self::settled_at`] onwards, with the message that states why.
    pub settled_status: Option<String>,
    pub settled_status_message: Option<String>,
    /// The instant at which the move to [`Self::settled_status`] is owed, or
    /// `None` for a rule with no transition in flight.
    pub settled_at: Option<Instant>,
}

impl FirewallRuleRecord {
    /// Project the record onto the modelled `FirewallRule`, with the lifecycle
    /// state this rule carries as of `now`: the transient one until the
    /// transition the clock owes falls due, the settled one after it — the same
    /// shape as [`FirewallRuleGroupRecord::status_at`].
    pub fn to_rule(&self, now: Instant) -> FirewallRule {
        let mut rule = self.rule.clone();
        let settled = self.settled_at.is_some_and(|due| now >= due);
        if settled {
            rule.status = self.settled_status.clone();
            rule.status_message = self.settled_status_message.clone();
        } else {
            rule.status = self.status.clone();
            rule.status_message = None;
        }
        rule
    }
}

/// One DNS Firewall rule to create, as the operations that create one state it.
///
/// `CreateFirewallRuleEntry` — one entry of a batch create — declares exactly
/// the members `CreateFirewallRuleRequest` does
/// (`smithy_query(query="shape",shape="CreateFirewallRuleEntry")`), so both
/// operations describe the same rule and both decide it through
/// [`ComAmazonawsRoute53resolverRoute53resolverState::create_firewall_rule`]
/// instead of each carrying its own copy of what creating a rule means.
#[derive(Debug, Clone, Copy)]
pub struct FirewallRuleCreate<'a> {
    pub creator_request_id: &'a str,
    pub firewall_rule_group_id: &'a str,
    pub firewall_domain_list_id: Option<&'a str>,
    pub priority: i32,
    pub action: &'a str,
    pub block_response: Option<&'a str>,
    pub block_override_domain: Option<&'a str>,
    pub block_override_dns_type: Option<&'a str>,
    pub block_override_ttl: Option<i32>,
    pub name: &'a str,
    pub firewall_domain_redirection_action: Option<&'a str>,
    pub qtype: Option<&'a str>,
    pub dns_threat_protection: Option<&'a str>,
    pub confidence_threshold: Option<&'a str>,
    pub firewall_rule_type: Option<&'a FirewallRuleType>,
}

/// Why one firewall-rule create was refused.
///
/// A create that carries ONE rule reports it as the modelled exception it holds,
/// and the whole call fails. A batch create decides each entry on its own, so it
/// reports the same refusal in the response body instead: the exception's shape
/// name as the error's `Code` ([`Self::code`]) and its sentence as the
/// `Message`, which is the code and the sentence the single create answers with
/// for that very entry.
#[derive(Debug, Clone)]
pub enum FirewallRuleCreateRefusal {
    Validation(ValidationException),
    ResourceNotFound(ResourceNotFoundException),
    LimitExceeded(LimitExceededException),
}

impl FirewallRuleCreateRefusal {
    /// The error code of this refusal: the modelled shape name of the exception
    /// that states it, which is the code the exception carries on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "ValidationException",
            Self::ResourceNotFound(_) => "ResourceNotFoundException",
            Self::LimitExceeded(_) => "LimitExceededException",
        }
    }

    /// The sentence this refusal states, as the exception's `Message`.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Validation(error) => error.message.clone(),
            Self::ResourceNotFound(error) => error.message.clone(),
            Self::LimitExceeded(error) => error.message.clone(),
        }
    }
}

/// The most rules one batch create, or one batch delete, may carry entries for.
///
/// Neither `CreateFirewallRuleEntries` nor `DeleteFirewallRuleEntries` declares
/// a `@length`
/// (`smithy_query(query="shape",shape="CreateFirewallRuleEntries,DeleteFirewallRuleEntries")`),
/// so without a bound one request could ask this emulator for unbounded work and
/// an unbounded response — the same reasoning as
/// [`MAX_RULES_PER_FIREWALL_RULE_GROUP`], and not a published AWS quota. A
/// request past it is refused WHOLE with the `LimitExceededException` both
/// operations model, so nothing is created or deleted for any of its entries.
pub const MAX_FIREWALL_RULE_ENTRIES_PER_BATCH: usize = 100;

/// How many entries one batch update may carry.
///
/// `UpdateFirewallRuleEntries` declares no `@length`
/// (`smithy_query(query="shape",shape="UpdateFirewallRuleEntries")`), so this
/// cap is the service's own, and the service's refusal is what names it: a
/// request has to carry FEWER entries than this number, which is how
/// [`firewall_rule_update_batch_too_large`] — the sentence the recorded
/// responses of this operation answer an oversized batch with — words it, so
/// the sentence and the number it reports cannot disagree. A request that does
/// not is refused WHOLE with the `LimitExceededException` the operation models,
/// so not one of its entries is applied. The batch CREATE carries its own,
/// larger bound ([`MAX_FIREWALL_RULE_ENTRIES_PER_BATCH`]), which is this
/// emulator's own and not a limit the service states.
pub const FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP: usize = 10;

/// The `LimitExceededException` a batch update of firewall rules is refused
/// with when it carries entries for more rules than one request may.
///
/// The refusal is the service's own sentence for that limit
/// ([`RSLVR_CODE_FIREWALL_RULE_BATCH_TOO_LARGE`]), whose one variable part is
/// the cap itself — it names neither the rules the batch asked for nor how many
/// of them there were. `ResourceType` is the rule, which is what the request
/// asked for too many of; the create's own limit on how many rules ONE GROUP
/// holds names the group instead, because there it is the group that is full.
/// The refusal is whole: the entries are not applied, not even the ones a
/// smaller batch would have applied.
pub fn firewall_rule_update_batch_too_large(cap: usize) -> LimitExceededException {
    LimitExceededException {
        message: Some(rslvr_message_sentences(
            RSLVR_CODE_FIREWALL_RULE_BATCH_TOO_LARGE,
            &[
                "The number of firewall rules in the update batch exceeds the allowed maximum",
                &format!(r#"Provide fewer than "{cap}" rules"#),
            ],
        )),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_RULE.to_string()),
    }
}

/// The `ResourceNotFoundException` the rule operations answer when no rule of
/// the group the request named matches the source it named the rule by, built
/// here so every operation that resolves a rule inside a group reports the same
/// body.
///
/// A rule is not addressed by an id of its own: it is addressed INSIDE a group,
/// by the source it matches on, so the service reports its own coded sentence
/// for it ([`RSLVR_CODE_FIREWALL_RULE_NOT_FOUND`]) and names both halves of
/// what the request named — the rule group, and the member it named the rule by
/// with the id it carried there. `ResourceType` is the rule's modelled shape
/// name, which is the resource that doesn't exist.
pub fn firewall_rule_not_found(
    firewall_rule_group_id: &str,
    match_source_member: &str,
    match_source_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_FIREWALL_RULE_NOT_FOUND,
            &format!(
                r#"Can’t locate the rule for the Firewall rule group ID: "{firewall_rule_group_id}" and {match_source_member} "{match_source_id}""#
            ),
        )),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_RULE.to_string()),
    }
}

/// The settings one DNS Firewall rule carries, as the operations that write one
/// state them.
///
/// A create states every setting of the rule it makes; an update states the
/// settings it changes, so what is checked for it is the stored rule with those
/// changes applied. Both are the same question — are these settings ones the
/// service admits — so both ask it of [`firewall_rule_settings_fault`].
#[derive(Debug, Clone, Copy)]
pub struct FirewallRuleSettings<'a> {
    pub name: &'a str,
    pub action: &'a str,
    pub block_response: Option<&'a str>,
    pub block_override_domain: Option<&'a str>,
    pub block_override_dns_type: Option<&'a str>,
    pub block_override_ttl: Option<i32>,
    pub firewall_domain_redirection_action: Option<&'a str>,
    pub firewall_domain_list_id: Option<&'a str>,
    pub dns_threat_protection: Option<&'a str>,
    pub confidence_threshold: Option<&'a str>,
    pub firewall_rule_type: Option<&'a FirewallRuleType>,
}

impl FirewallRuleSettings<'_> {
    /// Whether these settings describe a DNS Firewall Advanced rule: one whose
    /// match source is a threat detector or one of the Amazon Web
    /// Services-managed categories, rather than a domain list of the account.
    pub fn is_advanced(&self) -> bool {
        self.dns_threat_protection.is_some() || self.firewall_rule_type.is_some()
    }
}

/// The `ValidationException` these rule settings are refused with, or `None`
/// when the service admits them.
///
/// This is the whole of what the service requires of one rule's settings: the
/// modelled constraints the boundary validator cannot express, the match
/// sources that exclude one another, and the settings one `Action` and one
/// `BlockResponse` require. Deciding it HERE is what makes a rule the create
/// admits and the same rule an update arrives at the same rule, refused on the
/// same grounds.
pub fn firewall_rule_settings_fault(
    settings: &FirewallRuleSettings<'_>,
) -> Option<ValidationException> {
    // `Name`'s pattern carries a negative lookahead, which is outside what the
    // boundary validator's regex engine accepts, so the rule is enforced here.
    if !name_satisfies_pattern(settings.name) {
        return Some(ValidationException {
            message: Some(name_pattern_violation(settings.name)),
        });
    }

    // Each enum member admits exactly the values its modelled enum declares; a
    // value outside one is refused rather than stored as if the service
    // understood it.
    for (member, value, values) in [
        ("Action", Some(settings.action), crate::ops::types::ACTION_VALUES),
        (
            "BlockResponse",
            settings.block_response,
            crate::ops::types::BLOCK_RESPONSE_VALUES,
        ),
        (
            "BlockOverrideDnsType",
            settings.block_override_dns_type,
            crate::ops::types::BLOCK_OVERRIDE_DNS_TYPE_VALUES,
        ),
        (
            "FirewallDomainRedirectionAction",
            settings.firewall_domain_redirection_action,
            crate::ops::types::FIREWALL_DOMAIN_REDIRECTION_ACTION_VALUES,
        ),
        (
            "DnsThreatProtection",
            settings.dns_threat_protection,
            crate::ops::types::DNS_THREAT_PROTECTION_VALUES,
        ),
        (
            "ConfidenceThreshold",
            settings.confidence_threshold,
            crate::ops::types::CONFIDENCE_THRESHOLD_VALUES,
        ),
    ] {
        if let Some(value) = value {
            if !values.contains(&value) {
                return Some(ValidationException {
                    message: Some(format!("{member} must be one of {values:?}")),
                });
            }
        }
    }

    // "The rule can use any one of the following match sources, and the chosen
    // source must be supplied through the matching request field — they are
    // mutually exclusive": `FirewallDomainListId`, `DnsThreatProtection` and
    // `FirewallRuleType`.
    let sources = [
        settings.firewall_domain_list_id.is_some(),
        settings.dns_threat_protection.is_some(),
        settings.firewall_rule_type.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count();
    if sources != 1 {
        return Some(ValidationException {
            message: Some(
                "Exactly one of FirewallDomainListId, DnsThreatProtection and \
                 FirewallRuleType identifies the rule's match source"
                    .to_string(),
            ),
        });
    }

    // "`FirewallRuleType` is a tagged union — exactly one member must be set
    // per rule, and the member determines what the rule matches against."
    if let Some(rule_type) = settings.firewall_rule_type {
        let variants = [
            rule_type.partner_threat_protection.is_some(),
            rule_type.firewall_advanced_content_category.is_some(),
            rule_type.firewall_advanced_threat_category.is_some(),
            rule_type.dns_threat_protection.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count();
        if variants != 1 {
            return Some(ValidationException {
                message: Some(
                    "FirewallRuleType carries exactly one of \
                     PartnerThreatProtection, FirewallAdvancedContentCategory, \
                     FirewallAdvancedThreatCategory and DnsThreatProtection"
                        .to_string(),
                ),
            });
        }
        if let Some(detector) = &rule_type.dns_threat_protection {
            for (member, value, values) in [
                (
                    "FirewallRuleType.DnsThreatProtection.Value",
                    detector.value.as_str(),
                    crate::ops::types::DNS_THREAT_PROTECTION_VALUES,
                ),
                (
                    "FirewallRuleType.DnsThreatProtection.ConfidenceThreshold",
                    detector.confidence_threshold.as_str(),
                    crate::ops::types::CONFIDENCE_THRESHOLD_VALUES,
                ),
            ] {
                if !values.contains(&value) {
                    return Some(ValidationException {
                        message: Some(format!("{member} must be one of {values:?}")),
                    });
                }
            }
        }
    }

    // A rule whose match source is one of the DNS Firewall Advanced kinds:
    // `ALLOW` is "not available for DNS Firewall Advanced rules", and a
    // `DnsThreatProtection` rule needs the confidence threshold — "you must
    // provide this value when you create or update a DNS Firewall Advanced
    // rule".
    if settings.is_advanced() && settings.action == FIREWALL_RULE_ACTION_ALLOW {
        return Some(ValidationException {
            message: Some(format!(
                "Action {FIREWALL_RULE_ACTION_ALLOW} is not available for DNS Firewall \
                 Advanced rules"
            )),
        });
    }
    if settings.dns_threat_protection.is_some() && settings.confidence_threshold.is_none() {
        return Some(ValidationException {
            message: Some(
                "ConfidenceThreshold is required for a rule that matches a \
                 DnsThreatProtection detector"
                    .to_string(),
            ),
        });
    }

    // "`BlockResponse` … This setting is required if the rule action setting is
    // BLOCK", and each `BlockOverride*` member "is required if the
    // BlockResponse setting is OVERRIDE". A block option the rule requires and
    // the request left out is not a block option the service can act on, so it
    // is refused as one that is not valid
    // ([`firewall_rule_block_option_not_valid`]).
    if settings.action == FIREWALL_RULE_ACTION_BLOCK && settings.block_response.is_none() {
        return Some(firewall_rule_block_option_not_valid(
            BLOCK_OPTION_BLOCK_RESPONSE,
        ));
    }
    if settings.block_response == Some(FIREWALL_RULE_BLOCK_RESPONSE_OVERRIDE) {
        let missing = [
            (
                BLOCK_OPTION_BLOCK_OVERRIDE_DOMAIN,
                settings.block_override_domain.is_none(),
            ),
            (
                BLOCK_OPTION_BLOCK_OVERRIDE_DNS_TYPE,
                settings.block_override_dns_type.is_none(),
            ),
            (
                BLOCK_OPTION_BLOCK_OVERRIDE_TTL,
                settings.block_override_ttl.is_none(),
            ),
        ]
        .into_iter()
        .find_map(|(member, absent)| absent.then_some(member));
        if let Some(member) = missing {
            return Some(firewall_rule_block_option_not_valid(member));
        }
    }

    None
}

/// The block options a DNS Firewall rule carries, named as the model spells
/// them (`smithy_query(query="shape",shape="CreateFirewallRuleRequest")`).
///
/// They are the members [`FirewallRuleBlockOptions`] holds, carried as constants
/// because a refusal that names one states it in the service's own spelling
/// ([`block_option_name`]) rather than in the model's.
pub const BLOCK_OPTION_BLOCK_RESPONSE: &str = "BlockResponse";
pub const BLOCK_OPTION_BLOCK_OVERRIDE_DOMAIN: &str = "BlockOverrideDomain";
pub const BLOCK_OPTION_BLOCK_OVERRIDE_DNS_TYPE: &str = "BlockOverrideDnsType";
pub const BLOCK_OPTION_BLOCK_OVERRIDE_TTL: &str = "BlockOverrideTtl";

/// How the service NAMES one block option in a refusal: the modelled member
/// name read as words — every word upper case, separated by a single space — so
/// `BlockResponse` is stated as `BLOCK RESPONSE`.
///
/// The service states the option a caller sets rather than the member an API
/// request carries, and the two differ only in this spelling, so the name is
/// derived from the member instead of written out once per option.
pub fn block_option_name(member: &str) -> String {
    let mut name = String::with_capacity(member.len() + 3);
    let mut inside_word = false;
    for character in member.chars() {
        if inside_word && character.is_ascii_uppercase() {
            name.push(' ');
        }
        inside_word = !character.is_ascii_uppercase();
        name.push(character.to_ascii_uppercase());
    }
    name
}

/// The Route 53 Resolver service error code for a block option a rule cannot
/// block with.
///
/// Carried for the same reason as [`RSLVR_CODE_RESOURCE_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text.
pub const RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_VALID: &str = "RSLVR-02016";

/// The `ValidationException` a rule write is refused with when the block option
/// `member` is one the rule cannot block with — because the rule requires it and
/// the request did not state it.
///
/// The sentence is the service's own for that fault
/// ([`RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_VALID`]) and names the OPTION,
/// in the service's spelling of it ([`block_option_name`]), and nothing else
/// from the request.
pub fn firewall_rule_block_option_not_valid(member: &str) -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_VALID,
            &format!("Your block option {} is not valid", block_option_name(member)),
        )),
    }
}

/// The block settings one rule write STATES, as the request carries them.
///
/// These are the four members whose meaning is "how DNS Firewall blocks the
/// query" — `BlockResponse` and the three `BlockOverride*` settings it may
/// require — held apart from [`FirewallRuleSettings`] because what they are
/// checked against is what the REQUEST provided and not what the rule ends up
/// carrying: a rule stored with block settings keeps them when its action moves
/// off `BLOCK`, where they simply never apply.
#[derive(Debug, Clone, Copy)]
pub struct FirewallRuleBlockOptions<'a> {
    pub block_response: Option<&'a str>,
    pub block_override_domain: Option<&'a str>,
    pub block_override_dns_type: Option<&'a str>,
    pub block_override_ttl: Option<i32>,
}

impl FirewallRuleBlockOptions<'_> {
    /// Whether the request stated any of them at all.
    pub fn any_stated(&self) -> bool {
        self.block_response.is_some()
            || self.block_override_domain.is_some()
            || self.block_override_dns_type.is_some()
            || self.block_override_ttl.is_some()
    }
}

/// The `ValidationException` a rule write is refused with when it states a
/// block setting for an action that blocks nothing.
///
/// `Action`'s doc gives the two actions that let the query through — `ALLOW`
/// ("permit the request to go through") and `ALERT` ("permit the request to go
/// through but send an alert to the logs") — against `BLOCK`, the one that
/// "requires additional details in the rule's BlockResponse". The sentence is
/// the service's own for that fault
/// ([`RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_FOR_ACTION`]) and is ENTIRELY
/// fixed text: it names neither the option the request stated nor the action it
/// stated it with.
pub fn firewall_rule_block_option_not_for_action() -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_FOR_ACTION,
            &format!(
                "You can't provide a block option if the ACTION is \
                 {FIREWALL_RULE_ACTION_ALLOW} or {FIREWALL_RULE_ACTION_ALERT}"
            ),
        )),
    }
}

/// The `ValidationException` these stated block settings are refused with for a
/// rule whose action is `action`, or `None` when the service admits them.
///
/// A block setting states how a BLOCKING rule answers the query it matched, so
/// stating one for a rule that lets the query through is refused. `action` is
/// the action the rule takes once the write is applied — the one the request
/// states, or the stored one for an update that leaves it alone — so the same
/// question is answered the same way for a create, for an update that changes
/// the action and for an update that only adds a block setting to a rule that
/// does not block.
pub fn firewall_rule_block_option_fault(
    action: &str,
    stated: &FirewallRuleBlockOptions<'_>,
) -> Option<ValidationException> {
    let lets_the_query_through =
        action == FIREWALL_RULE_ACTION_ALLOW || action == FIREWALL_RULE_ACTION_ALERT;
    (lets_the_query_through && stated.any_stated()).then(firewall_rule_block_option_not_for_action)
}

/// One DNS Firewall rule to update, as the operations that update one state it.
///
/// `UpdateFirewallRuleEntry` — one entry of a batch update — declares exactly
/// the members `UpdateFirewallRuleRequest` does
/// (`smithy_query(query="shape",shape="UpdateFirewallRuleEntry")`), so both
/// describe the same change and both decide it through
/// [`ComAmazonawsRoute53resolverRoute53resolverState::update_firewall_rule`]
/// instead of each carrying its own copy of what updating a rule means.
///
/// `firewall_rule_group_id` and the match source NAME the rule — the match
/// source is immutable — and every other member replaces the stored value when
/// the request carries it and leaves it alone when it does not.
#[derive(Debug, Clone, Copy)]
pub struct FirewallRuleUpdate<'a> {
    pub firewall_rule_group_id: &'a str,
    pub firewall_domain_list_id: Option<&'a str>,
    pub firewall_threat_protection_id: Option<&'a str>,
    pub priority: Option<i32>,
    pub action: Option<&'a str>,
    pub block_response: Option<&'a str>,
    pub block_override_domain: Option<&'a str>,
    pub block_override_dns_type: Option<&'a str>,
    pub block_override_ttl: Option<i32>,
    pub name: Option<&'a str>,
    pub firewall_domain_redirection_action: Option<&'a str>,
    pub qtype: Option<&'a str>,
    pub dns_threat_protection: Option<&'a str>,
    pub confidence_threshold: Option<&'a str>,
    pub firewall_rule_type: Option<&'a FirewallRuleType>,
}

/// Why one firewall-rule update was refused.
///
/// The same relation to [`FirewallRuleCreateRefusal`] that an update has to a
/// create: an update that carries ONE rule reports it as the modelled exception
/// it holds and the whole call fails, while a batch update decides each entry on
/// its own and reports the same refusal in the response body — the exception's
/// shape name as the error's `Code` ([`Self::code`]) and its sentence as the
/// `Message`.
#[derive(Debug, Clone)]
pub enum FirewallRuleUpdateRefusal {
    Validation(ValidationException),
    ResourceNotFound(ResourceNotFoundException),
    Conflict(ConflictException),
}

impl FirewallRuleUpdateRefusal {
    /// The error code of this refusal: the modelled shape name of the exception
    /// that states it, which is the code the exception carries on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "ValidationException",
            Self::ResourceNotFound(_) => "ResourceNotFoundException",
            Self::Conflict(_) => "ConflictException",
        }
    }

    /// The sentence this refusal states, as the exception's `Message`.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Validation(error) => error.message.clone(),
            Self::ResourceNotFound(error) => error.message.clone(),
            Self::Conflict(error) => error.message.clone(),
        }
    }
}

/// The match source a `FirewallRuleType` names: the union member that is set and
/// the value it matches against, as one string.
///
/// What a rule matches is fixed when the rule is made, so an update that
/// restates the rule type has to restate the SAME source; the confidence
/// threshold a threat detector carries is a setting of the rule and not part of
/// what it matches, so it is left out of this. `None` for a union with no member
/// set, which [`firewall_rule_settings_fault`] refuses in its own right.
pub fn firewall_rule_type_match_source(rule_type: &FirewallRuleType) -> Option<String> {
    if let Some(config) = &rule_type.partner_threat_protection {
        return Some(format!("PartnerThreatProtection={}", config.partner));
    }
    if let Some(config) = &rule_type.firewall_advanced_content_category {
        return Some(format!(
            "FirewallAdvancedContentCategory={}",
            config.category
        ));
    }
    if let Some(config) = &rule_type.firewall_advanced_threat_category {
        return Some(format!("FirewallAdvancedThreatCategory={}", config.category));
    }
    if let Some(config) = &rule_type.dns_threat_protection {
        return Some(format!("DnsThreatProtection={}", config.value));
    }
    None
}

/// The `ValidationException` an update is refused with when it asks to change
/// what the rule matches against.
///
/// `UpdateFirewallRule`'s doc states the rule: "The rule's `FirewallRuleType`,
/// `FirewallDomainListId`, and top-level `DnsThreatProtection` match source
/// cannot be changed after creation." The member the request restated
/// differently is what the sentence names.
pub fn firewall_rule_match_source_is_immutable(member: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} identifies the rule's match source and cannot be changed after the \
             rule is created"
        )),
    }
}

/// How one request names a single stored rule of one rule group.
///
/// `DeleteFirewallRuleEntry` declares exactly the members
/// `DeleteFirewallRuleRequest` does
/// (`smithy_query(query="shape",shape="DeleteFirewallRuleEntry")`), and the two
/// updates name their rule by those same members, so what NAMES a rule is
/// decided in ONE place for every operation that addresses a stored rule
/// ([`locate_firewall_rule`]) instead of each carrying its own copy of it.
#[derive(Debug, Clone, Copy)]
pub struct FirewallRuleMatchSource<'a> {
    pub firewall_rule_group_id: &'a str,
    pub firewall_domain_list_id: Option<&'a str>,
    pub firewall_threat_protection_id: Option<&'a str>,
    pub qtype: Option<&'a str>,
    /// What the request asks of the rule it names, as the sentences a request
    /// that names no single rule is refused with word it.
    pub action: &'a str,
}

impl<'a> FirewallRuleMatchSource<'a> {
    /// The member this request names its rule by, as the service words it, and
    /// the id it carried there — or `None` when it carries both ids or neither,
    /// in both of which it names no single rule.
    ///
    /// The member is part of what the service says when no rule of the group
    /// answers to what was named ([`firewall_rule_not_found`]), so which member
    /// that is and which id it carries are read out in ONE place.
    pub fn match_source(&self) -> Option<(&'static str, &'a str)> {
        match (
            self.firewall_domain_list_id,
            self.firewall_threat_protection_id,
        ) {
            (Some(id), None) => Some((FIREWALL_RULE_DOMAIN_LIST_MEMBER, id)),
            (None, Some(id)) => Some((FIREWALL_RULE_THREAT_PROTECTION_MEMBER, id)),
            _ => None,
        }
    }
}

/// How the service words `FirewallDomainListId`, the member a domain-list rule
/// is named by, inside the sentence it reports when no rule of the group
/// answers to it: the modelled member read as words.
pub const FIREWALL_RULE_DOMAIN_LIST_MEMBER: &str = "Firewall domain list ID";

/// The same for `FirewallThreatProtectionId`, the member a DNS Firewall
/// Advanced rule is named by.
pub const FIREWALL_RULE_THREAT_PROTECTION_MEMBER: &str = "Firewall threat protection ID";

/// Why no single stored rule answers to what a request named.
///
/// The two refusals naming a rule can fail with, which every operation that
/// addresses a stored rule reports as its own refusal
/// ([`FirewallRuleUpdateRefusal`], [`FirewallRuleDeleteRefusal`]) — so one rule
/// named the same way is refused on the same grounds whichever operation named
/// it.
#[derive(Debug, Clone)]
pub enum FirewallRuleLookupFault {
    Validation(ValidationException),
    ResourceNotFound(ResourceNotFoundException),
}

/// Where among `group`'s rules the rule the request names sits, beside the
/// member it was named by and the id it carried there.
///
/// This is the whole of what naming one stored rule means: a rule is addressed
/// by what it matches on — `FirewallDomainListId` names a domain-list rule and
/// `FirewallThreatProtectionId` names a DNS Firewall Advanced rule, and the two
/// name different rules, so a request that carries both or neither names none —
/// and among the rules of one group that match the SAME source, the `Qtype` the
/// request names is what tells them apart.
///
/// The match source travels back with the index because a request that reaches
/// a rule named exactly one member, and what a LATER refusal about that rule
/// states is that member and that id: carrying it out of here is what saves
/// every caller from re-deriving a pair this function already proved.
pub fn locate_firewall_rule<'a>(
    group: &FirewallRuleGroupRecord,
    named: &FirewallRuleMatchSource<'a>,
) -> Result<(usize, (&'static str, &'a str)), FirewallRuleLookupFault> {
    let Some((match_source_member, match_source_id)) = named.match_source() else {
        return Err(FirewallRuleLookupFault::Validation(ValidationException {
            message: Some(format!(
                "Exactly one of FirewallDomainListId and FirewallThreatProtectionId \
                 names the rule to {}",
                named.action
            )),
        }));
    };
    let by_domain_list = named.firewall_domain_list_id.is_some();
    let candidates: Vec<usize> = group
        .rules
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            let stored = if by_domain_list {
                record.rule.firewall_domain_list_id.as_deref()
            } else {
                record.rule.firewall_threat_protection_id.as_deref()
            };
            stored == Some(match_source_id)
        })
        .map(|(index, _)| index)
        .collect();

    match candidates.as_slice() {
        [] => Err(FirewallRuleLookupFault::ResourceNotFound(
            firewall_rule_not_found(
                named.firewall_rule_group_id,
                match_source_member,
                match_source_id,
            ),
        )),
        [only] => Ok((*only, (match_source_member, match_source_id))),
        // Among rules of one group that match the same source, the `Qtype` the
        // request names is what tells them apart — a rule that evaluates every
        // query type stores none, so a request that names none names that rule.
        // Where SEVERAL rules answer to the query type the request named, the
        // first of them in the group's order is the one addressed: a request has
        // no other member to name a rule by, so refusing would leave such rules
        // impossible to update or delete at all, while a teardown that deletes
        // them one at a time reaches each of them in turn. Where NONE answers to
        // it, the request has not said which rule it means.
        several => {
            let mut evaluating_the_qtype = several.iter().copied().filter(|index| {
                group
                    .rules
                    .get(*index)
                    .and_then(|record| record.rule.qtype.as_deref())
                    == named.qtype
            });
            match evaluating_the_qtype.next() {
                Some(first) => Ok((first, (match_source_member, match_source_id))),
                None => Err(FirewallRuleLookupFault::Validation(ValidationException {
                    message: Some(format!(
                        "More than one rule of firewall rule group {} matches \
                         {match_source_id}; name the Qtype of the rule to {}",
                        named.firewall_rule_group_id, named.action
                    )),
                })),
            }
        }
    }
}

/// The word [`FirewallRuleMatchSource::action`] carries for the operations that
/// update one stored rule, and for the ones that delete one.
pub const FIREWALL_RULE_ACTION_WORD_UPDATE: &str = "update";
pub const FIREWALL_RULE_ACTION_WORD_DELETE: &str = "delete";

/// Why one firewall-rule delete was refused.
///
/// The same relation to [`FirewallRuleCreateRefusal`] that a delete has to a
/// create: a delete that carries ONE rule reports it as the modelled exception
/// it holds and the whole call fails, while a batch delete decides each entry on
/// its own and reports the same refusal in the response body — the exception's
/// shape name as the error's `Code` ([`Self::code`]) and its sentence as the
/// `Message`.
#[derive(Debug, Clone)]
pub enum FirewallRuleDeleteRefusal {
    Validation(ValidationException),
    ResourceNotFound(ResourceNotFoundException),
}

impl FirewallRuleDeleteRefusal {
    /// The error code of this refusal: the modelled shape name of the exception
    /// that states it, which is the code the exception carries on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "ValidationException",
            Self::ResourceNotFound(_) => "ResourceNotFoundException",
        }
    }

    /// The sentence this refusal states, as the exception's `Message`.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Validation(error) => error.message.clone(),
            Self::ResourceNotFound(error) => error.message.clone(),
        }
    }
}

impl From<FirewallRuleLookupFault> for FirewallRuleDeleteRefusal {
    fn from(fault: FirewallRuleLookupFault) -> Self {
        match fault {
            FirewallRuleLookupFault::Validation(error) => Self::Validation(error),
            FirewallRuleLookupFault::ResourceNotFound(error) => Self::ResourceNotFound(error),
        }
    }
}

impl From<FirewallRuleLookupFault> for FirewallRuleUpdateRefusal {
    fn from(fault: FirewallRuleLookupFault) -> Self {
        match fault {
            FirewallRuleLookupFault::Validation(error) => Self::Validation(error),
            FirewallRuleLookupFault::ResourceNotFound(error) => Self::ResourceNotFound(error),
        }
    }
}

impl ComAmazonawsRoute53resolverRoute53resolverState {
    /// Create one DNS Firewall rule in the group the request names, and answer
    /// it as the operations report it as of `now`.
    ///
    /// This is the whole of what creating one rule means: the modelled
    /// constraints the boundary validator cannot express, the match sources that
    /// exclude one another, the settings one `Action` and one `BlockResponse`
    /// require, the idempotent retry of a `CreatorRequestId`, the priority that
    /// has to be unique in the group, the bound on what one group holds, and the
    /// lifecycle state a rule whose provisioning is asynchronous begins in.
    /// Deciding it HERE is what makes a rule created one at a time and a rule
    /// created as one entry of a batch the same rule, admitted and refused on the
    /// same grounds. A refusal writes nothing.
    pub fn create_firewall_rule(
        &mut self,
        request: FirewallRuleCreate<'_>,
        now: Instant,
    ) -> Result<FirewallRule, FirewallRuleCreateRefusal> {
        // Every setting the rule is to carry has to be one the service admits,
        // and a create states them all. The same question is asked of the
        // settings an update arrives at, so it is answered in ONE place for
        // both ([`firewall_rule_settings_fault`]).
        let settings = FirewallRuleSettings {
            name: request.name,
            action: request.action,
            block_response: request.block_response,
            block_override_domain: request.block_override_domain,
            block_override_dns_type: request.block_override_dns_type,
            block_override_ttl: request.block_override_ttl,
            firewall_domain_redirection_action: request.firewall_domain_redirection_action,
            firewall_domain_list_id: request.firewall_domain_list_id,
            dns_threat_protection: request.dns_threat_protection,
            confidence_threshold: request.confidence_threshold,
            firewall_rule_type: request.firewall_rule_type,
        };
        if let Some(fault) = firewall_rule_settings_fault(&settings) {
            return Err(FirewallRuleCreateRefusal::Validation(fault));
        }
        // A block setting belongs to the action that blocks, and a create states
        // the rule's action itself ([`firewall_rule_block_option_fault`]).
        let stated_block_options = FirewallRuleBlockOptions {
            block_response: request.block_response,
            block_override_domain: request.block_override_domain,
            block_override_dns_type: request.block_override_dns_type,
            block_override_ttl: request.block_override_ttl,
        };
        if let Some(fault) = firewall_rule_block_option_fault(request.action, &stated_block_options)
        {
            return Err(FirewallRuleCreateRefusal::Validation(fault));
        }
        let advanced = settings.is_advanced();

        let timestamp = now_rfc3339();

        // The rule group the rule is created in has to exist, and so does the
        // domain list a domain-list rule matches against — including one DNS
        // Firewall publishes itself, which such a rule may name.
        let Some(group) = self.firewall_rule_groups.get(request.firewall_rule_group_id) else {
            return Err(FirewallRuleCreateRefusal::ResourceNotFound(
                firewall_rule_group_not_found(request.firewall_rule_group_id),
            ));
        };

        // `CreatorRequestId` is "a unique string that identifies the request and
        // that allows you to retry failed requests without the risk of running
        // the operation twice": a repeat resolves to the rule it already made.
        if let Some(record) = group
            .rules
            .iter()
            .find(|record| record.rule.creator_request_id.as_deref() == Some(request.creator_request_id))
        {
            return Ok(record.to_rule(now));
        }

        // "You must specify a unique priority for each rule in a rule group":
        // the priority is unique over the GROUP, so a rule already holding it
        // clashes whatever else that rule states — its `Qtype`, its `Action`,
        // its match source scope nothing.
        if group
            .rules
            .iter()
            .any(|record| record.rule.priority == Some(request.priority))
        {
            return Err(FirewallRuleCreateRefusal::Validation(
                firewall_rule_priority_in_use(request.firewall_rule_group_id),
            ));
        }

        // The group's rules are bounded: a create past the bound is refused and
        // the stored rules are left exactly as they were.
        if group.rules.len() >= MAX_RULES_PER_FIREWALL_RULE_GROUP {
            return Err(FirewallRuleCreateRefusal::LimitExceeded(
                LimitExceededException {
                    message: Some(format!(
                        "This emulator stores at most {MAX_RULES_PER_FIREWALL_RULE_GROUP} rules \
                         per firewall rule group; delete one before creating another"
                    )),
                    resource_type: Some(RESOURCE_TYPE_FIREWALL_RULE_GROUP.to_string()),
                },
            ));
        }

        if let Some(firewall_domain_list_id) = request.firewall_domain_list_id {
            if !self
                .firewall_domain_lists
                .contains_key(firewall_domain_list_id)
                && managed_firewall_domain_list(firewall_domain_list_id).is_none()
            {
                return Err(FirewallRuleCreateRefusal::ResourceNotFound(
                    firewall_domain_list_not_found(firewall_domain_list_id),
                ));
            }
        }

        // A DNS Firewall Advanced rule carries an id of its own, which
        // `FirewallRule$FirewallThreatProtectionId` reports; a domain-list rule
        // is not one and carries none.
        let firewall_threat_protection_id = if advanced {
            let seq = self.next_seq();
            Some(mint_firewall_threat_protection_id(seq))
        } else {
            None
        };

        // "Rules created with the PartnerThreatProtection rule type begin in
        // [CREATING] while DNS Firewall verifies the calling account's Amazon Web
        // Services Marketplace entitlement" — the one match source whose
        // provisioning is asynchronous. This emulator holds no Marketplace
        // subscription, so that verification is what the clock owes the rule, and
        // it settles on the documented failure.
        let partner = request
            .firewall_rule_type
            .and_then(|rule_type| rule_type.partner_threat_protection.as_ref())
            .map(|config| config.partner.as_str());

        let rule = FirewallRule {
            firewall_rule_group_id: Some(request.firewall_rule_group_id.to_string()),
            firewall_domain_list_id: request.firewall_domain_list_id.map(str::to_string),
            firewall_threat_protection_id,
            name: Some(request.name.to_string()),
            priority: Some(request.priority),
            action: Some(request.action.to_string()),
            block_response: request.block_response.map(str::to_string),
            // The override target is a DNS NAME the rule answers the blocked
            // query with, so it is stored canonical
            // ([`firewall_domain_canonical`]) and every reader projects that one
            // spelling — the same reduction the domains of a domain list get.
            block_override_domain: request
                .block_override_domain
                .map(firewall_domain_canonical),
            block_override_dns_type: request.block_override_dns_type.map(str::to_string),
            block_override_ttl: request.block_override_ttl,
            creator_request_id: Some(request.creator_request_id.to_string()),
            creation_time: Some(timestamp.clone()),
            modification_time: Some(timestamp),
            // The redirection action applies to the redirection chain of a
            // domain the rule matched in its domain list, so it is reported —
            // with its documented default when the request names none — for a
            // domain-list rule alone.
            firewall_domain_redirection_action: request
                .firewall_domain_redirection_action
                .map(str::to_string)
                .or_else(|| {
                    request
                        .firewall_domain_list_id
                        .map(|_| FIREWALL_DOMAIN_REDIRECTION_INSPECT.to_string())
                }),
            qtype: request.qtype.map(str::to_string),
            dns_threat_protection: request.dns_threat_protection.map(str::to_string),
            confidence_threshold: request.confidence_threshold.map(str::to_string),
            firewall_rule_type: request.firewall_rule_type.cloned(),
            // The lifecycle state is the record's, not the rule's: it is decided
            // from the clock by `FirewallRuleRecord::to_rule`.
            status: None,
            status_message: None,
        };

        let record = match partner {
            Some(partner) => FirewallRuleRecord {
                rule,
                status: Some(FIREWALL_RULE_STATUS_CREATING.to_string()),
                settled_status: Some(FIREWALL_RULE_STATUS_CREATION_FAILED.to_string()),
                settled_status_message: Some(partner_subscription_missing(partner)),
                settled_at: Some(now + PROVISIONING_DELAY),
            },
            // Every other match source is in force the moment the rule exists,
            // so the rule needs no asynchronous provisioning and reports no
            // `Status` at all.
            None => FirewallRuleRecord {
                rule,
                status: None,
                settled_status: None,
                settled_status_message: None,
                settled_at: None,
            },
        };

        let reported = record.to_rule(now);
        let Some(group) = self
            .firewall_rule_groups
            .get_mut(request.firewall_rule_group_id)
        else {
            return Err(FirewallRuleCreateRefusal::ResourceNotFound(
                firewall_rule_group_not_found(request.firewall_rule_group_id),
            ));
        };
        group.rules.push(record);
        // The group's own `ModificationTime` and the step it reports move with
        // the rules it holds: a group a rule was just added to has changed.
        group.modification_time = now_rfc3339();
        group.change = FirewallChange::Changed;

        Ok(reported)
    }

    /// Update one DNS Firewall rule of the group the request names, and answer
    /// it as the operations report it as of `now`.
    ///
    /// This is the whole of what updating one rule means: the rule the request
    /// NAMES (its group and its match source), the members it leaves alone, the
    /// match source and rule type that "cannot be changed after creation", the
    /// settings the rule has to satisfy once the change is applied, the priority
    /// that has to stay unique in the group, and the lifecycle states a rule
    /// cannot be updated in. Deciding it HERE is what makes a rule updated one
    /// at a time and a rule updated as one entry of a batch the same rule,
    /// admitted and refused on the same grounds — the relation
    /// [`Self::create_firewall_rule`] has to the two creates. A refusal writes
    /// nothing.
    pub fn update_firewall_rule(
        &mut self,
        request: FirewallRuleUpdate<'_>,
        now: Instant,
    ) -> Result<FirewallRule, FirewallRuleUpdateRefusal> {
        let Some(group) = self.firewall_rule_groups.get(request.firewall_rule_group_id) else {
            return Err(FirewallRuleUpdateRefusal::ResourceNotFound(
                firewall_rule_group_not_found(request.firewall_rule_group_id),
            ));
        };

        // The rule the entry NAMES, by the one rule every operation that
        // addresses a stored rule follows ([`locate_firewall_rule`]).
        let named = FirewallRuleMatchSource {
            firewall_rule_group_id: request.firewall_rule_group_id,
            firewall_domain_list_id: request.firewall_domain_list_id,
            firewall_threat_protection_id: request.firewall_threat_protection_id,
            qtype: request.qtype,
            action: FIREWALL_RULE_ACTION_WORD_UPDATE,
        };
        // The rule the entry named, beside the member it named it by and the id
        // it carried there — what a refusal about that rule states, carried out
        // of the lookup that proved them rather than re-derived here.
        let (index, (match_source_member, match_source_id)) =
            locate_firewall_rule(group, &named)?;

        let Some(stored) = group.rules.get(index).map(|record| record.to_rule(now)) else {
            return Err(FirewallRuleUpdateRefusal::ResourceNotFound(
                firewall_rule_not_found(
                    request.firewall_rule_group_id,
                    match_source_member,
                    match_source_id,
                ),
            ));
        };

        // "Rules whose Status is CREATING or CREATION_FAILED cannot be updated;
        // remove a failed rule with DeleteFirewallRule."
        if stored.status.as_deref().is_some_and(|status| {
            status == FIREWALL_RULE_STATUS_CREATING || status == FIREWALL_RULE_STATUS_CREATION_FAILED
        }) {
            return Err(FirewallRuleUpdateRefusal::Conflict(
                state_transition_conflict(),
            ));
        }

        // The settings the rule carries once the change is applied: a member the
        // request names replaces the stored value, a member it does not name
        // keeps it, and the match source is the stored one either way. They have
        // to be settings the service admits, exactly as for a create.
        let settings = FirewallRuleSettings {
            // A stored rule always carries both of these — the create requires
            // them — so the fallbacks below stand for an impossible record
            // rather than for a member the request may leave out.
            name: request.name.or(stored.name.as_deref()).unwrap_or_default(),
            action: request.action.or(stored.action.as_deref()).unwrap_or_default(),
            block_response: request.block_response.or(stored.block_response.as_deref()),
            block_override_domain: request
                .block_override_domain
                .or(stored.block_override_domain.as_deref()),
            block_override_dns_type: request
                .block_override_dns_type
                .or(stored.block_override_dns_type.as_deref()),
            block_override_ttl: request.block_override_ttl.or(stored.block_override_ttl),
            firewall_domain_redirection_action: request
                .firewall_domain_redirection_action
                .or(stored.firewall_domain_redirection_action.as_deref()),
            firewall_domain_list_id: stored.firewall_domain_list_id.as_deref(),
            dns_threat_protection: stored.dns_threat_protection.as_deref(),
            confidence_threshold: request
                .confidence_threshold
                .or(stored.confidence_threshold.as_deref()),
            firewall_rule_type: request
                .firewall_rule_type
                .or(stored.firewall_rule_type.as_ref()),
        };
        if let Some(fault) = firewall_rule_settings_fault(&settings) {
            return Err(FirewallRuleUpdateRefusal::Validation(fault));
        }

        // A block setting belongs to the action that blocks: one STATED by this
        // request is refused when the rule lets the query through once the
        // change is applied — whether the request moves the action there or the
        // rule was already there ([`firewall_rule_block_option_fault`]). The
        // block settings the rule already carries are left alone: they are not
        // something this request provided, and they apply to nothing until the
        // rule blocks again.
        let stated_block_options = FirewallRuleBlockOptions {
            block_response: request.block_response,
            block_override_domain: request.block_override_domain,
            block_override_dns_type: request.block_override_dns_type,
            block_override_ttl: request.block_override_ttl,
        };
        if let Some(fault) = firewall_rule_block_option_fault(settings.action, &stated_block_options)
        {
            return Err(FirewallRuleUpdateRefusal::Validation(fault));
        }

        // "The rule's FirewallRuleType, FirewallDomainListId, and top-level
        // DnsThreatProtection match source cannot be changed after creation": an
        // entry may restate the source it addressed the rule by, and a
        // restatement that names another source is refused. The confidence
        // threshold a threat detector carries is a setting and not part of what
        // the rule matches, so restating a rule type with another threshold is
        // an update of that setting.
        if let Some(requested) = request.dns_threat_protection {
            if stored.dns_threat_protection.as_deref() != Some(requested) {
                return Err(FirewallRuleUpdateRefusal::Validation(
                    firewall_rule_match_source_is_immutable("DnsThreatProtection"),
                ));
            }
        }
        if let Some(requested) = request.firewall_rule_type {
            let stored_source = stored
                .firewall_rule_type
                .as_ref()
                .and_then(firewall_rule_type_match_source);
            if firewall_rule_type_match_source(requested) != stored_source {
                return Err(FirewallRuleUpdateRefusal::Validation(
                    firewall_rule_match_source_is_immutable("FirewallRuleType"),
                ));
            }
        }

        // "You must specify a unique priority for each rule in a rule group" —
        // group-wide, and not scoped by the query type the rules evaluate.
        if let Some(requested) = request.priority {
            if group
                .rules
                .iter()
                .enumerate()
                .any(|(other, record)| other != index && record.rule.priority == Some(requested))
            {
                return Err(FirewallRuleUpdateRefusal::Validation(
                    firewall_rule_priority_in_use(request.firewall_rule_group_id),
                ));
            }
        }

        // Moving a rule onto the query type another rule of the group evaluates
        // would leave two rules no later request could tell apart — and cannot
        // happen: `Qtype` both selects among the rules that match one source
        // and states the value, so where more than one rule matches, the rule
        // selected is the one ALREADY evaluating that query type, and where one
        // rule matches there is no other rule to collide with.
        let timestamp = now_rfc3339();
        let Some(group) = self
            .firewall_rule_groups
            .get_mut(request.firewall_rule_group_id)
        else {
            return Err(FirewallRuleUpdateRefusal::ResourceNotFound(
                firewall_rule_group_not_found(request.firewall_rule_group_id),
            ));
        };
        let Some(record) = group.rules.get_mut(index) else {
            return Err(FirewallRuleUpdateRefusal::ResourceNotFound(
                firewall_rule_not_found(
                    request.firewall_rule_group_id,
                    match_source_member,
                    match_source_id,
                ),
            ));
        };

        let rule = &mut record.rule;
        if let Some(value) = request.priority {
            rule.priority = Some(value);
        }
        if let Some(value) = request.action {
            rule.action = Some(value.to_string());
        }
        if let Some(value) = request.block_response {
            rule.block_response = Some(value.to_string());
        }
        if let Some(value) = request.block_override_domain {
            rule.block_override_domain = Some(firewall_domain_canonical(value));
        }
        if let Some(value) = request.block_override_dns_type {
            rule.block_override_dns_type = Some(value.to_string());
        }
        if let Some(value) = request.block_override_ttl {
            rule.block_override_ttl = Some(value);
        }
        if let Some(value) = request.name {
            rule.name = Some(value.to_string());
        }
        if let Some(value) = request.firewall_domain_redirection_action {
            rule.firewall_domain_redirection_action = Some(value.to_string());
        }
        if let Some(value) = request.qtype {
            rule.qtype = Some(value.to_string());
        }
        if let Some(value) = request.confidence_threshold {
            rule.confidence_threshold = Some(value.to_string());
        }
        // The source it matches is the one it already matched — checked above —
        // so what this carries over is the threshold the detector reads at.
        if let Some(value) = request.firewall_rule_type {
            rule.firewall_rule_type = Some(value.clone());
        }
        rule.modification_time = Some(timestamp.clone());

        let reported = record.to_rule(now);
        // The group's own `ModificationTime` and the step it reports move with
        // the rules it holds: a group whose rule just changed has changed.
        group.modification_time = timestamp;
        group.change = FirewallChange::Changed;

        Ok(reported)
    }

    /// Delete one DNS Firewall rule of the group the request names, and answer
    /// the rule as it stood when it was deleted, as of `now`.
    ///
    /// This is the whole of what deleting one rule means: the rule the request
    /// NAMES (its group and its match source, told apart by the query type
    /// where more than one rule of the group matches that source), the rule
    /// leaving the group that held it — so the group's `RuleCount` and every
    /// later read of its rules follow from the one removal — and the group's own
    /// `ModificationTime` moving with the rules it holds. No lifecycle state
    /// refuses a delete: `DeleteFirewallRule` "is the only operation that
    /// succeeds against a rule whose Status is CREATION_FAILED", which an update
    /// is refused for. Deciding it HERE is what makes a rule deleted one at a
    /// time and a rule deleted as one entry of a batch the same deletion,
    /// admitted and refused on the same grounds — the relation
    /// [`Self::create_firewall_rule`] has to the two creates. A refusal writes
    /// nothing.
    pub fn delete_firewall_rule(
        &mut self,
        named: FirewallRuleMatchSource<'_>,
        now: Instant,
    ) -> Result<FirewallRule, FirewallRuleDeleteRefusal> {
        let Some(group) = self
            .firewall_rule_groups
            .get_mut(named.firewall_rule_group_id)
        else {
            return Err(FirewallRuleDeleteRefusal::ResourceNotFound(
                firewall_rule_group_not_found(named.firewall_rule_group_id),
            ));
        };
        let (index, _) = locate_firewall_rule(group, &named)?;
        let record = group.rules.remove(index);
        // The group's own `ModificationTime` and the step it reports move with
        // the rules it holds: a group a rule was just removed from has changed.
        group.modification_time = now_rfc3339();
        group.change = FirewallChange::Changed;

        // The rule is reported as the group held it at the moment it was
        // deleted, lifecycle state included, so a caller reads back exactly the
        // rule that is now gone.
        Ok(record.to_rule(now))
    }

    /// The id of a rule group holding a rule that matches against the domain
    /// list `firewall_domain_list_id`, if the store holds one — the oldest
    /// group, so a list several groups reference answers deterministically.
    ///
    /// This is the index `DeleteFirewallDomainList` consults before it takes a
    /// list out ([`firewall_domain_list_in_use_conflict`]): a list a rule
    /// matches against cannot be deleted, so no rule is ever left matching
    /// against a `FirewallDomainListId` the store no longer holds. The scan is
    /// over a store bounded by [`MAX_STORED_FIREWALL_RULE_GROUPS`], each group
    /// by [`MAX_RULES_PER_FIREWALL_RULE_GROUP`].
    pub fn firewall_rule_group_referencing_domain_list(
        &self,
        firewall_domain_list_id: &str,
    ) -> Option<&str> {
        self.firewall_rule_groups
            .values()
            .filter(|group| {
                group.rules.iter().any(|record| {
                    record.rule.firewall_domain_list_id.as_deref() == Some(firewall_domain_list_id)
                })
            })
            .min_by_key(|group| group.seq)
            .map(|group| group.id.as_str())
    }
}

/// The Route 53 Resolver service error code for a DNS Firewall domain list a
/// rule still matches against.
pub const RSLVR_CODE_FIREWALL_DOMAIN_LIST_IN_USE: &str = "RSLVR-02304";

/// The `ConflictException` `DeleteFirewallDomainList` is refused with while a
/// DNS Firewall rule still matches against the list it names.
///
/// A list a rule matches against is still deciding whether that rule fires, so
/// deleting it is not a state transition the service makes: the rule goes first
/// ("delete the rule"), which is the teardown order
/// `DeleteFirewallRule`/`DeleteFirewallDomainList` are meant to be called in,
/// and the model's `ConflictException` ("the requested state transition isn't
/// valid") is the answer for a transition it will not make.
///
/// The sentence names the RULE GROUP the referencing rule belongs to — a rule
/// is addressed by its group and its match source and carries no id of its own
/// — and closes with the trace clause of the request ([`traced_message`]), as
/// the refusals of a DNS Firewall resource another one still holds do.
pub fn firewall_domain_list_in_use_conflict(
    firewall_rule_group_id: &str,
    trace_root: &str,
) -> ConflictException {
    ConflictException {
        message: Some(traced_message(
            rslvr_message_sentences(
                RSLVR_CODE_FIREWALL_DOMAIN_LIST_IN_USE,
                &[
                    &format!(
                        "The DNS Firewall domain list is associated with a DNS Firewall rule : \
                         \"{firewall_rule_group_id}\""
                    ),
                    "Delete the rule and then try again",
                ],
            ),
            trace_root,
        )),
    }
}
