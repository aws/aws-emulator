// SPDX-License-Identifier: CC0-1.0

//! The DISTRIBUTION record: its invalidations, its edge cache, and the transitions the
//! clock owes it.
//!
//! One concern, because the three move together: an invalidation finishing is what takes
//! the objects it names out of the edge cache of the distribution it was created under, and
//! both are materialised by the one [`StoredDistribution::settle`] every handler applies
//! before it reads. The path matcher the cache behaviours and the invalidation batches are
//! both read with lives here too, since it is what decides which cached entries a batch
//! names.
//!
//! Every bound the work in this module can be asked for is a constant declared in it:
//! [`MAX_EDGE_CACHE_ENTRIES`] caps what one distribution's cache holds,
//! [`MAX_INVALIDATION_MATCHES`] caps the comparisons one finished batch makes against it —
//! and the comparisons ONE settlement's whole sweep makes, however many batches the clock
//! finishes at the same instant — and [`MAX_GLOB_COMPARISONS`] caps one
//! pattern-against-path match.

use super::*;

/// Whether `path` is one of the paths `pattern` names, with `*` standing for any run of
/// characters and `?` for exactly one.
///
/// ONE matcher for the two places CloudFront matches a path against a pattern: a cache
/// behaviour's `CacheBehavior$PathPattern` ("The pattern that specifies which requests to
/// apply the behavior to", whose wildcards the member documents as `*` and `?`) and an
/// invalidation's `Paths$Items` ("The path of the object to invalidate", which may end in
/// `*`). A pattern that does not begin at the root is matched against the path without its
/// leading `/` as well, because a path pattern may be written with or without one.
pub fn matches_path_pattern(pattern: &str, path: &str) -> bool {
    matches_glob(pattern, path) || matches_glob(pattern, path.trim_start_matches('/'))
}

/// The most character comparisons one wildcard match makes before it reports no match.
///
/// A single-backtrack glob walk is O(pattern x subject), and both come from caller-stored
/// or caller-sent values, so the work one comparison can ask for is capped here. The cap
/// is far above any real path and pattern (a `PathPattern` is at most 255 characters and a
/// request path far less than this bound), so no real match reaches it; one that does is
/// reported as NO match, which for a cache behaviour means the next behaviour (and in the
/// end the default one) decides the request, and for an invalidation means the object is
/// left in the cache rather than removed on the strength of a comparison that was cut
/// short.
pub(super) const MAX_GLOB_COMPARISONS: usize = 1 << 16;

/// The wildcard match itself, over characters: the standard linear walk with one
/// backtrack point per `*`.
///
/// Public because CloudFront matches a caller-written pattern against a subject in one more
/// place than a path: a signed URL's policy `Resource` names the URLs it was signed for with
/// the same `*` and `?` wildcards (`crate::service::edge::signed_access`).
pub fn matches_glob(pattern: &str, subject: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let subject: Vec<char> = subject.chars().collect();
    let (mut at_pattern, mut at_subject) = (0usize, 0usize);
    // Where the last `*` was, and how much of the subject it has consumed so far.
    let mut star: Option<usize> = None;
    let mut consumed = 0usize;
    let mut comparisons = 0usize;
    while at_subject < subject.len() {
        comparisons += 1;
        if comparisons > MAX_GLOB_COMPARISONS {
            return false;
        }
        let literal = at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == subject[at_subject]);
        if literal {
            at_pattern += 1;
            at_subject += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some(at_pattern);
            consumed = at_subject;
            at_pattern += 1;
        } else if let Some(wildcard) = star {
            // Give the wildcard one more character and try the rest of the pattern again.
            at_pattern = wildcard + 1;
            consumed += 1;
            at_subject = consumed;
        } else {
            return false;
        }
    }
    while at_pattern < pattern.len() && pattern[at_pattern] == '*' {
        at_pattern += 1;
    }
    at_pattern == pattern.len()
}

/// One response a distribution's edge holds in its cache.
///
/// The ORIGIN'S OWN answer: the status, the bytes, and the two things the origin said about
/// those bytes — their media type and the content coding they are already under — beside the
/// instants that decide whether the entry may still be served. What the edge serves FROM it
/// (the content coding one viewer negotiated) is computed per request rather than held, so
/// one viewer's negotiation is never served to another.
#[derive(Debug, Clone)]
pub struct CachedEdgeResponse {
    /// The status the origin answered.
    pub status: u16,
    /// The bytes the origin answered.
    pub body: Vec<u8>,
    /// `Content-Type` as the origin stated it, or `None` when it stated none.
    pub content_type: Option<String>,
    /// `Content-Encoding` as the origin stated it — the coding its own bytes are under — or
    /// `None` for bytes the origin did not encode.
    pub content_encoding: Option<String>,
    /// The path the viewer request carried, so an invalidation can be matched against
    /// this entry when a batch is created.
    pub path: String,
    /// When the origin fetch that filled this entry happened: the order the cache evicts
    /// in when it is full.
    pub stored_at: SystemTime,
    /// When this entry stops being servable, from the cache behaviour's TTL.
    pub expires_at: SystemTime,
}

/// How many cached responses one distribution's edge holds at once.
///
/// A real edge cache is bounded by the storage of an edge location; this emulator holds
/// its cache in memory beside the distribution, so it is bounded by an entry count and
/// the least recently filled entry is dropped to make room. The bound is this emulator's
/// own, not CloudFront's: eviction is invisible to a caller except as a cache MISS, which
/// is a thing CloudFront answers too.
pub const MAX_EDGE_CACHE_ENTRIES: usize = 256;

/// The most path-pattern comparisons one invalidation makes against the edge cache before
/// it empties the cache instead ([`StoredDistribution::invalidate_edge_cache`]).
///
/// This emulator's own bound on the work one call can ask for: the cache holds at most
/// [`MAX_EDGE_CACHE_ENTRIES`] entries and a batch names at most
/// [`MAX_INVALIDATION_PATHS`] paths, so their product is what this caps.
pub const MAX_INVALIDATION_MATCHES: usize = 4096;

/// The CloudFront resource TYPES the tagging surface accepts, as the resource part of an
/// ARN spells each of them.
///
/// Read off the model: a resource type is taggable exactly where the operation that
/// creates it takes tags — `CreateDistributionWithTags` and
/// `CreateStreamingDistributionWithTags` through their `…ConfigWithTags` payload, and
/// `CreateFunction`, `CreateConnectionFunction`, `CreateKeyValueStore`,
/// `CreateAnycastIpList`, `CreateConnectionGroup`, `CreateDistributionTenant`,
/// `CreateTrustStore` and `CreateVpcOrigin` through their `Tags` member
/// (`smithy_query(query="operation",
/// operation="…")`). Every other kind of CloudFront resource — a cache policy, an origin
/// request policy, a response headers policy, an origin access control, a public key, a
/// real-time log configuration, a continuous deployment policy, an origin access identity —
/// is created by an operation that takes no tags at all, so its ARN names no taggable type.
///
/// Each is spelled as the ARN of that resource spells it, which is not always the way its
/// route spells it: a VPC origin's ARN carries [`RESOURCE_POLICY_RESOURCE_TYPE`].
///
/// Sorted, so the list reads as the SET it is.
pub const TAGGABLE_RESOURCE_TYPES: [&str; 10] = [
    "anycast-ip-list",
    "connection-function",
    "connection-group",
    "distribution",
    "distribution-tenant",
    "function",
    "key-value-store",
    "streaming-distribution",
    "trust-store",
    RESOURCE_POLICY_RESOURCE_TYPE,
];

/// The resource TYPE an ARN names — the token its resource part opens with — or `None`
/// where the ARN carries no resource part at all.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and this service's
/// resources spell their resource part `<type>/<id>`, so the type is the token up to the
/// first separator. `ResourceARN`'s pattern is already enforced before any handler runs,
/// so what is read here is the one part of the ARN the pattern leaves open.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let end = resource.find(['/', ':']).unwrap_or(resource.len());
    let kind = &resource[..end];
    (!kind.is_empty()).then_some(kind)
}

/// The modelled `InvalidArgument` (HTTP 400) a tagging call naming a resource type the
/// tagging surface does not accept is refused with, or `None` when the type is one it
/// accepts (so the question becomes whether the store holds that resource).
///
/// What the refusal states is the CONSTRAINT on the parameter — the resource type the
/// submitted ARN names is not one the tagging surface takes — so the one variable part of
/// the sentence is that type, read out of the request's own ARN. It is decided BEFORE any
/// lookup: a type the surface does not accept is a parameter the caller has to fix,
/// whether or not a resource of that type exists, which is what keeps `NoSuchResource` for
/// an accepted type whose resource is absent.
///
/// An ARN carrying no resource part names no type for the sentence to state, so it is left
/// to the existence lookup the accepted types go through.
pub fn untaggable_resource_type(arn: &str) -> Option<crate::ops::types::InvalidArgument> {
    let kind = arn_resource_type(arn)?;
    if TAGGABLE_RESOURCE_TYPES.contains(&kind) {
        return None;
    }
    Some(crate::ops::types::InvalidArgument {
        message: Some(format!("The parameter resource type: {kind} is invalid.")),
    })
}

/// One invalidation batch submitted against one distribution or one distribution tenant.
#[derive(Debug, Clone)]
pub struct StoredInvalidation {
    /// `Invalidation$Id` — "The identifier for the invalidation request."
    pub id: String,
    /// [`STATUS_IN_PROGRESS`] until [`StoredInvalidation::completed_due`], then
    /// [`STATUS_COMPLETED`].
    pub status: String,
    /// "The date and time the invalidation request was first made."
    pub create_time: SystemTime,
    /// The instant the batch is owed to be `Completed`. Stored rather than slept for.
    pub completed_due: SystemTime,
    /// The batch exactly as submitted — "The current invalidation information for the
    /// batch request."
    pub batch: InvalidationBatch,
}

impl StoredInvalidation {
    /// The `Invalidation` projection every invalidation operation returns, read from the
    /// one stored record.
    pub fn to_invalidation(&self) -> Invalidation {
        Invalidation {
            id: self.id.clone(),
            status: self.status.clone(),
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            invalidation_batch: self.batch.clone(),
        }
    }

    /// The paths this batch names — each "the path of the object to invalidate" — or none
    /// at all for a batch whose `Paths` carries no `Items` element.
    pub fn paths(&self) -> &[String] {
        self.batch.paths.items.as_deref().unwrap_or_default()
    }

    /// The `InvalidationSummary` projection a list page carries — "a summary of an
    /// invalidation request" — read from the SAME stored record
    /// [`StoredInvalidation::to_invalidation`] reports, so a summary and a read of the
    /// same invalidation can never disagree about its status or its creation time.
    pub fn to_summary(&self) -> InvalidationSummary {
        InvalidationSummary {
            id: self.id.clone(),
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            status: self.status.clone(),
        }
    }
}

/// Materialise the batch completions the clock owes `invalidations` at `now`.
///
/// Idempotent and monotonic: a batch that has already finished stays finished, and one
/// whose deadline has not passed is left alone, so applying this on a read never changes
/// what a later caller sees. The invalidations of a distribution and those of a
/// distribution tenant are held in the same kind of map and finish under the same rule, so
/// this is one function rather than one per owner.
pub fn settle_invalidations(
    invalidations: &mut BTreeMap<String, StoredInvalidation>,
    now: SystemTime,
) {
    for invalidation in invalidations.values_mut() {
        if invalidation.status != STATUS_COMPLETED && now >= invalidation.completed_due {
            invalidation.status = STATUS_COMPLETED.to_string();
        }
    }
}

/// `invalidations` in the order a list serves them.
///
/// CloudFront documents the order on the `Marker` of every invalidation listing: "the
/// results are returned in decreasing order from most recent to oldest, [so] the most
/// recent results are on the first page". Two invalidations stamped with the SAME instant
/// are ordered by id, which makes this a stable TOTAL order rather than merely a sorted
/// one — and that is what lets a `Marker` naming one of them resume at exactly one
/// position.
///
/// Reads [`StoredInvalidation::status`] nowhere, so the caller decides whether to settle
/// the clock first; every list handler does, because the summaries it builds from these
/// records report that status.
pub fn invalidations_most_recent_first(
    invalidations: &BTreeMap<String, StoredInvalidation>,
) -> Vec<&StoredInvalidation> {
    let mut ordered: Vec<&StoredInvalidation> = invalidations.values().collect();
    ordered.sort_by(|left, right| {
        right
            .create_time
            .cmp(&left.create_time)
            .then_with(|| left.id.cmp(&right.id))
    });
    ordered
}

/// ONE page of `invalidations`, as the `InvalidationList` every CloudFront invalidation
/// listing answers with — or `None` when `marker` names none of them.
///
/// The page contract is the same whichever resource the invalidations hang off, so it
/// lives here once rather than in each handler:
///
/// * the order is the documented one, most recent first
///   ([`invalidations_most_recent_first`]);
/// * "To get the next page of results, set Marker to the value of the NextMarker from the
///   current page's response. This value is the same as the ID of the last invalidation
///   batch on that page" — so the marker is a resume point this service handed out, and
///   this page starts after the invalidation it names. A value naming none of them was
///   never such a resume point: this answers `None`, and the caller refuses it with the
///   modelled `InvalidArgument` naming the resource it asked about, rather than silently
///   serving the first page again;
/// * `page_size` is how many summaries this response serves (see [`page_size`]), and
///   `max_items` is reported as "the value that you provided for the MaxItems request
///   parameter";
/// * `NextMarker` is present exactly when invalidations remain, carrying the id this page
///   ended at, and `IsTruncated` says the same thing;
/// * `Quantity` is the length of `Items`, and a page that lists nothing carries no `Items`
///   element at all rather than an empty one.
///
/// Each summary is [`StoredInvalidation::to_summary`] of the one stored record, so a list
/// page and a read of the same invalidation can never disagree about it.
pub fn invalidation_page(
    invalidations: &BTreeMap<String, StoredInvalidation>,
    marker: Option<&str>,
    max_items: i32,
    page_size: usize,
) -> Option<crate::ops::types::InvalidationList> {
    let ordered = invalidations_most_recent_first(invalidations);
    let start = match marker {
        Some(marker) => {
            ordered
                .iter()
                .position(|invalidation| invalidation.id == marker)?
                + 1
        }
        None => 0,
    };

    let remaining = &ordered[start..];
    let served = remaining.len().min(page_size);
    let items: Vec<InvalidationSummary> = remaining[..served]
        .iter()
        .map(|invalidation| invalidation.to_summary())
        .collect();
    let is_truncated = remaining.len() > served;

    Some(crate::ops::types::InvalidationList {
        marker: marker.unwrap_or_default().to_string(),
        next_marker: if is_truncated {
            items.last().map(|summary| summary.id.clone())
        } else {
            None
        },
        max_items,
        is_truncated,
        quantity: i32::try_from(items.len()).unwrap_or(i32::MAX),
        items: if items.is_empty() { None } else { Some(items) },
    })
}

/// One CloudFront distribution, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredDistribution {
    /// `Distribution$Id`, allocated at creation and never reused.
    pub id: String,
    /// `Distribution$ARN`. CloudFront is a global service, so the ARN carries no region.
    pub arn: String,
    /// `Distribution$DomainName`, allocated at creation and stable for the
    /// distribution's whole life.
    pub domain_name: String,
    /// [`STATUS_IN_PROGRESS`] until [`StoredDistribution::deployed_due`], then
    /// [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this distribution's current configuration is owed to be `Deployed`.
    pub deployed_due: SystemTime,
    /// `Distribution$LastModifiedTime`: when the configuration last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check.
    pub etag: String,
    /// The configuration as submitted, with every member the service itself materialises
    /// resolved into it ([`resolve_distribution_config`]), stored whole so a later read
    /// returns the configuration the distribution actually runs under rather than a
    /// re-derivation of it.
    pub config: DistributionConfig,
    /// Whether this distribution's configuration has been REPLACED since it was created.
    ///
    /// `false` on the record a create leaves behind, and `true` from the first write that
    /// replaces the configuration ([`ComAmazonawsCloudfrontCloudfront20200531State::apply_config`]).
    /// What it decides is the reach of the caller reference's idempotent replay: a reference
    /// names the create it was sent with, so it answers a repeat of that create only while
    /// the distribution still holds the configuration that create stored. Once the
    /// distribution has been reconfigured, the reference is associated with a distribution no
    /// create under it describes, and a create under it is refused rather than answered with
    /// this record.
    pub reconfigured: bool,
    /// The tags associated with this distribution. A distribution created without a tag
    /// set carries none, which is the empty set rather than an absent one.
    pub tags: TagSet,
    /// Every invalidation created under this distribution, keyed by invalidation id.
    pub invalidations: BTreeMap<String, StoredInvalidation>,
    /// The additional-CloudWatch-metrics subscription associated with this distribution,
    /// or `None` while it has none.
    ///
    /// A sub-resource of the distribution rather than a member of its configuration —
    /// `CreateMonitoringSubscription`, `GetMonitoringSubscription` and
    /// `DeleteMonitoringSubscription` address it under
    /// `/distributions/{Id}/monitoring-subscription` — so it is carried on this record
    /// and released with it, and changing it leaves the configuration, the version token
    /// and the propagation status of the distribution exactly as they were. A
    /// distribution is created without one: nothing but a create puts one here.
    pub monitoring_subscription: Option<MonitoringSubscription>,
    /// What this distribution's edge holds in its cache, keyed by the cache key the edge
    /// computed for the request ([`crate::service::edge`]).
    ///
    /// A sub-resource of the running distribution rather than a member of its
    /// configuration: nothing reports it, and filling it changes no value any read
    /// answers with.
    pub edge_cache: BTreeMap<String, CachedEdgeResponse>,
}

impl StoredDistribution {
    /// Apply every transition the clock owes this distribution at `now`.
    ///
    /// Idempotent and monotonic: a status that is already terminal stays put, so calling
    /// this on a read cannot change what a later caller sees.
    ///
    /// BOUNDED, which matters because every handler applies this under the store's lock
    /// before it reads: the sweep that empties the cache runs only for the batches this
    /// call is the one to finish — so its cost is paid once per invalidation ever created,
    /// not once per request — it stops as soon as the cache is empty, and it holds one
    /// batch's paths at a time. The comparisons the WHOLE sweep may make are capped at
    /// [`MAX_INVALIDATION_MATCHES`], the same cap
    /// [`StoredDistribution::invalidate_edge_cache`] holds one batch to: the clock can owe
    /// many batches at one instant, so the budget is shared across them rather than granted
    /// to each, and a sweep that would exceed it empties the cache and ends — the answer that
    /// method already gives a single batch too large to match, for the same reason (removing
    /// more than a batch named can only cost a later request a cache MISS). Nothing here
    /// allocates once the cache is empty.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != STATUS_DEPLOYED && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
        // An invalidation is ASYNCHRONOUS — it is accepted `InProgress` and reports
        // `Completed` only once its batch has finished — so the objects a batch names
        // leave the edge cache when the batch FINISHES, not when it was accepted: until
        // then the cached object is still what a viewer is served. The batches THIS call
        // is the one to finish are named here, before the statuses move, so each batch
        // empties the cache exactly once however often the clock is settled afterwards.
        let finished: Vec<String> = self
            .invalidations
            .values()
            .filter(|invalidation| {
                invalidation.status != STATUS_COMPLETED && now >= invalidation.completed_due
            })
            .map(|invalidation| invalidation.id.clone())
            .collect();
        settle_invalidations(&mut self.invalidations, now);
        // ONE comparison budget for the whole sweep: every batch the clock owes at this
        // instant is swept inside this one call, under the store's lock, so what bounds the
        // work a request pays for is the sum over the batches rather than each batch's own
        // cap.
        let mut budget = MAX_INVALIDATION_MATCHES;
        for id in finished {
            if self.edge_cache.is_empty() {
                // Nothing is held, so no batch has anything to remove: the rest of this
                // sweep would read every remaining batch's paths to do nothing.
                break;
            }
            let Some(paths) = self
                .invalidations
                .get(&id)
                .map(|invalidation| invalidation.paths().to_vec())
            else {
                continue;
            };
            let comparisons = self.edge_cache.len().saturating_mul(paths.len());
            if comparisons > budget {
                // Past the budget the sweep answers as a single oversized batch does: the
                // cache is emptied rather than matched, which removes at least what the
                // remaining batches named and ends the sweep here.
                self.edge_cache.clear();
                break;
            }
            budget -= comparisons;
            self.invalidate_edge_cache(&paths);
        }
    }

    /// This distribution's invalidations in the order a list serves them — the one order
    /// every invalidation listing uses ([`invalidations_most_recent_first`]).
    pub fn invalidations_most_recent_first(&self) -> Vec<&StoredInvalidation> {
        invalidations_most_recent_first(&self.invalidations)
    }

    /// The cached response the edge may serve for `key`, or `None` when it must fetch the
    /// origin.
    ///
    /// An entry is servable while its TTL has not run out. An invalidation does not have
    /// to be consulted here: a batch removes what it names from the cache when the clock
    /// FINISHES it ([`StoredDistribution::settle`], which every read applies before it
    /// looks), so this is one map lookup and a comparison however many batches the
    /// distribution has had. A READ: it reports what the cache holds and changes nothing.
    pub fn cached_edge_response(&self, key: &str, now: SystemTime) -> Option<&CachedEdgeResponse> {
        self.edge_cache
            .get(key)
            .filter(|entry| now < entry.expires_at)
    }

    /// Remove from the edge cache every object `paths` names — "you can invalidate files
    /// ... to remove them from CloudFront edge caches".
    ///
    /// Done once, by the call the clock finishes the batch at ([`StoredDistribution::settle`]),
    /// rather than on every later cache read: one invalidation is one pass over the at most
    /// [`MAX_EDGE_CACHE_ENTRIES`] entries, and a viewer request pays nothing for the
    /// batches that came before it.
    /// A batch whose paths and the cache between them would need more comparisons than
    /// [`MAX_INVALIDATION_MATCHES`] empties the cache instead of matching: removing more
    /// than the batch named can only cost a later request a cache MISS, which is a thing
    /// CloudFront answers too, and it keeps the work one call can ask for bounded.
    pub fn invalidate_edge_cache(&mut self, paths: &[String]) {
        if self.edge_cache.is_empty() || paths.is_empty() {
            return;
        }
        let comparisons = self.edge_cache.len().saturating_mul(paths.len());
        if comparisons > MAX_INVALIDATION_MATCHES {
            self.edge_cache.clear();
            return;
        }
        self.edge_cache.retain(|_, entry| {
            !paths
                .iter()
                .any(|path| matches_path_pattern(path, &entry.path))
        });
    }

    /// Hold `entry` in this distribution's edge cache under `key`.
    ///
    /// Bounded by [`MAX_EDGE_CACHE_ENTRIES`]: once the cache is full the entry filled
    /// longest ago makes room for the new one.
    pub fn store_edge_response(&mut self, key: String, entry: CachedEdgeResponse) {
        while self.edge_cache.len() >= MAX_EDGE_CACHE_ENTRIES {
            let oldest = self
                .edge_cache
                .iter()
                .min_by_key(|(_, held)| held.stored_at)
                .map(|(held_key, _)| held_key.clone());
            match oldest {
                Some(held_key) => {
                    self.edge_cache.remove(&held_key);
                }
                None => break,
            }
        }
        self.edge_cache.insert(key, entry);
    }

    /// "The number of invalidation batches currently in progress" — counted from the
    /// stored invalidations, so it falls as their batches finish.
    pub fn in_progress_invalidation_batches(&self) -> i32 {
        let in_progress = self
            .invalidations
            .values()
            .filter(|invalidation| invalidation.status != STATUS_COMPLETED)
            .count();
        i32::try_from(in_progress).unwrap_or(i32::MAX)
    }

    /// Whether a delete may remove this distribution.
    ///
    /// CloudFront's teardown is TWO steps, and the second one is a wait: you disable the
    /// distribution, and then — "wait until the value of the `Status` column for your
    /// distribution changes from `InProgress` to `Deployed`" — you delete it. So a
    /// distribution is deletable only once its stored configuration has it disabled AND
    /// that configuration has finished propagating; a delete issued while either is
    /// untrue is refused with `DistributionNotDisabled` and the record stays readable.
    ///
    /// Reads [`StoredDistribution::status`], so the caller settles the clock first.
    pub fn deletable(&self) -> bool {
        !self.config.enabled && self.status == STATUS_DEPLOYED
    }

    /// The `DomainName` this distribution reports.
    ///
    /// A standard distribution reports the edge domain allocated to it at creation; a
    /// multi-tenant one has no domain of its own and reports
    /// [`MULTI_TENANT_DOMAIN_NAME`]. Read from the STORED configuration, so every
    /// projection of one record — `Distribution` and `DistributionSummary` alike — reports
    /// the same value, and a configuration change is reflected by all of them at once.
    pub fn reported_domain_name(&self) -> String {
        if is_multi_tenant(&self.config) {
            MULTI_TENANT_DOMAIN_NAME.to_string()
        } else {
            self.domain_name.clone()
        }
    }

    /// The `Distribution` projection `CreateDistribution`, `GetDistribution` and
    /// `UpdateDistribution` return.
    ///
    /// `key_groups` are the account's stored key groups, which the active key group set
    /// below reports the public keys of: a distribution's payload names the key groups its
    /// configuration trusts, and the keys in each one are the ones those RECORDS hold.
    pub fn to_distribution(
        &self,
        key_groups: &BTreeMap<String, StoredKeyGroup>,
    ) -> Distribution {
        Distribution {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            in_progress_invalidation_batches: self.in_progress_invalidation_batches(),
            domain_name: self.reported_domain_name(),
            active_trusted_signers: Some(self.active_trusted_signers()),
            active_trusted_key_groups: Some(self.active_trusted_key_groups(key_groups)),
            distribution_config: self.config.clone(),
            // An ICP recordal applies only to a distribution with alternate domain
            // names in the China partition, which this emulator does not serve.
            alias_icp_recordals: None,
        }
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// Every cache behaviour of the stored configuration, as the view that names the list
    /// wrappers they share: the default cache behaviour first, then the ordered ones.
    ///
    /// Bounded by [`MAX_CACHE_BEHAVIORS`], [`MAX_TRUSTED_SIGNERS`] and
    /// [`MAX_TRUSTED_KEY_GROUPS`], which [`config_limit_exceeded`] refuses a configuration
    /// over BEFORE it is stored — so the walks below cost a bounded amount of work on
    /// every read, whatever a request asked to store.
    pub(super) fn behaviors(&self) -> Vec<BehaviorLists<'_>> {
        behavior_views(&self.config)
    }

    /// "A list of Amazon Web Services accounts and the active CloudFront key pairs in each
    /// account that CloudFront can use to verify the signatures of signed URLs and signed
    /// cookies."
    ///
    /// Read from the stored configuration, which is what says who the trusted signers are:
    /// `Enabled` is "true if any of the Amazon Web Services accounts in the list are
    /// configured as trusted signers", `Quantity` is "the number of Amazon Web Services
    /// accounts in the list", and the list itself is every account the distribution's cache
    /// behaviours trust, each named once in the order they first appear.
    ///
    /// A signer's `KeyPairIds` are the ACTIVE CloudFront key pairs of that account — the
    /// account-wide key pairs a root user creates, which this service's API cannot create
    /// (no operation in the model mints one), so no account has one and the reported set is
    /// empty. The public keys a key group holds are a different thing, reported by
    /// [`StoredDistribution::active_trusted_key_groups`] from the stored key groups.
    pub fn active_trusted_signers(&self) -> ActiveTrustedSigners {
        let behaviors = self.behaviors();
        let accounts = first_seen(
            behaviors
                .iter()
                .filter_map(|behavior| behavior.trusted_signers)
                .flat_map(|signers| signers.items.as_deref().unwrap_or_default()),
        );
        ActiveTrustedSigners {
            enabled: behaviors
                .iter()
                .any(|behavior| behavior.trusted_signers.is_some_and(|signers| signers.enabled)),
            quantity: i32::try_from(accounts.len()).unwrap_or(i32::MAX),
            items: (!accounts.is_empty()).then(|| {
                accounts
                    .into_iter()
                    .map(|account| Signer {
                        aws_account_number: Some(account),
                        key_pair_ids: Some(KeyPairIds {
                            quantity: 0,
                            items: None,
                        }),
                    })
                    .collect()
            }),
        }
    }

    /// "A list of key groups, and the public keys in each key group, that CloudFront can
    /// use to verify the signatures of signed URLs and signed cookies."
    ///
    /// Read from the stored configuration the same way [`StoredDistribution::active_trusted_signers`]
    /// is: `Quantity` is "the number of key groups in the list" and the list is every key
    /// group the distribution's cache behaviours trust, each named once, and `Enabled`
    /// reports whether the configuration trusts any key group at all.
    ///
    /// A group's `KeyPairIds` are "the identifiers of the public keys in each key group
    /// that CloudFront can use to verify the signatures", which is what the key group
    /// RECORD `key_groups` holds for that id states — so the set is read from the store
    /// rather than from this distribution's configuration, and a trusted id the account
    /// holds no key group for contributes no key.
    pub fn active_trusted_key_groups(
        &self,
        key_groups: &BTreeMap<String, StoredKeyGroup>,
    ) -> ActiveTrustedKeyGroups {
        let behaviors = self.behaviors();
        let groups = first_seen(
            behaviors
                .iter()
                .filter_map(|behavior| behavior.trusted_key_groups)
                .flat_map(|groups| groups.items.as_deref().unwrap_or_default()),
        );
        ActiveTrustedKeyGroups {
            enabled: behaviors
                .iter()
                .any(|behavior| behavior.trusted_key_groups.is_some_and(|groups| groups.enabled)),
            quantity: i32::try_from(groups.len()).unwrap_or(i32::MAX),
            items: (!groups.is_empty()).then(|| {
                groups
                    .into_iter()
                    .map(|group| {
                        let keys = key_groups
                            .get(&group)
                            .map(|stored| stored.config.items.as_slice())
                            .unwrap_or_default();
                        KGKeyPairIds {
                            key_group_id: Some(group),
                            key_pair_ids: Some(KeyPairIds {
                                quantity: i32::try_from(keys.len()).unwrap_or(i32::MAX),
                                items: (!keys.is_empty()).then(|| keys.to_vec()),
                            }),
                        }
                    })
                    .collect()
            }),
        }
    }

    /// The `DistributionSummary` projection `ListDistributions` returns.
    ///
    /// Every member it shares with [`StoredDistribution::to_distribution`] takes the
    /// same value: both read the one stored record. Where the summary requires a member
    /// the stored configuration leaves out, the value reported is the one the absent
    /// configuration member means (see [`DEFAULT_PRICE_CLASS`],
    /// [`DEFAULT_HTTP_VERSION`], [`GEO_RESTRICTION_NONE`]).
    pub fn to_summary(&self) -> DistributionSummary {
        let config = &self.config;
        DistributionSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            // "ETag: the current version of the distribution" — the same token the last
            // mutation handed back.
            e_tag: Some(self.etag.clone()),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            domain_name: self.reported_domain_name(),
            aliases: config.aliases.clone().unwrap_or_default(),
            origins: config.origins.clone(),
            origin_groups: config.origin_groups.clone(),
            default_cache_behavior: config.default_cache_behavior.clone(),
            // An absent wrapper means no cache behaviours and no custom error
            // responses at all, which is the empty wrapper.
            cache_behaviors: config.cache_behaviors.clone().unwrap_or(CacheBehaviors {
                quantity: 0,
                items: None,
            }),
            custom_error_responses: config.custom_error_responses.clone().unwrap_or(
                CustomErrorResponses {
                    quantity: 0,
                    items: None,
                },
            ),
            comment: config.comment.clone(),
            price_class: config
                .price_class
                .clone()
                .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
            enabled: config.enabled,
            // A configuration that names no certificate configures nothing about the
            // viewer certificate, so there is nothing to report under it.
            viewer_certificate: config.viewer_certificate.clone().unwrap_or_default(),
            restrictions: config
                .restrictions
                .clone()
                .unwrap_or_else(|| Restrictions {
                    geo_restriction: GeoRestriction {
                        restriction_type: GEO_RESTRICTION_NONE.to_string(),
                        quantity: 0,
                        items: None,
                    },
                }),
            web_acl_id: config.web_acl_id.clone().unwrap_or_default(),
            http_version: config
                .http_version
                .clone()
                .unwrap_or_else(|| DEFAULT_HTTP_VERSION.to_string()),
            is_ipv6_enabled: config.is_ipv6_enabled.unwrap_or_default(),
            alias_icp_recordals: None,
            staging: config.staging.unwrap_or_default(),
            connection_mode: config.connection_mode.clone(),
            anycast_ip_list_id: config.anycast_ip_list_id.clone(),
            viewer_mtls_config: config.viewer_mtls_config.clone(),
            connection_function_association: config.connection_function_association.clone(),
        }
    }
}
