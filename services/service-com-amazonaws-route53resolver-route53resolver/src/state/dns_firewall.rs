// SPDX-License-Identifier: CC0-1.0

//! What DNS FIREWALL decides about a query asked from inside a VPC, before anything resolves
//! the name.
//!
//! # Which rule groups apply to a VPC
//!
//! A rule group reaches a VPC two ways, and both are read here:
//!
//! * directly, through an association of its own — `AssociateFirewallRuleGroup` "associates a
//!   <a>FirewallRuleGroup</a> with a VPC, to provide DNS filtering for the VPC", carrying the
//!   `Priority` that "sets the order of … rule group among the rule groups you associate with
//!   the specified VPC";
//! * through a Route 53 PROFILE associated with that VPC — a Profile holds DNS configuration
//!   that is pushed onto every VPC it is associated with, and a DNS Firewall rule group is one
//!   of the resources it can hold (`AssociateResourceToProfile`, whose `ResourceProperties`
//!   "indicates the priority" for such a rule group).
//!
//! Only a rule group whose association has settled applies: an association still being built
//! is not yet filtering, and a rule still `CREATING` is not yet "provisioned and enforcing
//! matches" (`FirewallRule$Status`).
//!
//! # What the firewall decides
//!
//! "DNS Firewall processes the rules in a rule group by order of priority, starting from the
//! lowest setting" (`FirewallRule$Priority`), and the rule groups themselves in the order of
//! their association priorities, so the FIRST rule whose domain list holds the name asked for
//! — and whose `Qtype`, where it names one, is the type asked about — is the one that decides:
//! `ALLOW` lets the query through, `ALERT` lets it through as well ("permit the request to go
//! through but send an alert to the logs"), and `BLOCK` answers it the rule's `BlockResponse`
//! way: `NXDOMAIN` as a name that does not exist, `NODATA` as a successful query with no
//! record, `OVERRIDE` with the rule's own record (`FirewallRule$Action`,
//! `FirewallRule$BlockResponse`).
//!
//! # A VIEW, not a live firewall
//!
//! Everything a decision needs is read ONCE, into [`FirewallView`], by the component that is
//! about to answer queries — the same shape as [`super::vpc_dns`], and for the same reason:
//! deciding a query then holds no store guard at all. Nothing here writes to any store.

use std::sync::Arc;
use std::time::Instant;

use ::service_com_amazonaws_route53profiles_route53profiles::state as profiles;
use ::service_common::vpc_dns_firewall::{
    FirewallDecision, VpcQueryFilter, VpcQueryFilters, OVERRIDE_DNS_TYPE_CNAME,
};

use super::{
    ComAmazonawsRoute53resolverRoute53resolverState, FirewallRuleGroupRecord,
    FIREWALL_RULE_ACTION_ALERT, FIREWALL_RULE_ACTION_ALLOW, FIREWALL_RULE_ACTION_BLOCK,
    FIREWALL_RULE_BLOCK_RESPONSE_OVERRIDE, FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE,
    FIREWALL_RULE_GROUP_STATUS_COMPLETE, FIREWALL_RULE_STATUS_COMPLETE,
};

/// `BlockResponse` `NODATA`: "respond indicating that the query was successful, but no response
/// is available for it". Named like the sibling value this service's store already declares
/// ([`FIREWALL_RULE_BLOCK_RESPONSE_OVERRIDE`]), so the enum's constants carry one prefix.
pub const FIREWALL_RULE_BLOCK_RESPONSE_NODATA: &str = "NODATA";

/// `BlockResponse` `NXDOMAIN`: "respond indicating that the domain name that's in the query
/// doesn't exist".
pub const FIREWALL_RULE_BLOCK_RESPONSE_NXDOMAIN: &str = "NXDOMAIN";

/// The TTL an `OVERRIDE` block answers with when the rule names no `BlockOverrideTtl`: the
/// member is "the recommended amount of time … to cache the provided override record", and a
/// rule that recommends none is answered with the minute this emulator uses wherever it holds
/// no TTL of its own.
const DEFAULT_OVERRIDE_TTL: u32 = 60;

/// How many rules one view carries, across every rule group that reaches the VPC.
///
/// The view is one reading of a store a client fills, so what it can hold is bounded here
/// rather than by how many rules the account created. Far above any VPC's own filtering: a rule
/// group holds at most [`super::MAX_RULES_PER_FIREWALL_RULE_GROUP`] rules.
const MAX_VIEW_RULES: usize = 1_000;

/// How many domains of one rule's domain list a view carries, which is the published ceiling
/// on how many one request may add to a list ("there is a limit of 1000 domains per request",
/// `UpdateFirewallDomainsRequest$Domains`).
const MAX_VIEW_DOMAINS_PER_RULE: usize = 1_000;

/// The one wildcard form a domain list entry may take: "it can optionally start with `*`
/// (asterisk)" (`UpdateFirewallDomainsRequest$Domains`), which matches the domain that follows
/// it and every name under it.
const WILDCARD_PREFIX: &str = "*.";

/// ONE rule of the view, as a decision reads it.
///
/// What the rule decides is what the register states ([`::service_common::vpc_dns_firewall`]):
/// `Allow` for a matched `ALLOW` or `ALERT` rule, which is also what a query no rule matches is
/// answered after, and one variant per `BlockResponse` a matched `BLOCK` rule answers with.
#[derive(Debug, Clone)]
struct ViewRule {
    /// Where the rule sits in the order a query is decided in: the priority of the association
    /// that brought its rule group to the VPC, then the rule's own priority inside the group,
    /// then the group's creation order — so two rule groups associated at the same priority are
    /// still decided in one stable order.
    order: (i64, i64, u64),
    /// The entries of the rule's domain list, canonical (lowercase, no trailing dot).
    domains: Vec<String>,
    /// The query type the rule evaluates, as RFC 1035 numbers it, or `None` for a rule that
    /// evaluates every type (`FirewallRule$Qtype`, which is optional).
    qtype: Option<u16>,
    /// What the rule does to a query it matches.
    decision: FirewallDecision,
    /// How the rule is NAMED in a record of a query it matched: its `Action`, the rule group it
    /// is in and the domain list it matches against ([`MatchedFirewallRule`]).
    matched: MatchedFirewallRule,
}

/// What a query log states about the DNS Firewall rule a query MATCHED, as the members of a
/// Resolver query log record name it: the action the rule took, the rule group the rule is in,
/// and the domain list it matched the name against.
///
/// Held beside the decision rather than derived from it, because two rules with the same
/// decision are still different rules — and because the actions that CHANGE no answer (`ALLOW`,
/// `ALERT`) are the ones a record of the match is the only evidence of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedFirewallRule {
    /// `FirewallRule$Action` of the rule that matched.
    pub action: String,
    /// The id of the rule group the rule belongs to.
    pub rule_group_id: String,
    /// The id of the domain list the rule matched the name against.
    pub domain_list_id: String,
}

/// One instant's reading of the DNS Firewall filtering a VPC's queries.
#[derive(Debug, Clone, Default)]
pub struct FirewallView {
    /// Every rule that applies to the VPC, in the order a query is decided in.
    rules: Vec<ViewRule>,
}

#[cfg(test)]
impl FirewallView {
    /// A view carrying ONE rule, for the tests of the answer paths that take a view: the rule
    /// matches the canonical forms of `domains`, evaluates `qtype` where one is named, and
    /// decides a query it matches the `decision` way.
    pub(crate) fn of_rule(
        domains: &[&str],
        qtype: Option<u16>,
        decision: FirewallDecision,
    ) -> Self {
        // The ACTION of a rule that decides a query the `decision` way, so a view built here
        // cannot report a match that contradicts the decision it was built with.
        let action = match &decision {
            FirewallDecision::Allow => FIREWALL_RULE_ACTION_ALLOW,
            _blocked => FIREWALL_RULE_ACTION_BLOCK,
        };
        Self {
            rules: vec![ViewRule {
                order: (profiles::MIN_FIREWALL_PRIORITY, 1, 0),
                domains: domains.iter().map(|domain| canonical_domain(domain)).collect(),
                qtype,
                decision,
                matched: MatchedFirewallRule {
                    action: action.to_string(),
                    rule_group_id: "rslvr-frg-testrulegroup".to_string(),
                    domain_list_id: "rslvr-fdl-testdomainlist".to_string(),
                },
            }],
        }
    }
}

impl FirewallView {
    /// Whether no rule filters this VPC's queries at all, so nothing here can change an answer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// What the firewall decides about a query for `name` of type `qtype`, as RFC 1035 numbers
    /// the type — `None` when no rule matches it, which is a query the firewall leaves alone.
    ///
    /// `qtype` is `None` for a query type this emulator cannot number; only a rule that names no
    /// type at all decides such a query, because a rule naming one evaluates that type alone.
    ///
    /// The first matching rule decides, in the order the rules were read into the view.
    #[must_use]
    pub fn decision(&self, name: &str, qtype: Option<u16>) -> Option<FirewallDecision> {
        self.matching(name, qtype)
            .map(|rule| rule.decision.clone())
    }

    /// How the rule that DECIDED a query for `name` of type `qtype` is named in a record of that
    /// query, and `None` when no rule matched it — which is a query no record names a rule for.
    ///
    /// The same rule [`Self::decision`] answers from, so what a query was decided by and what a
    /// record of it states cannot disagree.
    #[must_use]
    pub fn matched(&self, name: &str, qtype: Option<u16>) -> Option<MatchedFirewallRule> {
        self.matching(name, qtype).map(|rule| rule.matched.clone())
    }

    /// The rule of the view that decides a query for `name` of type `qtype`: the FIRST that
    /// matches, in the order the rules were read into the view.
    fn matching(&self, name: &str, qtype: Option<u16>) -> Option<&ViewRule> {
        let asked = canonical_domain(name);
        self.rules.iter().find(|rule| {
            rule.qtype.is_none_or(|only| qtype == Some(only))
                && rule
                    .domains
                    .iter()
                    .any(|entry| entry_matches(entry, &asked))
        })
    }
}

/// A domain as both a list entry and a queried name are compared: lowercase, with the trailing
/// dot of a fully qualified name dropped.
fn canonical_domain(name: &str) -> String {
    name.trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Whether the domain list entry `entry` covers the canonical name `asked`.
///
/// An entry is matched whole: `example.com` covers that name and nothing else, while the
/// wildcard form `*.example.com` covers it and every name under it — the only form an entry may
/// take besides a plain domain ("it can optionally start with `*`").
fn entry_matches(entry: &str, asked: &str) -> bool {
    match entry.strip_prefix(WILDCARD_PREFIX) {
        Some(suffix) => {
            asked == suffix
                || asked
                    .strip_suffix(suffix)
                    .is_some_and(|label| label.ends_with('.'))
        }
        None => entry == asked,
    }
}

/// The RFC 1035 number of a `Qtype` as a rule spells it, or `None` for a spelling that names no
/// type.
///
/// The names are the ones `FirewallRule$Qtype` lists as allowed values, each with the number
/// IANA's DNS parameter registry assigns it, plus the generic `TYPENUMBER` form that member
/// also admits ("the values must be defined as TYPENUMBER, where the NUMBER can be 1-65534").
#[must_use]
pub fn qtype_number(spelling: &str) -> Option<u16> {
    let named = spelling.trim().to_ascii_uppercase();
    let number = match named.as_str() {
        "A" => 1,
        "NS" => 2,
        "CNAME" => 5,
        "SOA" => 6,
        "PTR" => 12,
        "MX" => 15,
        "TXT" => 16,
        "AAAA" => 28,
        "SRV" => 33,
        "NAPTR" => 35,
        "DS" => 43,
        "SPF" => 99,
        "CAA" => 257,
        _ => {
            let digits = named.strip_prefix("TYPE")?;
            let number: u16 = digits.parse().ok()?;
            if !(1..=65_534).contains(&number) {
                return None;
            }
            number
        }
    };
    Some(number)
}

/// The store handles the firewall is read off: this service's own rule groups, rules, domain
/// lists and associations, and the Route 53 Profiles a rule group can reach a VPC through.
///
/// A handle is an `Arc` on the state itself rather than on the engine, so the filter this holds
/// keeps no engine alive and reads the SAME records the two services' own operations write.
#[derive(Clone)]
pub struct StoredFirewall {
    /// This service's own store.
    resolver: ::engine::ServiceState<ComAmazonawsRoute53resolverRoute53resolverState>,
    /// The Route 53 Profiles store, or `None` for an engine that vends none — then a rule
    /// group reaches a VPC only through an association of its own.
    profiles: Option<
        ::engine::ServiceState<
            profiles::ComAmazonawsRoute53profilesRoute53profilesState,
        >,
    >,
}

/// Written by hand rather than derived: the state handles inside carry no `Debug`, and what is
/// diagnosable about this filter is which stores it reads.
impl std::fmt::Debug for StoredFirewall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredFirewall")
            .field("profiles", &self.profiles.is_some())
            .finish()
    }
}

impl StoredFirewall {
    /// The filter reading the stores of `engine`, or `None` for an engine that vends this
    /// service none.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        Some(Self {
            resolver: super::service_state(engine)?,
            profiles: profiles::service_state(engine),
        })
    }

    /// The DNS Firewall filtering the queries of `vpc_id` as of now.
    ///
    /// Call it with NO store guard held: it takes each store in turn and releases it before the
    /// next.
    #[must_use]
    pub fn view_for_vpc(&self, vpc_id: &str) -> FirewallView {
        let now = Instant::now();
        let mut applied = self.applied_rule_groups(vpc_id, now);
        applied.extend(self.profile_rule_groups(vpc_id, now));
        if applied.is_empty() {
            return FirewallView::default();
        }
        let store = self.resolver.lock();
        let mut rules: Vec<ViewRule> = Vec::new();
        for (group_id, priority) in applied {
            let Some(group) = store.firewall_rule_groups.get(&group_id) else {
                continue;
            };
            // A rule group still being provisioned is not yet filtering anything.
            if group.status_at(now) != FIREWALL_RULE_GROUP_STATUS_COMPLETE {
                continue;
            }
            for rule in &group.rules {
                if rules.len() >= MAX_VIEW_RULES {
                    break;
                }
                if let Some(read) = view_rule(&store, group, rule.to_rule(now), priority) {
                    rules.push(read);
                }
            }
        }
        drop(store);
        rules.sort_by_key(|rule| rule.order);
        FirewallView { rules }
    }

    /// The rule groups associated with `vpc_id` through this service's own associations, each
    /// with the priority its association carries.
    fn applied_rule_groups(&self, vpc_id: &str, now: Instant) -> Vec<(String, i64)> {
        let store = self.resolver.lock();
        store
            .firewall_rule_group_associations
            .values()
            .filter(|association| association.vpc_id == vpc_id)
            // An association still being built is not filtering the VPC's queries yet.
            .filter(|association| {
                association.status_at(now) == FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE
            })
            .map(|association| {
                (
                    association.firewall_rule_group_id.clone(),
                    i64::from(association.priority),
                )
            })
            .collect()
    }

    /// The rule groups that reach `vpc_id` through the Route 53 Profiles associated with it,
    /// each with the priority its own resource association states.
    ///
    /// The Profiles are that service's records, read through its own accessors: a VPC's Profile
    /// associations are evaluated in priority order, lowest first, and the DNS Firewall rule
    /// groups each one pushes are the resource associations of its Profile whose `ResourceType`
    /// is a rule group. An association whose own priority change is still propagating stays in
    /// force throughout, so only one being removed pushes nothing.
    fn profile_rule_groups(&self, vpc_id: &str, now: Instant) -> Vec<(String, i64)> {
        let Some(state) = self.profiles.as_ref() else {
            return Vec::new();
        };
        let store = state.lock();
        let mut associated: Vec<(i32, String)> = Vec::new();
        for association in store.associations_for_resource(vpc_id) {
            // An association being REMOVED pushes nothing; one whose own priority change is
            // still propagating stays in force throughout.
            if association.status == profiles::STATUS_DELETING {
                continue;
            }
            associated.push((association.priority, association.profile_id.clone()));
        }
        // Read in the order the Resolver evaluates the VPC's Profile associations in, lowest
        // priority first, so a rule-group priority two Profiles both state is broken by the
        // order their associations are evaluated in.
        associated.sort();
        let mut reaching: Vec<(String, i64)> = Vec::new();
        for (_, profile_id) in associated {
            for resource in store.profile_resource_associations.values() {
                if resource.profile_id != profile_id {
                    continue;
                }
                if resource.resource_type != profiles::FIREWALL_RULE_GROUP_RESOURCE_TYPE {
                    continue;
                }
                // A resource association still being added to the Profile is not pushed to the
                // VPC yet.
                if resource.status_at(now) != profiles::STATUS_COMPLETE {
                    continue;
                }
                if let Some(group_id) = rule_group_id_of_arn(&resource.resource_arn) {
                    reaching.push((
                        group_id,
                        firewall_priority_of(resource.resource_properties.as_deref()),
                    ));
                }
            }
        }
        reaching
    }
}

impl VpcQueryFilter for StoredFirewall {
    fn decisions(
        &self,
        vpc_id: &str,
        asked: &[(String, Option<u16>)],
    ) -> Vec<Option<FirewallDecision>> {
        // ONE reading of the rules for the whole message: what a message of many queries costs
        // is this one view, and never one view per query.
        let view = self.view_for_vpc(vpc_id);
        asked
            .iter()
            .map(|(name, qtype)| view.decision(name, *qtype))
            .collect()
    }
}

/// Record this service as what decides the in-VPC queries of `engine`, which the service does
/// when it is mounted on it ([`crate::service::EmulatedService::new`]).
///
/// A component of ANOTHER crate that answers a query asked inside a VPC then reads its
/// decisions out of that register ([`::service_common::vpc_dns_firewall`]) instead of holding a
/// copy of the rules — the rules are this service's records, and a register keeps the reading of
/// them here. This crate's own answer path takes the view directly
/// ([`StoredFirewall::view_for_vpc`]), which is the same reading.
pub fn register_as_the_vpc_query_filter<Eng: Send + Sync + 'static>(engine: &Eng) {
    if let (Some(filters), Some(firewall)) =
        (VpcQueryFilters::of(engine), StoredFirewall::of(engine))
    {
        filters.registering(Arc::new(firewall));
    }
}

/// The DNS Firewall filtering the queries of `vpc_id`, read off the stores of `engine`.
///
/// Call it with NO store guard held, as [`StoredFirewall::view_for_vpc`] asks. An engine that
/// vends this service no store filters nothing, which is the empty view.
#[must_use]
pub fn view_for_vpc<Eng: Send + Sync + 'static>(engine: &Eng, vpc_id: &str) -> FirewallView {
    StoredFirewall::of(engine)
        .map(|firewall| firewall.view_for_vpc(vpc_id))
        .unwrap_or_default()
}

/// One stored rule as the view holds it, or `None` for a rule that decides nothing: one that is
/// not provisioned yet, one whose domain list this store no longer holds or holds no domain in,
/// one naming a `Qtype` that is no DNS type, and one whose action this service does not model.
fn view_rule(
    store: &ComAmazonawsRoute53resolverRoute53resolverState,
    group: &FirewallRuleGroupRecord,
    rule: crate::ops::types::FirewallRule,
    group_priority: i64,
) -> Option<ViewRule> {
    // "COMPLETE — the rule is provisioned and enforcing matches"; a rule whose provisioning
    // needed no time at all reports no status, and is enforcing.
    if rule
        .status
        .as_deref()
        .is_some_and(|status| status != FIREWALL_RULE_STATUS_COMPLETE)
    {
        return None;
    }
    let qtype = match rule.qtype.as_deref() {
        Some(spelling) => Some(qtype_number(spelling)?),
        None => None,
    };
    let decision = decision_of(&rule)?;
    let list = store
        .firewall_domain_lists
        .get(rule.firewall_domain_list_id.as_deref()?)?;
    let domains: Vec<String> = list
        .domains
        .iter()
        .take(MAX_VIEW_DOMAINS_PER_RULE)
        .map(|domain| canonical_domain(domain))
        .collect();
    if domains.is_empty() {
        return None;
    }
    Some(ViewRule {
        order: (group_priority, i64::from(rule.priority?), group.seq),
        domains,
        qtype,
        decision,
        matched: MatchedFirewallRule {
            action: rule.action.clone().unwrap_or_default(),
            rule_group_id: rule.firewall_rule_group_id.clone().unwrap_or_default(),
            domain_list_id: rule.firewall_domain_list_id.clone().unwrap_or_default(),
        },
    })
}

/// What one rule does to a query it matches, or `None` for an action this service does not
/// model.
fn decision_of(rule: &crate::ops::types::FirewallRule) -> Option<FirewallDecision> {
    let action = rule.action.as_deref()?;
    if action.eq_ignore_ascii_case(FIREWALL_RULE_ACTION_ALLOW)
        || action.eq_ignore_ascii_case(FIREWALL_RULE_ACTION_ALERT)
    {
        return Some(FirewallDecision::Allow);
    }
    if !action.eq_ignore_ascii_case(FIREWALL_RULE_ACTION_BLOCK) {
        return None;
    }
    let response = rule
        .block_response
        .as_deref()
        .unwrap_or(FIREWALL_RULE_BLOCK_RESPONSE_NODATA);
    if response.eq_ignore_ascii_case(FIREWALL_RULE_BLOCK_RESPONSE_NXDOMAIN) {
        return Some(FirewallDecision::BlockNxDomain);
    }
    if response.eq_ignore_ascii_case(FIREWALL_RULE_BLOCK_RESPONSE_OVERRIDE) {
        // An OVERRIDE block answers with the rule's own record, so a rule that names no record
        // has none to answer with.
        let domain = rule.block_override_domain.clone()?;
        return Some(FirewallDecision::BlockOverride {
            domain,
            dns_type: rule
                .block_override_dns_type
                .clone()
                .unwrap_or_else(|| OVERRIDE_DNS_TYPE_CNAME.to_string()),
            ttl: rule
                .block_override_ttl
                .and_then(|ttl| u32::try_from(ttl).ok())
                .unwrap_or(DEFAULT_OVERRIDE_TTL),
        });
    }
    Some(FirewallDecision::BlockNoData)
}

/// The rule group id a `firewall-rule-group` ARN names
/// (`arn:<partition>:route53resolver:<region>:<account>:firewall-rule-group/<id>`), or `None`
/// for an ARN naming anything else.
///
/// The ARN is read by the one ARN parse of this tree ([`::service_common::arn`]), which decides
/// the type and the id of a resource field in either form AWS writes one.
fn rule_group_id_of_arn(resource_arn: &str) -> Option<String> {
    let id = ::service_common::arn::resource_id_of_type(
        resource_arn,
        FIREWALL_RULE_GROUP_ARN_SEGMENT,
    )?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The resource-type segment of a DNS Firewall rule group's ARN, which is how this service
/// builds one and the segment a Profile's `ResourceArn` carries for a rule group.
const FIREWALL_RULE_GROUP_ARN_SEGMENT: &str = "firewall-rule-group";

/// The priority a Profile's `ResourceProperties` document states for a rule group, and the
/// highest the service accepts for one where the document states none: the member "indicates the
/// priority", and a rule group associated without one is filtered after every rule group that
/// named one.
///
/// The document is read by the service that owns it ([`profiles::firewall_priority_of`]), beside
/// the validator that decides which priorities it accepts, so its shape is read in one place.
fn firewall_priority_of(resource_properties: Option<&str>) -> i64 {
    profiles::firewall_priority_of(resource_properties).unwrap_or(profiles::MAX_FIREWALL_PRIORITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number of the `A` query type, which the rows below ask about.
    const TYPE_A: u16 = 1;

    /// One rule of a view, built for the rows below.
    fn rule(order: (i64, i64, u64), domains: &[&str], qtype: Option<u16>, decision: FirewallDecision) -> ViewRule {
        ViewRule {
            order,
            domains: domains.iter().map(|domain| canonical_domain(domain)).collect(),
            qtype,
            decision,
            // The rows below are about which rule decides; how a record of the query NAMES the
            // rule that matched is pinned by its own row, so every rule here is named alike
            // except for the order it sits in.
            matched: MatchedFirewallRule {
                action: FIREWALL_RULE_ACTION_BLOCK.to_string(),
                rule_group_id: format!("rslvr-frg-{}", order.2),
                domain_list_id: format!("rslvr-fdl-{}", order.2),
            },
        }
    }

    /// RULE: a record of a query names the rule that DECIDED it — its action, its rule group and
    /// its domain list — which is the same rule the decision came from, including the actions
    /// that change no answer.
    #[test]
    fn a_matched_rule_is_named_by_the_rule_that_decided_the_query() {
        let view = FirewallView {
            rules: vec![
                ViewRule {
                    order: (100, 1, 7),
                    domains: vec![canonical_domain("alerted.example.com")],
                    qtype: None,
                    decision: FirewallDecision::Allow,
                    matched: MatchedFirewallRule {
                        action: FIREWALL_RULE_ACTION_ALERT.to_string(),
                        rule_group_id: "rslvr-frg-7".to_string(),
                        domain_list_id: "rslvr-fdl-7".to_string(),
                    },
                },
                rule(
                    (100, 2, 8),
                    &["blocked.example.com"],
                    None,
                    FirewallDecision::BlockNxDomain,
                ),
            ],
        };
        // (case, the name asked for, the action a record of the query names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a name a matched ALERT rule permits",
                "alerted.example.com",
                Some(FIREWALL_RULE_ACTION_ALERT),
            ),
            (
                "a name a matched BLOCK rule covers",
                "blocked.example.com",
                Some(FIREWALL_RULE_ACTION_BLOCK),
            ),
            ("a name no rule covers", "other.example.com", None),
        ];
        for (case, name, expected) in rows {
            let matched = view.matched(name, Some(TYPE_A));
            assert_eq!(
                matched.as_ref().map(|rule| rule.action.as_str()),
                *expected,
                "case {case:?}: expected the action {expected:?}, got {matched:?}"
            );
            if let Some(matched) = matched {
                assert!(
                    matched.rule_group_id.starts_with("rslvr-frg-")
                        && matched.domain_list_id.starts_with("rslvr-fdl-"),
                    "case {case:?}: expected the rule's own group and list, got {matched:?}"
                );
            }
        }
    }

    /// RULE: the first rule whose domain list holds the name decides, in priority order —
    /// the association's priority first, then the rule's own.
    #[test]
    fn the_lowest_priority_matching_rule_decides() {
        let view = FirewallView {
            rules: vec![
                rule(
                    (100, 1, 0),
                    &["blocked.example.com"],
                    None,
                    FirewallDecision::BlockNxDomain,
                ),
                rule(
                    (101, 1, 0),
                    &["blocked.example.com", "listed.example.net"],
                    None,
                    FirewallDecision::Allow,
                ),
            ],
        };
        let rows: &[(&str, &str, Option<FirewallDecision>)] = &[
            (
                "a name the lowest-priority group blocks",
                "blocked.example.com",
                Some(FirewallDecision::BlockNxDomain),
            ),
            (
                "a name only the later group lists",
                "listed.example.net",
                Some(FirewallDecision::Allow),
            ),
            ("a name no list holds", "control.example.org", None),
        ];
        for (case, name, expected) in rows {
            let decided = view.decision(name, Some(TYPE_A));
            assert_eq!(
                &decided, expected,
                "case {case:?}: expected {expected:?} for {name}, got {decided:?}"
            );
        }
    }

    /// RULE: an entry is matched whole — a plain domain covers itself alone, and the wildcard
    /// form covers the domain that follows it and every name under it. A rule naming a `Qtype`
    /// decides only queries of that type.
    #[test]
    fn an_entry_covers_the_names_its_form_states() {
        let rows: &[(&str, &str, &str, bool)] = &[
            ("the entry itself", "example.com", "example.com", true),
            ("the entry, fully qualified", "example.com", "example.com.", true),
            ("a name under a plain entry", "example.com", "www.example.com", false),
            ("a longer name ending in the entry", "example.com", "notexample.com", false),
            ("the domain of a wildcard entry", "*.example.com", "example.com", true),
            ("a name under a wildcard entry", "*.example.com", "www.example.com", true),
            ("a deeper name under it", "*.example.com", "a.b.example.com", true),
            ("a name only ending in its text", "*.example.com", "notexample.com", false),
        ];
        for (case, entry, asked, expected) in rows {
            let matched = entry_matches(entry, &canonical_domain(asked));
            assert_eq!(
                matched, *expected,
                "case {case:?}: expected {entry} to cover {asked}: {expected}, got {matched}"
            );
        }
    }

    /// RULE: a rule that names a `Qtype` evaluates only queries of that type, so a query of
    /// another type — and one of a type this emulator cannot number — is left alone by it.
    #[test]
    fn a_rule_with_a_qtype_decides_only_that_type() {
        let view = FirewallView {
            rules: vec![rule(
                (100, 1, 0),
                &["blocked.example.com"],
                qtype_number("AAAA"),
                FirewallDecision::BlockNoData,
            )],
        };
        let rows: &[(&str, Option<u16>, Option<FirewallDecision>)] = &[
            ("the type the rule names", Some(28), Some(FirewallDecision::BlockNoData)),
            ("another type", Some(TYPE_A), None),
            ("a type this emulator cannot number", None, None),
        ];
        for (case, qtype, expected) in rows {
            let decided = view.decision("blocked.example.com", *qtype);
            assert_eq!(
                &decided, expected,
                "case {case:?}: expected {expected:?} for type {qtype:?}, got {decided:?}"
            );
        }
    }

    /// RULE: a rule that names NO `Qtype` evaluates every query type, including one this
    /// emulator cannot number.
    #[test]
    fn a_rule_naming_no_qtype_decides_every_type() {
        let view = FirewallView {
            rules: vec![rule(
                (100, 1, 0),
                &["blocked.example.com"],
                None,
                FirewallDecision::BlockNxDomain,
            )],
        };
        let rows: &[(&str, Option<u16>)] = &[
            ("the A type", Some(TYPE_A)),
            ("another type", Some(28)),
            ("a type this emulator cannot number", None),
        ];
        for (case, qtype) in rows {
            let decided = view.decision("blocked.example.com", *qtype);
            assert_eq!(
                decided,
                Some(FirewallDecision::BlockNxDomain),
                "case {case:?}: expected the rule to decide type {qtype:?}, got {decided:?}"
            );
        }
    }

    /// RULE: every `Qtype` spelling `FirewallRule$Qtype` admits reads as the number IANA
    /// assigns it, including the generic `TYPENUMBER` form, and nothing else does.
    #[test]
    fn every_admitted_qtype_spelling_reads_as_its_number() {
        let rows: &[(&str, Option<u16>)] = &[
            ("A", Some(1)),
            ("NS", Some(2)),
            ("CNAME", Some(5)),
            ("SOA", Some(6)),
            ("PTR", Some(12)),
            ("MX", Some(15)),
            ("TXT", Some(16)),
            ("AAAA", Some(28)),
            ("SRV", Some(33)),
            ("NAPTR", Some(35)),
            ("DS", Some(43)),
            ("SPF", Some(99)),
            ("CAA", Some(257)),
            ("aaaa", Some(28)),
            ("TYPE28", Some(28)),
            ("TYPE1", Some(1)),
            ("TYPE65534", Some(65_534)),
            ("TYPE0", None),
            ("TYPE65535", None),
            ("TYPE", None),
            ("MADE-UP", None),
        ];
        for (spelling, expected) in rows {
            let read = qtype_number(spelling);
            assert_eq!(
                &read, expected,
                "case {spelling:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: a `BLOCK` rule is decided by its `BlockResponse`, and a rule that permits the
    /// query — `ALLOW` or `ALERT` — lets it through.
    #[test]
    fn each_action_decides_the_way_the_model_states() {
        /// One row: the case, the action, the block response, the override domain, and what
        /// the rule decides.
        type Row = (
            &'static str,
            &'static str,
            Option<&'static str>,
            Option<&'static str>,
            Option<FirewallDecision>,
        );
        let rows: &[Row] = &[
            ("ALLOW", "ALLOW", None, None, Some(FirewallDecision::Allow)),
            ("ALERT", "ALERT", None, None, Some(FirewallDecision::Allow)),
            (
                "BLOCK with NXDOMAIN",
                "BLOCK",
                Some("NXDOMAIN"),
                None,
                Some(FirewallDecision::BlockNxDomain),
            ),
            (
                "BLOCK with NODATA",
                "BLOCK",
                Some("NODATA"),
                None,
                Some(FirewallDecision::BlockNoData),
            ),
            (
                "BLOCK with OVERRIDE and a record",
                "BLOCK",
                Some("OVERRIDE"),
                Some("safe.example.net"),
                Some(FirewallDecision::BlockOverride {
                    domain: "safe.example.net".to_string(),
                    dns_type: OVERRIDE_DNS_TYPE_CNAME.to_string(),
                    ttl: DEFAULT_OVERRIDE_TTL,
                }),
            ),
            (
                "BLOCK with OVERRIDE and no record to answer with",
                "BLOCK",
                Some("OVERRIDE"),
                None,
                None,
            ),
        ];
        for (case, action, response, domain, expected) in rows {
            let read = decision_of(&crate::ops::types::FirewallRule {
                action: Some((*action).to_string()),
                block_response: response.map(|response| response.to_string()),
                block_override_domain: domain.map(|domain| domain.to_string()),
                ..Default::default()
            });
            assert_eq!(
                &read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: a Profile's `ResourceProperties` states the priority of the rule group it carries,
    /// and a document that states none leaves the group at the highest priority the service
    /// accepts.
    #[test]
    fn a_profiles_properties_state_the_priority_of_its_rule_group() {
        let rows: &[(&str, Option<&str>, i64)] = &[
            ("a document naming a priority", Some(r#"{"priority": 150}"#), 150),
            ("a document naming none", Some("{}"), profiles::MAX_FIREWALL_PRIORITY),
            ("no document at all", None, profiles::MAX_FIREWALL_PRIORITY),
            ("a document that is no JSON", Some("priority=150"), profiles::MAX_FIREWALL_PRIORITY),
        ];
        for (case, document, expected) in rows {
            let read = firewall_priority_of(*document);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected priority {expected}, got {read}"
            );
        }
    }

    /// RULE: a Profile holds a rule group by ARN, and the id the firewall reads off it is that
    /// ARN's own resource segment.
    #[test]
    fn the_rule_group_of_an_arn_is_its_own_resource_segment() {
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a rule group ARN",
                "arn:aws:route53resolver:us-east-1:123456789012:firewall-rule-group/rslvr-frg-1",
                Some("rslvr-frg-1"),
            ),
            (
                "an ARN of another resource of the same service",
                "arn:aws:route53resolver:us-east-1:123456789012:resolver-rule/rslvr-rr-1",
                None,
            ),
            ("text that is no ARN", "rslvr-frg-1", None),
            (
                "a rule group ARN with no id",
                "arn:aws:route53resolver:us-east-1:123456789012:firewall-rule-group/",
                None,
            ),
        ];
        for (case, arn, expected) in rows {
            let read = rule_group_id_of_arn(arn);
            assert_eq!(
                read.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?} from {arn}, got {read:?}"
            );
        }
    }
}
