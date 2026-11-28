// SPDX-License-Identifier: CC0-1.0

//! The domains an AutoAllowlist DNS Firewall rule has on record, and the managed
//! domain list DNS Firewall creates and maintains for such a rule.
//!
//! An AutoAllowlist rule "observes the DNS traffic of the VPCs that use the rule
//! group … The rule allows the domains on its managed domain list", and the
//! review status of one domain is decided by that list and by the denials a
//! caller recorded — never held twice, so the list and the statuses reported for
//! it cannot disagree.

use super::*;

/// The `DomainListType` of the one domain list DNS Firewall creates for an
/// AutoAllowlist rule, from the model's `DomainListType` enum (re-exported as
/// [`crate::ops::types::DOMAIN_LIST_TYPE_VALUES`]).
pub const DOMAIN_LIST_TYPE_AUTOALLOWLIST: &str = "AUTOALLOWLIST";

/// The `FirewallDomainStatus` of a domain DNS Firewall has on record for the
/// rule and the caller has neither allowed nor denied: "`OBSERVED` — DNS
/// Firewall saw the domain in the VPCs' DNS traffic and you haven't allowed or
/// denied it … a domain becomes observed … when you remove it from the managed
/// domain list with `REMOVE`."
pub const FIREWALL_DOMAIN_STATUS_OBSERVED: &str = "OBSERVED";

/// The `FirewallDomainStatus` of a domain the rule allows: "`ALLOWED` — DNS
/// Firewall allowed queries for the domain", which is the domain being on the
/// rule's managed domain list.
pub const FIREWALL_DOMAIN_STATUS_ALLOWED: &str = "ALLOWED";

/// The `FirewallDomainStatus` of a domain a caller denied: "`DENIED` — The
/// domain was denied with the `DENY` operation of `UpdateFirewallDomains`, which
/// also removes it from the managed domain list if it was there."
pub const FIREWALL_DOMAIN_STATUS_DENIED: &str = "DENIED";

/// The `FirewallRuleDomainSortBy` that orders the domains by "the `AddedTime` of
/// the domain", which is also the one applied when a request names none: "if you
/// don't specify a value, the operation sorts by `TIME_ADDED`".
pub const FIREWALL_RULE_DOMAIN_SORT_BY_TIME_ADDED: &str = "TIME_ADDED";

/// The `FirewallRuleDomainSortBy` that orders by "the `Last24HourRequests`
/// count".
pub const FIREWALL_RULE_DOMAIN_SORT_BY_DAILY_TOTAL: &str = "DAILY_TOTAL";

/// The `FirewallRuleDomainSortBy` that orders by "the `TotalRequests` count".
pub const FIREWALL_RULE_DOMAIN_SORT_BY_TOTAL_REQUESTS: &str = "TOTAL_REQUESTS";

/// The ascending `FirewallRuleDomainSortOrder`.
pub const FIREWALL_RULE_DOMAIN_SORT_ORDER_ASC: &str = "ASC";

/// The descending `FirewallRuleDomainSortOrder`, which is also the one applied
/// when a request names none: "if you don't specify a value, the operation sorts
/// in descending order".
pub const FIREWALL_RULE_DOMAIN_SORT_ORDER_DESC: &str = "DESC";

/// The word the service uses for what a `ListFirewallRuleDomains` request asks
/// of the rule it names, inside the sentence a request that names no single rule
/// is refused with ([`firewall_rule_match_source_not_single`]) — the same shape
/// as the words the writes of a rule are refused with
/// ([`FIREWALL_RULE_ACTION_WORD_UPDATE`]).
pub const FIREWALL_RULE_ACTION_WORD_LIST_DOMAINS: &str = "list the domains of";

/// The name DNS Firewall gives the managed domain list of an AutoAllowlist rule:
/// "the list is named `AutoAllowlist - ` followed by the rule name, cut to 64
/// characters, and is renamed when you rename the rule".
///
/// The cut is to 64 characters because that is what `Name`'s modelled length
/// allows (0..64, `smithy_query(query="shape",shape="Name")`), and it is taken
/// on a character boundary so the name stays a string.
pub fn auto_allowlist_list_name(rule_name: &str) -> String {
    let full = format!("{AUTO_ALLOWLIST_LIST_NAME_PREFIX}{rule_name}");
    match full.char_indices().nth(FIREWALL_NAME_MAX_CHARS) {
        Some((cut, _)) => full[..cut].to_string(),
        None => full,
    }
}

/// The prefix of that name, as the member's doc writes it.
pub const AUTO_ALLOWLIST_LIST_NAME_PREFIX: &str = "AutoAllowlist - ";

/// The longest `Name` the model admits (length 0..64), which the generated name
/// of a managed list is cut to.
pub const FIREWALL_NAME_MAX_CHARS: usize = 64;

/// The id of the managed domain list an AutoAllowlist rule owns, and `None` for
/// every other rule — which is how a rule is recognised as one DNS Firewall
/// maintains a list for, wherever that question is asked.
pub fn auto_allowlist_list_id(rule: &FirewallRule) -> Option<&str> {
    rule.firewall_rule_type
        .as_ref()?
        .auto_allowlist
        .as_ref()?
        .auto_allowlist_firewall_domain_list_id
        .as_deref()
}

/// One domain DNS Firewall has on record for an AutoAllowlist rule.
///
/// The REVIEW STATUS is not held here: it follows from the rule's managed domain
/// list holding the domain or not, and from the denial recorded below, so a
/// status and the list it is read off cannot disagree
/// ([`FirewallDomainListRecord::auto_allowlist_domain_status`]).
#[derive(Debug, Clone, Default)]
pub struct AutoAllowlistDomainRecord {
    /// The domain, in the one canonical spelling every DNS Firewall resource
    /// holds a name in ([`firewall_domain_canonical`]).
    pub domain: String,
    /// `FirewallRuleDomain$AddedTime`: "the date and time when the domain was
    /// first observed and added", so a later change to the same domain leaves it
    /// alone.
    pub added_time: String,
    /// Whether a caller denied this domain with the `DENY` operation of
    /// `UpdateFirewallDomains`, which is what makes its status `DENIED` until a
    /// later `ADD` allows it again.
    pub denied: bool,
}

/// What the managed domain list of an AutoAllowlist rule holds beyond the
/// domains every domain list holds: the rule it belongs to, and the domains DNS
/// Firewall has on record for that rule.
#[derive(Debug, Clone, Default)]
pub struct AutoAllowlistFacts {
    /// `FirewallDomainListMetadata$FirewallRuleGroupId`: "the ID of the firewall
    /// rule group that contains the auto-allowlist rule that owns this domain
    /// list".
    pub firewall_rule_group_id: String,
    /// `FirewallDomainListMetadata$FirewallThreatProtectionId`: "the threat
    /// protection ID of the auto-allowlist rule that owns this domain list",
    /// which is also what `ListFirewallRuleDomains` identifies the rule by.
    pub firewall_threat_protection_id: String,
    /// Every domain DNS Firewall has on record for the rule, in the order it
    /// first recorded them — which is the order a page is cut out of and the
    /// tiebreaker of every sort.
    pub domains: Vec<AutoAllowlistDomainRecord>,
}

impl FirewallDomainListRecord {
    /// This list's `ManagedListType`, which only the list DNS Firewall maintains
    /// for an AutoAllowlist rule of this account carries
    /// ([`DOMAIN_LIST_TYPE_AUTOALLOWLIST`]); a list the account created itself is
    /// not a managed list of any type.
    pub fn managed_list_type(&self) -> Option<&'static str> {
        self.auto_allowlist
            .as_ref()
            .map(|_| DOMAIN_LIST_TYPE_AUTOALLOWLIST)
    }

    /// The domains this list holds, as a set.
    ///
    /// The review status of every domain on record is decided against the list
    /// ([`auto_allowlist_domain_status`]), and deciding each one by scanning the
    /// list itself would cost the PRODUCT of the two — both bounded by
    /// [`MAX_DOMAINS_PER_LIST`] — so the readers take the set once.
    pub fn allowed_domains(&self) -> HashSet<&str> {
        self.domains.iter().map(String::as_str).collect()
    }

    /// How many domains would stand on record for this list's rule once `domains`
    /// are written to it under `review`: the ones already on record, plus — for a
    /// write that ALLOWS them, the one review that puts a domain on record — the
    /// ones it has nothing on record for.
    ///
    /// The record is BOUNDED, by the same [`MAX_DOMAINS_PER_LIST`] that bounds
    /// the domains of a list, and the bound on the list does not bound the
    /// record: a `REMOVE` shrinks the list and leaves the domain on record, so a
    /// caller alternating `ADD` and `REMOVE` over fresh domains would grow it
    /// without limit. The writes ask this BEFORE they write, whatever the review,
    /// and refuse past the bound.
    pub fn auto_allowlist_record_size_after(
        &self,
        domains: &[String],
        review: AutoAllowlistReview,
    ) -> usize {
        let Some(facts) = self.auto_allowlist.as_ref() else {
            return 0;
        };
        if review == AutoAllowlistReview::Denied {
            // A denial puts no domain on record ([`AutoAllowlistReview::Denied`]
            // says why), so the record stands exactly as large as it is.
            return facts.domains.len();
        }
        let on_record: HashSet<&str> = facts
            .domains
            .iter()
            .map(|record| record.domain.as_str())
            .collect();
        facts.domains.len()
            + domains
                .iter()
                .filter(|domain| !on_record.contains(domain.as_str()))
                .count()
    }

    /// Put `domains` on record for this list's rule, as of `timestamp`, under the
    /// review `review` states.
    ///
    /// A domain already on record keeps its `AddedTime` — "the date and time when
    /// the domain was FIRST observed and added" — and only the review moves, so
    /// allowing a denied domain again records no second entry for it. A domain
    /// the rule has nothing on record for goes on record only when the rule is
    /// asked to ALLOW it ([`AutoAllowlistReview::Denied`] says why), and the
    /// caller bounds the record with [`Self::auto_allowlist_record_size_after`]
    /// before it writes.
    pub fn record_auto_allowlist_domains(
        &mut self,
        domains: &[String],
        review: AutoAllowlistReview,
        timestamp: &str,
    ) {
        let Some(facts) = self.auto_allowlist.as_mut() else {
            return;
        };
        let denied = review == AutoAllowlistReview::Denied;
        // One pass over the domains already on record and one over the domains
        // the request names, so recording them costs their SUM and not their
        // product — a request carries up to [`MAX_DOMAINS_PER_UPDATE`] domains
        // and a rule's record holds up to [`MAX_DOMAINS_PER_LIST`].
        let mut outstanding: HashSet<&str> = domains.iter().map(String::as_str).collect();
        for record in facts.domains.iter_mut() {
            if outstanding.remove(record.domain.as_str()) {
                record.denied = denied;
            }
        }
        if review == AutoAllowlistReview::Denied {
            return;
        }
        for domain in domains {
            if outstanding.remove(domain.as_str()) {
                facts.domains.push(AutoAllowlistDomainRecord {
                    domain: domain.clone(),
                    added_time: timestamp.to_string(),
                    denied,
                });
            }
        }
    }
}

/// What a write of an AutoAllowlist rule's managed domain list puts on record
/// about the domains it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAllowlistReview {
    /// The rule allows them: "`ADD` — for an AutoAllowlist rule's managed domain
    /// list, this also marks the observed domains as allowed". A domain the rule
    /// has nothing on record for goes on record here, which is the ONE place the
    /// record grows.
    Allowed,
    /// A caller denied them: "`DENY` — … mark the specified OBSERVED domains as
    /// denied". A domain the rule has nothing on record for is not one of them,
    /// so a denial moves the review of what is on record and puts nothing new
    /// there.
    Denied,
}

/// The `FirewallDomainStatus` a rule's managed domain list reports for one domain
/// it has on record: the denial a caller recorded, else the list holding the
/// domain (`allowed`, from [`FirewallDomainListRecord::allowed_domains`]), else
/// the domain standing observed and unreviewed.
pub fn auto_allowlist_domain_status(
    record: &AutoAllowlistDomainRecord,
    allowed: &HashSet<&str>,
) -> &'static str {
    if record.denied {
        return FIREWALL_DOMAIN_STATUS_DENIED;
    }
    if allowed.contains(record.domain.as_str()) {
        return FIREWALL_DOMAIN_STATUS_ALLOWED;
    }
    FIREWALL_DOMAIN_STATUS_OBSERVED
}

/// The rule the managed domain list is created for, as
/// [`ComAmazonawsRoute53resolverRoute53resolverState::create_auto_allowlist_domain_list`]
/// takes it.
#[derive(Debug, Clone, Copy)]
pub struct AutoAllowlistListCreate<'a> {
    pub firewall_rule_group_id: &'a str,
    pub firewall_threat_protection_id: &'a str,
    /// The rule's `Name`, which the managed list's own name is built from
    /// ([`auto_allowlist_list_name`]).
    pub rule_name: &'a str,
    /// `AutoAllowlistConfig$ImportFromFirewallDomainListId`: "the ID of a domain
    /// list whose domains DNS Firewall copies into the AutoAllowlist rule's
    /// managed domain list after you create the rule", or `None` for a rule that
    /// names no source list.
    pub import_from_firewall_domain_list_id: Option<&'a str>,
    /// The region the request was signed for, which the list's ARN is rooted in.
    pub region: &'a str,
    /// The trace of the request that created the rule, which the refusals naming
    /// a resource it cannot address close with ([`traced_message`]).
    pub trace_root: &'a str,
}

impl ComAmazonawsRoute53resolverRoute53resolverState {
    /// Create the managed domain list DNS Firewall maintains for one
    /// AutoAllowlist rule, and answer its `Id`.
    ///
    /// The source list a rule names is copied into it: "you must own the source
    /// list, and it must be in `COMPLETE` status … and not a managed or
    /// AutoAllowlist list", and while the copy runs the managed list stands in
    /// `IMPORTING` and settles in `COMPLETE` — the same shape every clock-owed
    /// transition of this service has. A refusal writes nothing.
    pub fn create_auto_allowlist_domain_list(
        &mut self,
        request: AutoAllowlistListCreate<'_>,
        now: Instant,
    ) -> Result<String, FirewallRuleCreateRefusal> {
        // The domains the copy puts on the managed list, from the source list the
        // rule names: a list of this account, settled, and not itself a managed
        // or AutoAllowlist list.
        let imported: Vec<String> = match request.import_from_firewall_domain_list_id {
            Some(source_id) => {
                let Some(source) = self.firewall_domain_lists.get(source_id) else {
                    // A list DNS Firewall publishes itself is not a list this
                    // account owns, so naming one as the source is refused
                    // exactly as naming a list that does not exist is.
                    return Err(FirewallRuleCreateRefusal::ResourceNotFound(
                        firewall_domain_list_not_found(source_id, request.trace_root),
                    ));
                };
                if source.auto_allowlist.is_some() {
                    return Err(FirewallRuleCreateRefusal::Validation(ValidationException {
                        message: Some(format!(
                            "ImportFromFirewallDomainListId \"{source_id}\" is the managed domain \
                             list of an AutoAllowlist rule, which cannot be copied into another"
                        )),
                    }));
                }
                let status = source.status_at(now);
                if status != FIREWALL_DOMAIN_LIST_STATUS_COMPLETE {
                    return Err(FirewallRuleCreateRefusal::Validation(ValidationException {
                        message: Some(format!(
                            "ImportFromFirewallDomainListId \"{source_id}\" is in status \
                             {status}; the source domain list must be in status \
                             {FIREWALL_DOMAIN_LIST_STATUS_COMPLETE}"
                        )),
                    }));
                }
                source.domains.clone()
            }
            None => Vec::new(),
        };

        // The managed list is a record of this store like any other domain list,
        // so it is bounded the same way and refused the same way past the bound.
        if !self.can_store_another_firewall_domain_list() {
            return Err(FirewallRuleCreateRefusal::LimitExceeded(
                LimitExceededException {
                    message: Some(format!(
                        "This emulator stores at most {MAX_STORED_FIREWALL_DOMAIN_LISTS} firewall \
                         domain lists; delete one before creating another"
                    )),
                    resource_type: Some(RESOURCE_TYPE_FIREWALL_DOMAIN_LIST.to_string()),
                },
            ));
        }

        let seq = self.next_seq();
        let id = mint_firewall_domain_list_id(seq);
        let arn = ::engine::emulator_arn(
            ARN_NAMESPACE,
            request.region,
            &format!("firewall-domain-list/{id}"),
        );
        let timestamp = now_rfc3339();
        // "DNS Firewall reads the source list while the copy runs": the copy is
        // what the `IMPORTING` step reports, and it settles in `COMPLETE`. A rule
        // that names no source list copies nothing, so its list is settled the
        // moment it exists.
        let importing = request.import_from_firewall_domain_list_id.is_some();
        let mut record = FirewallDomainListRecord {
            seq,
            id: id.clone(),
            arn,
            name: auto_allowlist_list_name(request.rule_name),
            // The list is DNS Firewall's own, created for the rule rather than by
            // a request of the account, so what identifies the request that made
            // it is the rule it belongs to.
            creator_request_id: request.firewall_threat_protection_id.to_string(),
            domains: imported.clone(),
            status: if importing {
                FIREWALL_DOMAIN_LIST_STATUS_IMPORTING.to_string()
            } else {
                FIREWALL_DOMAIN_LIST_STATUS_COMPLETE.to_string()
            },
            tags: Vec::new(),
            creation_time: timestamp.clone(),
            modification_time: timestamp.clone(),
            settled_at: importing.then(|| now + PROVISIONING_DELAY),
            settled_status: importing.then(|| FIREWALL_DOMAIN_LIST_STATUS_COMPLETE.to_string()),
            settled_status_message: None,
            change: FirewallChange::Created,
            auto_allowlist: Some(AutoAllowlistFacts {
                firewall_rule_group_id: request.firewall_rule_group_id.to_string(),
                firewall_threat_protection_id: request.firewall_threat_protection_id.to_string(),
                domains: Vec::new(),
            }),
        };
        // Every domain the copy put on the list is a domain the rule allows, so
        // it goes on record with the list that holds it — the one write that
        // keeps the statuses `ListFirewallRuleDomains` reports derivable from the
        // list itself. The record starts empty and the source list holds at most
        // [`MAX_DOMAINS_PER_LIST`] domains, so the copy cannot carry it past the
        // bound the later writes check
        // ([`FirewallDomainListRecord::auto_allowlist_record_size_after`]).
        record.record_auto_allowlist_domains(&imported, AutoAllowlistReview::Allowed, &timestamp);
        self.firewall_domain_lists.insert(id.clone(), record);
        Ok(id)
    }

    /// Rename the managed domain list of an AutoAllowlist rule, which is "renamed
    /// when you rename the rule".
    pub fn rename_auto_allowlist_domain_list(&mut self, id: &str, rule_name: &str) {
        if let Some(record) = self.firewall_domain_lists.get_mut(id) {
            if record.auto_allowlist.is_some() {
                record.name = auto_allowlist_list_name(rule_name);
                record.modification_time = now_rfc3339();
                record.change = FirewallChange::Changed;
            }
        }
    }

    /// Take the managed domain list of an AutoAllowlist rule out of the store,
    /// which is what deleting the rule that owns it does: the list exists for the
    /// rule, so nothing of it outlives the rule.
    pub fn remove_auto_allowlist_domain_list(&mut self, id: &str) {
        if self
            .firewall_domain_lists
            .get(id)
            .is_some_and(|record| record.auto_allowlist.is_some())
        {
            self.firewall_domain_lists.remove(id);
        }
    }

    /// The domains of the AutoAllowlist rule a `ListFirewallRuleDomains` request
    /// names, filtered and sorted as the request's `QueryOptions` say, in the
    /// order the response reports them.
    ///
    /// A pure read: it answers from the managed domain list and the domains on
    /// record for the rule, and writes nothing.
    pub fn firewall_rule_domains(
        &self,
        query: &FirewallRuleDomainsQuery<'_>,
    ) -> Result<Vec<FirewallRuleDomain>, FirewallRuleDomainsFault> {
        // Every value the request states for a `QueryOptions` member has to be
        // one its modelled enum declares; a value outside one is refused rather
        // than read as if the service understood it.
        for (member, value, values) in [
            (
                "QueryOptions.DomainStatus",
                query.domain_status,
                crate::ops::types::FIREWALL_DOMAIN_STATUS_FILTER_VALUES,
            ),
            (
                "QueryOptions.SortBy",
                query.sort_by,
                crate::ops::types::FIREWALL_RULE_DOMAIN_SORT_BY_VALUES,
            ),
            (
                "QueryOptions.SortOrder",
                query.sort_order,
                crate::ops::types::FIREWALL_RULE_DOMAIN_SORT_ORDER_VALUES,
            ),
        ] {
            if let Some(value) = value {
                if !values.contains(&value) {
                    return Err(FirewallRuleDomainsFault::Validation(ValidationException {
                        message: Some(format!("{member} must be one of {values:?}")),
                    }));
                }
            }
        }

        // "The unique identifier of the firewall rule group that contains the
        // rule": a group the store does not hold contains no rule at all.
        if !self
            .firewall_rule_groups
            .contains_key(query.firewall_rule_group_id)
        {
            return Err(FirewallRuleDomainsFault::ResourceNotFound(
                firewall_rule_group_not_found(query.firewall_rule_group_id, query.trace_root),
            ));
        }

        // The rule is addressed the way every operation that addresses one
        // addresses it: by exactly one of the two members, and a request that
        // carries both or neither names no single rule.
        let Some((member, named_id)) = (FirewallRuleMatchSource {
            firewall_rule_group_id: query.firewall_rule_group_id,
            firewall_domain_list_id: query.firewall_domain_list_id,
            firewall_threat_protection_id: query.firewall_threat_protection_id,
            qtype: None,
            action: FIREWALL_RULE_ACTION_WORD_LIST_DOMAINS,
            trace_root: query.trace_root,
        })
        .match_source() else {
            return Err(FirewallRuleDomainsFault::Validation(
                firewall_rule_match_source_not_single(FIREWALL_RULE_ACTION_WORD_LIST_DOMAINS),
            ));
        };

        // The rule's managed domain list, found by what the request named it by:
        // the list's own id, or the threat protection id of the rule that owns
        // it. Either way the list has to belong to a rule of the group the
        // request names, so an id of another group's rule names no rule here.
        let found = self.firewall_domain_lists.values().find_map(|record| {
            let facts = record.auto_allowlist.as_ref()?;
            (facts.firewall_rule_group_id == query.firewall_rule_group_id
                && match query.firewall_domain_list_id {
                    Some(_) => record.id == named_id,
                    None => facts.firewall_threat_protection_id == named_id,
                })
            .then_some((record, facts))
        });
        let Some((list, facts)) = found else {
            // What the request named is a RULE — "only the owner of the rule
            // group can call this operation" and the domains belong to that rule
            // — so an id that answers to no AutoAllowlist rule of the group is
            // reported exactly as every other operation reports a rule the group
            // does not hold.
            return Err(FirewallRuleDomainsFault::ResourceNotFound(
                firewall_rule_not_found(
                    query.firewall_rule_group_id,
                    member,
                    named_id,
                    query.trace_root,
                ),
            ));
        };

        // The domains the list holds, taken ONCE: every review status below is
        // decided against them ([`auto_allowlist_domain_status`]).
        let allowed = list.allowed_domains();

        // What the request selected: naming the managed domain list asks for "the
        // domains that the rule currently allows", so the candidates are the
        // domains the list holds; naming the rule asks for the domains DNS
        // Firewall observed for it, which is every domain it has on record.
        let by_list = query.firewall_domain_list_id.is_some();
        let candidates: Vec<(usize, &AutoAllowlistDomainRecord)> = facts
            .domains
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                !by_list
                    || auto_allowlist_domain_status(record, &allowed)
                        == FIREWALL_DOMAIN_STATUS_ALLOWED
            })
            .collect();

        // "Filters the returned domains by review status … if you don't specify a
        // value, the operation returns OBSERVED domains." The default belongs to
        // the review of what the rule observed, which is the set the rule's own
        // id selects; the managed list selects the domains it holds, each of
        // which the rule allows, so only a status the request STATES narrows it.
        let status_filter = match (query.domain_status, by_list) {
            (Some(stated), _) => Some(stated),
            (None, false) => Some(FIREWALL_DOMAIN_STATUS_OBSERVED),
            (None, true) => None,
        };
        // "Returns only the domains whose name contains this substring": the
        // stored domain is canonical, so the substring is read the same way — two
        // spellings of one name are one name here too.
        let name_filter = query.domain_name_filter.map(firewall_domain_substring);

        let mut domains: Vec<(usize, FirewallRuleDomain)> = candidates
            .into_iter()
            .filter(|(_, record)| {
                status_filter
                    .is_none_or(|wanted| auto_allowlist_domain_status(record, &allowed) == wanted)
            })
            .filter(|(_, record)| {
                name_filter
                    .as_deref()
                    .is_none_or(|needle| record.domain.contains(needle))
            })
            .map(|(order, record)| {
                (
                    order,
                    FirewallRuleDomain {
                        domain_name: Some(record.domain.clone()),
                        domain_status: Some(
                            auto_allowlist_domain_status(record, &allowed).to_string(),
                        ),
                        // The counts are "the number of DNS requests observed for
                        // this domain": this emulator keeps no per-domain count of
                        // the queries its DNS data plane answered, so neither
                        // count is invented for a domain — the same reading every
                        // member this store does not know takes.
                        last24_hour_requests: None,
                        total_requests: None,
                        added_time: Some(record.added_time.clone()),
                    },
                )
            })
            .collect();

        // "The attribute to sort the returned domains by … if you don't specify a
        // value, the operation sorts by TIME_ADDED", "in descending order". Where
        // two domains carry the same key, the order they were recorded in decides
        // — so a descending sort reports the one recorded later first, and every
        // sort is a total order a page can be cut out of.
        let descending = query.sort_order != Some(FIREWALL_RULE_DOMAIN_SORT_ORDER_ASC);
        match query.sort_by {
            Some(FIREWALL_RULE_DOMAIN_SORT_BY_DAILY_TOTAL) => sort_domains(
                &mut domains,
                descending,
                |(order, domain)| (domain.last24_hour_requests.unwrap_or_default(), *order),
            ),
            Some(FIREWALL_RULE_DOMAIN_SORT_BY_TOTAL_REQUESTS) => sort_domains(
                &mut domains,
                descending,
                |(order, domain)| (domain.total_requests.unwrap_or_default(), *order),
            ),
            _ => sort_domains(&mut domains, descending, |(order, domain)| {
                (domain.added_time.clone(), *order)
            }),
        }
        Ok(domains.into_iter().map(|(_, domain)| domain).collect())
    }
}

/// Sort `domains` by `key`, ascending or descending, keeping the order of two
/// items whose keys compare equal — which the tiebreaker in every key makes
/// impossible, so one query always answers one order.
fn sort_domains<K: Ord>(
    domains: &mut [(usize, FirewallRuleDomain)],
    descending: bool,
    key: impl Fn(&(usize, FirewallRuleDomain)) -> K,
) {
    if descending {
        domains.sort_by_key(|left| ::std::cmp::Reverse(key(left)));
    } else {
        domains.sort_by_key(|left| key(left));
    }
}

/// The substring a `DomainNameFilter` names, read the way the stored domains are
/// spelled: case folded, since DNS labels are compared without regard to case,
/// and without the root dot a fully qualified filter may carry — so a filter
/// matches the names a caller sees whichever way it spells them.
pub fn firewall_domain_substring(filter: &str) -> String {
    let folded = filter.trim().to_ascii_lowercase();
    match folded.strip_suffix('.') {
        Some(unrooted) if !unrooted.is_empty() => unrooted.to_string(),
        _ => folded,
    }
}

/// One `ListFirewallRuleDomains` request, as the store reads it.
#[derive(Debug, Clone, Copy)]
pub struct FirewallRuleDomainsQuery<'a> {
    pub firewall_rule_group_id: &'a str,
    pub firewall_domain_list_id: Option<&'a str>,
    pub firewall_threat_protection_id: Option<&'a str>,
    pub domain_status: Option<&'a str>,
    pub sort_by: Option<&'a str>,
    pub sort_order: Option<&'a str>,
    pub domain_name_filter: Option<&'a str>,
    /// The trace of the request, as for [`FirewallRuleCreate::trace_root`].
    pub trace_root: &'a str,
}

/// Why one `ListFirewallRuleDomains` request names no domains to report.
#[derive(Debug, Clone)]
pub enum FirewallRuleDomainsFault {
    Validation(ValidationException),
    ResourceNotFound(ResourceNotFoundException),
}

/// The cursor scope of one `ListFirewallRuleDomains` query.
///
/// The collection a page is cut out of is the one the request's rule and
/// `QueryOptions` select, so a cursor minted for one query is not a cursor
/// another query accepts — the same reasoning as
/// [`firewall_rule_group_association_list_scope`].
pub fn firewall_rule_domains_list_scope(query: &FirewallRuleDomainsQuery<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListFirewallRuleDomains/rule-and-query-options/v1");
    feed_field(&mut hasher, query.firewall_rule_group_id.as_bytes());
    for stated in [
        query.firewall_domain_list_id,
        query.firewall_threat_protection_id,
        query.domain_status,
        query.sort_by,
        query.sort_order,
        query.domain_name_filter,
    ]
    .into_iter()
    .flatten()
    {
        feed_field(&mut hasher, stated.as_bytes());
    }
    format!(
        "ListFirewallRuleDomains/{}",
        hex::encode(&hasher.finalize()[..8])
    )
}
