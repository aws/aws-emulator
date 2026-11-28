// SPDX-License-Identifier: CC0-1.0

//! Every BOUND this service's state is held to, in one place: the per-Region quota each
//! create is refused past, the per-record bounds a resource's own contents are held to,
//! and the server-side ceiling a page of a listing is clamped to.
//!
//! The figures are gathered here rather than left beside each record so a reader can see
//! at a glance that the store cannot grow without limit: every one of them is a
//! compile-time constant, and the whole store holds at most [`MAX_REGIONS`] times the
//! per-Region quota of each kind, each record bounded in turn by the per-record figures
//! below. The refusal every create is worded through is [`create_quota_refusal`], so no
//! create of this service can disagree with another about which clauses apply or in which
//! order.

use super::*;

/// The most feeds this emulator keeps in ONE Region at once.
///
/// Elemental Inference limits how many feeds an account may hold (the model declares
/// `ServiceQuotaExceededException` for `CreateFeed` precisely for that), but it does
/// not state the figure, so this one is the emulator's own. A service quota is a
/// per-Region figure, so this is what one Region's endpoint holds: past it `CreateFeed`
/// refuses with the modelled `ServiceQuotaExceededException` (HTTP 402), which is the
/// answer the real service gives when the quota is exhausted, and together with
/// [`MAX_REGIONS`] it is what bounds a store that would otherwise grow by one record
/// per request forever, and caps the work every settle pass and every page can do.
pub const MAX_FEEDS: usize = 100;

/// The most dictionaries this emulator keeps in ONE Region at once.
///
/// As with [`MAX_FEEDS`], Elemental Inference limits how many dictionaries an account
/// may hold (the model declares `ServiceQuotaExceededException` for
/// `CreateDictionary`) without stating the figure, so this one is the emulator's own,
/// and it is per Region because a service quota is. Past it `CreateDictionary` refuses
/// with the modelled `ServiceQuotaExceededException` (HTTP 402), which is also the
/// answer the real service gives once the quota is exhausted.
pub const MAX_DICTIONARIES: usize = 100;

/// The most functions this emulator keeps in ONE Region at once — on the same reasoning
/// as [`MAX_DICTIONARIES`]: the model declares `ServiceQuotaExceededException` for
/// `CreateFunction` without stating the figure, so this one is the emulator's own, and it
/// is per Region because a service quota is.
pub const MAX_FUNCTIONS: usize = 100;

/// The most question sets this emulator keeps in ONE Region at once — the question set's
/// counterpart of [`MAX_DICTIONARIES`], on the same reasoning.
pub const MAX_QUESTION_SETS: usize = 100;

/// The most published versions one question set keeps at once.
///
/// "A question set can have a limited number of versions. If the question set already has
/// the maximum number of versions, Elemental Inference returns
/// ServiceQuotaExceededException." (`CreateQuestionSetVersion`) — the limit is documented,
/// the figure is not, so this one is the emulator's own. It bounds the versions one record
/// holds, which is what keeps the store bounded by [`MAX_QUESTION_SETS`] alone.
pub const MAX_QUESTION_SET_VERSIONS: usize = 10;

/// The most scored items one version of a question set may contain.
///
/// DOCUMENTED: "A version can contain up to 100 scored items, where each yesNo question,
/// each option of a yesNoPerOption question, and each chooseOne question counts as one
/// item." (`CreateQuestionSet`).
pub const MAX_SCORED_ITEMS: usize = 100;

/// The most schedule actions one feed holds at once.
///
/// MODELLED: `ListScheduleActionsResponse$scheduleActions` is a list of at most 100
/// entries (`smithy_query(query="operation",operation="ListScheduleActions")`), so a feed
/// cannot hold more than a listing can report. Past it `CreateScheduleAction` refuses with
/// the modelled `ServiceQuotaExceededException` (HTTP 402), which also bounds the per-feed
/// state a run of requests can grow.
pub const MAX_SCHEDULE_ACTIONS_PER_FEED: usize = 100;

/// The most Regions this store keeps resources of one kind in at once.
///
/// The Region a record belongs to comes from the request's own SigV4 credential scope
/// ([`request_region`]), so a caller chooses it: without a bound on how many distinct
/// Regions the store partitions across, a per-Region quota would bound nothing and a
/// run of requests could grow the store without limit. With it the store holds at most
/// [`MAX_REGIONS`] * [`MAX_FEEDS`] feeds and as many dictionaries, and a create signed
/// for one Region past that earns the same modelled `ServiceQuotaExceededException`
/// (HTTP 402) a create past a Region's own quota does. The figure is comfortably more
/// than the Regions this service is offered in, so no caller using it as a client of
/// the real service's Regions meets it.
pub const MAX_REGIONS: usize = 32;

/// The most outputs one feed may hold.
///
/// `AssociateFeed` APPENDS outputs ("New outputs will be appended"), so without a
/// bound a caller could grow one feed's record by another output on every call. AWS
/// does not publish the figure, so this one is the emulator's own: it is the BOUND on
/// a feed's output list, and past it a request refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the answer the real service gives
/// when a request would exceed a service quota.
///
/// It binds EVERY operation that sets a feed's output list, not only the append:
/// `CreateFeed` and `UpdateFeed` each REPLACE the list, so one request of theirs would
/// otherwise put a stored feed arbitrarily far past the bound in a single call. Holding
/// it on all three is what makes the stored list short — and the stored list is what a
/// later call reads, compares against and reports, so the bound on it bounds that work
/// too.
pub const MAX_OUTPUTS_PER_FEED: usize = 20;

/// Which clause of the per-Region create quota refuses one more record of a resource
/// kind.
///
/// A create of EITHER kind is held to the same two-clause rule, so which clauses there
/// are, which comparison each makes and the order they are decided in are stated once
/// ([`create_quota_refusal`]) instead of once per kind. Each kind only words the clause
/// it earned, since the sentence names the resource and the bound it was decided from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateQuotaRefusal {
    /// The Region already holds as many records of the kind as its quota allows.
    /// Reported as the modelled `ServiceQuotaExceededException` (HTTP 402).
    RegionFull,
    /// The create would open a record in a Region this store holds none of this kind in,
    /// and the store already partitions this kind across [`MAX_REGIONS`] Regions.
    /// Reported as the modelled `ServiceQuotaExceededException` (HTTP 402).
    NewRegionBeyondBound,
}

/// Which clause of the per-Region create quota rule, if any, refuses one more record in
/// `region` — `None` while that Region may take one.
///
/// The ONE statement of the rule both of this service's creates follow. `regions` is the
/// Region of every record of the kind the store holds and `max_per_region` that kind's
/// own bound, so a kind supplies its map and its figure and the rule itself is written
/// here: a service quota is a PER-REGION figure, so what the Region already holds is what
/// decides the first clause — another Region's records neither consume it nor are consumed
/// by it, and a record whose deletion has completed holds nothing, since the record went
/// with the resource. The second clause is the STORE's own bound rather than a quota of
/// the service (see [`MAX_REGIONS`]): a Region this store holds no record of the kind in
/// is a new partition, and there are at most that many. The Region's own quota is decided
/// FIRST, so a Region that is full is told so whether or not it is also a new partition.
///
/// Walks the kind's records once, counting both what `region` holds and how many distinct
/// Regions the store partitions the kind across.
pub fn create_quota_refusal<'a>(
    regions: impl Iterator<Item = &'a str>,
    region: &str,
    max_per_region: usize,
) -> Option<CreateQuotaRefusal> {
    let mut held_in_region = 0usize;
    let mut partitions: BTreeSet<&str> = BTreeSet::new();
    for held in regions {
        if held == region {
            held_in_region += 1;
        }
        partitions.insert(held);
    }
    if held_in_region >= max_per_region {
        return Some(CreateQuotaRefusal::RegionFull);
    }
    // A Region holding no record of the kind is one this store has no partition for yet.
    if held_in_region == 0 && partitions.len() >= MAX_REGIONS {
        return Some(CreateQuotaRefusal::NewRegionBeyondBound);
    }
    None
}

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// The refusal a create of ONE resource kind answers in `region`, or `None` while
    /// that Region may take one more record of it.
    ///
    /// The one place every kind's refusal is worded: `regions` is the Region of every
    /// record of the kind the store holds, `max_per_region` that kind's own finite bound
    /// above, and `plural` how a sentence names the kind. Which clauses a create is held
    /// to and in which order is [`create_quota_refusal`], so no kind can disagree with
    /// another; each refusal states the bound it was decided from and reports nothing else
    /// about what the account holds.
    pub(super) fn quota_refusal<'a>(
        &self,
        regions: impl Iterator<Item = &'a str>,
        region: &str,
        max_per_region: usize,
        plural: &str,
    ) -> Option<String> {
        create_quota_refusal(regions, region, max_per_region).map(|refusal| match refusal {
            CreateQuotaRefusal::RegionFull => format!(
                "This account already holds the maximum of {max_per_region} {plural} in this \
                 Region. Delete unused {plural} or request a quota increase."
            ),
            CreateQuotaRefusal::NewRegionBeyondBound => format!(
                "This account already holds {plural} in the maximum of {MAX_REGIONS} Regions. \
                 Delete the {plural} of a Region you no longer use or request a quota increase."
            ),
        })
    }

    /// The refusal `CreateFeed` answers in `region`, or `None` while that Region may
    /// take one more feed — the feed store against [`MAX_FEEDS`].
    pub fn feed_quota_refusal(&self, region: &str) -> Option<String> {
        self.quota_refusal(
            self.feeds.values().map(|feed| feed.region.as_str()),
            region,
            MAX_FEEDS,
            "feeds",
        )
    }

    /// The refusal `CreateDictionary` answers in `region`, or `None` while that Region
    /// may take one more dictionary — the dictionary store against [`MAX_DICTIONARIES`].
    pub fn dictionary_quota_refusal(&self, region: &str) -> Option<String> {
        self.quota_refusal(
            self.dictionaries
                .values()
                .map(|dictionary| dictionary.region.as_str()),
            region,
            MAX_DICTIONARIES,
            "dictionaries",
        )
    }
}

/// The most tags one resource may carry.
///
/// AWS's tag restrictions, which apply to every taggable resource, cap a resource at
/// 50 tags ("Maximum number of tags per resource: 50", *Tagging AWS resources*), and
/// this service's model states no figure of its own, so that published cap is the
/// bound here. It is also what BOUNDS the tag map on a stored record, which
/// `TagResource` would otherwise grow by one entry per request forever: past it the
/// request is refused with the modelled `ValidationException` (HTTP 400), the answer a
/// request that "fails to satisfy the constraints specified by the service" earns.
///
/// It bounds the tag set a request ASKS a resource to carry, whichever operation
/// carries it: `TagResource` accumulates onto the stored set, while a create's tag map
/// is the whole set the new resource would carry, so each is measured against the same
/// cap ([`invalid_tags_message`]).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many records one page of a paginated operation carries, resolved from what the
/// request asked for.
///
/// The rule every paginated operation of this service follows, stated once: a request
/// that names `maxResults` is answered with that many records, one that names none with
/// the operation's own `default`, and neither fewer than one record nor more than
/// `ceiling`. `default` and `ceiling` are the per-operation constants — the page size
/// the operation's model documents for a request that omits `maxResults`, and the
/// `@range` maximum the model gives that member — so each listing keeps its own values
/// while the resolution is written here.
///
/// The floor and the ceiling do not depend on the boundary having refused an out-of-range
/// `maxResults`: they are the server-side bound, so no page can exceed `ceiling` records
/// however the request was built.
pub fn resolve_page_size(max_results: Option<i32>, default: usize, ceiling: usize) -> usize {
    max_results
        .map_or(default, |max| max.max(1) as usize)
        .clamp(1, ceiling)
}
