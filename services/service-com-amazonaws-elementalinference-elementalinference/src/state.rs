// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elementalinference-elementalinference service.
//!
//! The feed is this service's primary resource: `CreateFeed` mints one, `GetFeed` /
//! `ListFeeds` report it, `UpdateFeed` mutates it and `DeleteFeed` retires it. The
//! record below is the single copy every one of those handlers reads and writes, so
//! the shapes the handlers hand back are all built from it. A feed's resource-based
//! policy is part of that record ([`Feed::policy`]) rather than a store of its own:
//! `PutFeedPolicy` attaches or replaces it, `GetFeedPolicy` reports it and
//! `DeleteFeedPolicy` detaches it, so the document a client reads back is the one the
//! last put stored, and a feed that leaves the store takes its policy with it.
//!
//! A deletion that has COMPLETED leaves NOTHING behind: a delete is ACCEPTED (HTTP 202)
//! and the record is REMOVED from the store the instant the deletion falls due (see
//! `settle_due_transitions`). From that instant the id names no resource of this
//! service at all — for every operation, the delete itself included — so a repeat of a
//! delete whose deletion has completed is refused with `ResourceNotFoundException`
//! exactly as an id this store never held is, and no record outlives the resource it
//! described. That also keeps the store bounded by the live quotas below alone.
//!
//! The dictionary is this service's other resource — a named, language-tagged payload
//! of custom words a subtitling output can reference. `CreateDictionary` mints one,
//! `GetDictionary` reports it, `UpdateDictionary` mutates it and `DeleteDictionary`
//! retires it; all four work on the single [`Dictionary`] record stored here, exactly
//! as the feed handlers work on [`Feed`]. WHICH feeds reference a dictionary is not
//! stored on the dictionary: it is derived from the feed store
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_referencing`]), so
//! one copy of that fact backs the dictionary's `references` list, its `REFERENCED`
//! status and `DeleteDictionary`'s refusal alike.
//!
//! Both of those resources are REGIONAL: this service is reached through a Region's
//! endpoint and each endpoint owns the resources created through it. So every record
//! carries the Region it was created in ([`Feed::region`], [`Dictionary::region`]), and
//! every operation that reaches one resolves it WITHIN the Region the request was signed
//! for ([`ComAmazonawsElementalinferenceElementalinferenceState::feed`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::dictionary`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::feeds_in`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::dictionaries_in`]): a
//! resource is listed, read and mutated through the endpoint that created it alone, an
//! id presented to any other Region names nothing there, and the quotas below are each
//! Region's own. The ARN-based tagging path resolves within that same Region
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource_mut`]), so
//! the id-based and the ARN-based paths cannot disagree about which resources a Region
//! holds. WHICH Region a request reached is read off its own credential scope, held to
//! the Region grammar first ([`request_region`]), so every ARN this service mints
//! carries a Region field the modelled `resourceArn` admits and a resource this service
//! handed an ARN for is one its tagging operations can be called with.
//!
//! A FIXTURE is not one of those resources: no operation of this service creates,
//! updates or deletes one. A fixture is a sports event "as provided by the data
//! source" (`GetFixtureResponse$name`/`$status`), an external feed of sports data that
//! both `SearchFixtures` and `GetFixture` only report — the model marks each of them
//! `@readonly`, so neither may write. The fixture catalog at the bottom of this module
//! is therefore a STORE of the events that data source holds
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::fixtures`]), and both
//! operations answer out of it: a search reports the held fixtures its criteria select
//! and nothing else, and `GetFixture` answers for an id the store holds.
//!
//! That store is FILLED when it is constructed, with the schedule this emulator's own
//! data source holds ([`default_fixture_catalog`]): a client can only learn a
//! `fixtureId` from a search, so a catalog holding nothing would make the whole
//! fixtures pair unusable — every search empty whatever it asked for, `GetFixture`
//! unreachable, and a clipping output with no event to name. The schedule covers the
//! [`CATALOG_SPAN_DAYS`] days either side of the day the store is built, for each sport
//! the `DataSourceSport` enum declares. A window OUTSIDE that span holds no event and
//! so still reports an empty list ("The array is empty if no fixtures match",
//! `SearchFixturesResponse$fixtures`), the in-process seam
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::seed_fixtures`] and
//! [`ComAmazonawsElementalinferenceElementalinferenceState::clear_fixtures`]) still
//! installs or drops records, and no request mutates what a later caller sees.
//!
//! ## How this module is laid out
//!
//! This file holds the STORE itself — the state struct, how it is constructed, and the
//! lookups and the settle pass every handler reaches it through. Everything else is a
//! child module, one per concern, and re-exported here so a handler still names one path
//! (`crate::state::…`):
//!
//! * the resource RECORDS: [`feeds`], [`dictionaries`], [`functions`], [`question_sets`]
//!   and the [`schedule_actions`] a feed carries;
//! * the BOUNDS all of them are held to, gathered in [`quotas`];
//! * the rules a request's payload is judged by: [`output_lists`], [`tags`],
//!   [`enum_value_sets`], [`dictionary_payload`];
//! * the wire surfaces: [`feed_wire`], [`page_tokens`], [`tagged_resources`];
//! * the values the service chooses for itself: [`ids`], and the [`fixtures`] catalog;
//! * [`output_config_equality`], the by-value comparison a stored output is compared by.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

mod dictionaries;
mod dictionary_payload;
mod enum_value_sets;
mod feed_wire;
mod feeds;
mod fixtures;
mod functions;
mod ids;
mod output_config_equality;
mod output_lists;
mod page_tokens;
mod question_sets;
mod quotas;
mod schedule_actions;
mod tagged_resources;
mod tags;

pub use dictionaries::*;
pub use dictionary_payload::*;
pub use enum_value_sets::*;
pub use feed_wire::*;
pub use feeds::*;
pub use fixtures::*;
pub use functions::*;
pub use output_lists::*;
pub use page_tokens::*;
pub use question_sets::*;
pub use quotas::*;
pub use schedule_actions::*;
pub use tagged_resources::*;
pub use tags::*;
// The id mint is the store's own: nothing outside this module names it, so it is
// imported rather than re-exported.
use ids::*;

use crate::ops::types::{
    AspectRatio, Competitor, DictionarySummary, FeedAssociation, FeedSummary, FixtureSummary,
    GetOutput, OutputConfig, QuestionSetReference, ScheduleActionOperation,
    ScheduleActionStartConfiguration,
};

/// This service's SigV4 signing name and ARN namespace (`OPERATIONS.md`, "Wire
/// protocol"), used for every ARN the service mints.
pub const SERVICE_NAMESPACE: &str = "elemental-inference";

/// The region an unsigned request is attributed to. `emulator_arn` documents
/// `us-east-1` as the generated emulator's default region.
///
/// Also where a request whose SigV4 credential scope names something that is not a
/// Region code at all is attributed ([`request_region`]): such a scope names no Region,
/// so the request is treated as one that carries none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a clock-owed transition takes: a feed's `DELETING` -> gone and its
/// `UPDATING` -> the status it was reconfigured from, and a dictionary's `CREATING` ->
/// `AVAILABLE` and `DELETING` -> gone.
///
/// AWS documents no duration for these, so this span is chosen rather than
/// specified: it is non-zero, so a caller that reads a resource straight after
/// starting the transition still observes the transient status the real service
/// reports, and it is short enough that no caller has to wait long for the settled
/// state. The due instant is stored on the record and materialised on the next
/// call — nothing ever sleeps for it.
///
/// A CREATED FEED's `CREATING` -> `AVAILABLE` move is NOT one of them, and this span
/// does not apply to it: see [`FEED_CREATION_SPAN`].
pub const TRANSITION_SPAN: Duration = Duration::from_millis(400);

/// How long a feed's `CREATING` -> `AVAILABLE` move takes: no time at all, so the move
/// is already owed the instant `CreateFeed` has answered.
///
/// `CreateFeed` is accepted (HTTP 202) with the feed in `CREATING` — creation has been
/// ACCEPTED, not completed — and the recorded service then has the feed built by the
/// time the caller's NEXT request arrives: a `GetFeed` taken immediately after the
/// accept reports `AVAILABLE`, the `AssociateFeed` a caller issues in its very next
/// request succeeds, and a `DisassociateFeed` refused right after a create reports the
/// status it observed as `AVAILABLE`. So the span a caller could observe `CREATING` for
/// is empty: the record passes through it, and the move to `AVAILABLE` falls due at the
/// accept instant rather than at a later one this emulator would have to invent.
/// Nothing sleeps for it either — the due instant is stored on the record and
/// materialised by the next call's settle pass ([`Feed::transition_due_at`],
/// [`ComAmazonawsElementalinferenceElementalinferenceState::settle_due_transitions`]).
pub const FEED_CREATION_SPAN: Duration = Duration::ZERO;

/// The `FeedStatus` values this slice of the service produces.
///
/// `DELETED` is not a value a feed record holds: a deletion that completes REMOVES the
/// feed, so there is no feed left to report it (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedStatus {
    /// The feed's creation has been ACCEPTED and is not complete yet: the status
    /// `CreateFeed`'s own 202 answer reports, since "after creation of the feed has
    /// succeeded, the status will be AVAILABLE" (`CreateFeedResponse$status`) — that is,
    /// AVAILABLE is what the feed becomes once the accepted work is done. Transient: the
    /// clock moves the feed to [`Self::Available`], and it owes that move already when
    /// the accept answers ([`FEED_CREATION_SPAN`]).
    Creating,
    /// The feed is settled and serviceable, with no resource associated.
    Available,
    /// A resource (the source media) is associated with the feed: `AssociateFeed`
    /// claims the feed for one resource, and that claim is what `ACTIVE` reports.
    Active,
    /// The feed is being reconfigured in place: `UpdateFeed` has replaced its payload
    /// and the reconfiguration is in flight. Transient — the clock moves the feed back
    /// into the status it was reconfigured from ([`Feed::settles_into`]).
    Updating,
    /// A `DeleteFeed` has been accepted; the deletion completes when it falls due.
    Deleting,
    /// The feed's resource has been released by `DisassociateFeed`. Terminal: a
    /// released feed takes no further association.
    Archived,
}

impl FeedStatus {
    /// The value this status takes on the wire, as spelled by the `FeedStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Active => "ACTIVE",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
            Self::Archived => "ARCHIVED",
        }
    }
}

/// The message this service renders for `ResourceNotFoundException`.
///
/// ElementalInference renders this error with one fixed text, the same whichever
/// operation refused and whichever id it was given, so the refusal never reports the
/// id that was probed.
pub const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The messages this service renders for the refusals its handlers decide from STORED
/// STATE or from a relation between request members.
///
/// A refusal's message follows the RULE that refused the request, not the request's or
/// the record's values: where the violated rule is a property of the stored resource,
/// or a relation between two members, no single value is what is at fault, so the
/// message is the rule's invariant sentence — the same text whatever the request named
/// and whatever the record holds. That is also what makes a message matchable by a
/// client, and it is what keeps one caller's refusal from disclosing another caller's
/// association or a resource's internal status. Only a rule about ONE value's own
/// validity (a member whose value violates its constraint, an identifier that resolved
/// to nothing) names a value, and then it names the value the REQUEST supplied.
///
/// The EXCEPTION is a refusal whose point is to report the state it was decided from:
/// where the service's refusal states the status it observed or how many records held
/// the request back, that figure is the information the refusal exists to give, so the
/// message is BUILT from the store and tracks it ([`feed_not_associated_message`],
/// [`dictionary_referenced_message`] — functions rather than constants for exactly that
/// reason). A fixed sentence in their place tells a caller nothing it did not already
/// know, and leaves tooling that reads the status or the count out of the refusal with
/// nothing to read.
///
/// `AssociateFeed` on a feed that already holds an association: the recorded refusal is
/// this sentence, naming neither the resource that holds the feed nor the feed's status.
pub const FEED_ALREADY_ASSOCIATED_MESSAGE: &str = "The feed is already associated with a resource.";

/// `AssociateFeed` on a feed whose status admits no association at all (every status
/// other than `AVAILABLE`, and `ACTIVE`, which [`FEED_ALREADY_ASSOCIATED_MESSAGE`]
/// covers): the rule is a property of the stored feed, so the sentence carries no
/// status.
pub const FEED_NOT_ASSOCIABLE_MESSAGE: &str =
    "You can't associate this feed because its current status doesn't allow an association.";

/// `DisassociateFeed` naming a resource that is not the one associated with the feed:
/// the recorded refusal is this sentence, naming neither the rejected name nor the
/// resource that does hold the feed.
pub const WRONG_ASSOCIATED_RESOURCE_MESSAGE: &str =
    "You can't disassociate this feed because it isn't associated with the resource you \
     specified. Specify the correct resource.";

/// `DisassociateFeed` on a feed that holds no resource to release: the refusal REPORTS
/// the status the feed was observed in, which is what decided it — recorded example,
/// for a feed that was never associated: "You can't disassociate this feed. It has
/// status AVAILABLE which indicate that it's not currently associated with a resource."
///
/// So the status is read from the stored feed on every call: a feed that is settled
/// without a resource, one whose deletion has been accepted and one that has been
/// released each report their own status, and a client diagnosing the refusal can tell
/// which of them blocked it.
pub fn feed_not_associated_message(status: FeedStatus) -> String {
    format!(
        "You can't disassociate this feed. It has status {} which indicate that it's not \
         currently associated with a resource.",
        status.as_wire()
    )
}

/// `UpdateFeed` on a feed whose deletion has already been accepted: a property of the
/// stored feed, so the sentence names neither the feed nor its status.
pub const FEED_BEING_DELETED_MESSAGE: &str =
    "You can't update this feed because it is being deleted.";

/// `UpdateDictionary` on a dictionary whose deletion has already been accepted: as with
/// [`FEED_BEING_DELETED_MESSAGE`], a property of the stored dictionary, so the sentence
/// names neither the dictionary nor its status.
pub const DICTIONARY_BEING_DELETED_MESSAGE: &str =
    "You can't update this dictionary because it is being deleted.";

/// `UpdateFunction`, or a tag write, on a function whose deletion has already been
/// accepted: as with [`FEED_BEING_DELETED_MESSAGE`], a property of the stored function,
/// so the sentence names neither the function nor its status.
pub const FUNCTION_BEING_DELETED_MESSAGE: &str =
    "You can't update this function because it is being deleted.";

/// `DeleteDictionary` on a dictionary a feed references ("You cannot delete a dictionary
/// that is referenced by a feed. You must first remove the dictionary reference from the
/// feed's subtitling configuration."): the refusal REPORTS how many feeds reference the
/// dictionary, which is what decided it — recorded example, for a single referencing
/// feed: "Cannot delete dictionary because it is referenced by 1 feed(s). Remove all
/// references by delete or disable related feed output(s) before deleting dictionary."
///
/// So the count is derived from the feed store on every call
/// ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_blocking_delete`]):
/// a caller tearing a dictionary down learns how much is left to remove, while the
/// refusal still names no feed, so it discloses no other caller's resource.
pub fn dictionary_referenced_message(feed_count: usize) -> String {
    format!(
        "Cannot delete dictionary because it is referenced by {feed_count} feed(s). Remove \
         all references by delete or disable related feed output(s) before deleting \
         dictionary."
    )
}

/// `PutFeedPolicy` with a `policy` that is not a policy document.
///
/// A resource-based policy is a JSON policy document (`PutFeedPolicyRequest$policy`:
/// "The resource-based policy document to attach to the feed"), so a body that does not
/// parse as a single JSON object is not one. The rule is about the member's own value,
/// so the message names the member and the form it must take and echoes nothing the
/// request supplied.
pub const INVALID_POLICY_DOCUMENT_MESSAGE: &str =
    "The policy member must be a resource-based policy document: supply one JSON object.";

/// Whether `policy` is a policy document, i.e. parses as a single JSON object.
///
/// One scan shared by every operation that is handed a policy document, so no two of
/// them can disagree about what the service accepts as one.
pub fn is_policy_document(policy: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(policy).is_ok_and(|parsed| parsed.is_object())
}

/// Per-service state for com-amazonaws-elementalinference-elementalinference. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsElementalinferenceElementalinferenceState {
    /// Every live feed, keyed by its `FeedId`. `BTreeMap` so `ListFeeds` has one
    /// stable order to paginate over. PARTITIONED BY REGION: each record carries the
    /// Region it was created in ([`Feed::region`]) and is reachable only through that
    /// Region ([`Self::feed`], [`Self::feeds_in`]), so this one map holds every
    /// Region's feeds without any of them seeing another's. BOUNDED at [`MAX_FEEDS`]
    /// entries per Region across at most [`MAX_REGIONS`] Regions: `CreateFeed` refuses
    /// past either with the modelled `ServiceQuotaExceededException`, so this map
    /// cannot grow without limit however many requests arrive.
    pub feeds: BTreeMap<String, Feed>,
    /// Every live dictionary, keyed by its `DictionaryId`. `BTreeMap` so
    /// `ListDictionaries` has one stable order to paginate over. Partitioned by Region
    /// exactly as [`Self::feeds`] is ([`Dictionary::region`], [`Self::dictionary`],
    /// [`Self::dictionaries_in`]), and BOUNDED at [`MAX_DICTIONARIES`] entries per
    /// Region across at most [`MAX_REGIONS`] Regions: `CreateDictionary` refuses past
    /// either with the modelled `ServiceQuotaExceededException`.
    pub dictionaries: BTreeMap<String, Dictionary>,
    /// Every live function, keyed by its `FunctionId`. `BTreeMap` so `ListFunctions` has
    /// one stable order to paginate over. Partitioned by Region exactly as
    /// [`Self::feeds`] is ([`Function::region`], [`Self::function`],
    /// [`Self::functions_in`]), and BOUNDED at [`MAX_FUNCTIONS`] entries per Region
    /// across at most [`MAX_REGIONS`] Regions: `CreateFunction` refuses past either with
    /// the modelled `ServiceQuotaExceededException`.
    pub functions: BTreeMap<String, Function>,
    /// Every live question set, keyed by its `QuestionSetId`, each holding its DRAFT
    /// version and the versions published from it. `BTreeMap` so `ListQuestionSets` has
    /// one stable order to paginate over. Partitioned by Region exactly as
    /// [`Self::feeds`] is ([`QuestionSet::region`], [`Self::question_set`],
    /// [`Self::question_sets_in`]), and BOUNDED at [`MAX_QUESTION_SETS`] entries per
    /// Region across at most [`MAX_REGIONS`] Regions, each holding at most
    /// [`MAX_QUESTION_SET_VERSIONS`] versions.
    pub question_sets: BTreeMap<String, QuestionSet>,
    /// Monotonic source for every id this emulator mints: one more per resource,
    /// whichever kind it is.
    ///
    /// SHARED by both kinds, because both are drawn from one form
    /// ([`ID_ALPHABET`]): a sequence per kind would hand the first feed and the first
    /// dictionary of a store the same mixed value, and so the same id.
    pub next_id_seq: u64,
    /// Every fixture the external sports data source holds, keyed by its `FixtureId`.
    /// `BTreeMap` so a search has ONE stable order to page over, which is what a
    /// `nextToken` names a position in.
    ///
    /// Filled from [`default_fixture_catalog`] when the store is constructed (see the
    /// module docs) and afterwards only through the in-process seam
    /// ([`Self::seed_fixtures`], [`Self::clear_fixtures`]): `SearchFixtures` and
    /// `GetFixture` are the only operations that touch a fixture and both are
    /// `@readonly`, so no request adds, changes or removes an entry — which is also
    /// what BOUNDS this map, since nothing on the wire can grow it.
    pub fixtures: BTreeMap<String, Fixture>,
    /// The earliest instant at which some record owes the clock a move, when any does.
    /// [`Self::settle_due_transitions`] reads it first and returns without touching
    /// the store while nothing is owed, so the common request path is O(1).
    pub earliest_due: Option<Instant>,
    /// The entropy that distinguishes THIS emulator's ids from another's, drawn once
    /// on the first mint ([`Self::run_seed`]).
    ///
    /// An id identifies exactly one resource, so no two resources may ever carry the
    /// same one. The per-resource sequence alone cannot promise that: it starts over
    /// at the same value in a fresh store, so two stores would hand out the same
    /// first id. Mixing this seed in is what makes a minted id unique across stores
    /// as well as within one.
    pub run_seed: Option<u64>,
}

/// A fresh store: no feeds, no dictionaries — and the data-source catalog already
/// holding this emulator's schedule ([`default_fixture_catalog`]).
///
/// `engine::InMemoryEngine::state` builds a service's state with `Default` the first
/// time a handler reaches it, so this is the ONE construction every store goes through,
/// and it is why the fixture catalog is filled here: no modelled operation creates a
/// fixture (AWS relays them from an external sports data source), so a store that came
/// up holding none could never be given one over the wire, and both `SearchFixtures`
/// and `GetFixture` would answer nothing for the life of the process.
impl Default for ComAmazonawsElementalinferenceElementalinferenceState {
    fn default() -> Self {
        let mut state = Self {
            feeds: BTreeMap::new(),
            dictionaries: BTreeMap::new(),
            functions: BTreeMap::new(),
            question_sets: BTreeMap::new(),
            next_id_seq: 0,
            fixtures: BTreeMap::new(),
            earliest_due: None,
            run_seed: None,
        };
        state.seed_fixtures(default_fixture_catalog(current_epoch_secs()));
        state
    }
}

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// Record that the clock owes a move at `due`, so the next settle pass runs.
    pub fn note_due(&mut self, due: Instant) {
        self.earliest_due = Some(self.earliest_due.map_or(due, |earliest| earliest.min(due)));
    }

    /// The one feed `id` names IN `region`, or `None` when that Region holds no such
    /// feed.
    ///
    /// This service is regional: a feed exists for the Region whose endpoint created it
    /// and for no other, so a lookup is a (Region, id) lookup and an id that names a
    /// feed of a different Region names nothing. Every reporting operation that reaches
    /// a feed by its `FeedId` goes through this (and every mutating one through
    /// [`Self::feed_mut`]), so none of them can disagree about which feeds a Region
    /// holds, or answer for another Region's.
    ///
    /// A feed whose deletion has COMPLETED is not one of them, and needs no exclusion
    /// here: completing the deletion removed the record, so the id resolves to nothing
    /// for EVERY operation — `DeleteFeed` included — exactly as an id this store never
    /// held does.
    pub fn feed(&self, region: &str, id: &str) -> Option<&Feed> {
        self.feeds.get(id).filter(|feed| feed.region == region)
    }

    /// The one feed `id` names in `region`, for a caller that writes it — the mutating
    /// half of [`Self::feed`], with the same Region rule.
    pub fn feed_mut(&mut self, region: &str, id: &str) -> Option<&mut Feed> {
        self.feeds.get_mut(id).filter(|feed| feed.region == region)
    }

    /// Every feed of `region`, in `FeedId` order — what `ListFeeds` pages over, since a
    /// list reports the feeds of the endpoint it was sent to and nothing else.
    pub fn feeds_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Feed> + 'a {
        self.feeds
            .values()
            .filter(move |feed| feed.region == region)
    }

    /// The one dictionary `id` names in `region`, or `None` when that Region holds no
    /// such dictionary — the dictionary's counterpart of [`Self::feed`], with the same
    /// Region rule and the same rule about a completed deletion.
    pub fn dictionary(&self, region: &str, id: &str) -> Option<&Dictionary> {
        self.dictionaries
            .get(id)
            .filter(|dictionary| dictionary.region == region)
    }

    /// The one dictionary `id` names in `region`, for a caller that writes it.
    pub fn dictionary_mut(&mut self, region: &str, id: &str) -> Option<&mut Dictionary> {
        self.dictionaries
            .get_mut(id)
            .filter(|dictionary| dictionary.region == region)
    }

    /// Every dictionary of `region`, in `DictionaryId` order — what `ListDictionaries`
    /// pages over.
    pub fn dictionaries_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a Dictionary> + 'a {
        self.dictionaries
            .values()
            .filter(move |dictionary| dictionary.region == region)
    }

    /// The feed a `CreateFeed` request has ALREADY minted in `region`, if this store
    /// holds it: the one whose stored name, access role, output list and tags are
    /// exactly what the request asks for, and which is otherwise still as that create
    /// left it ([`Feed::is_as_created`]).
    ///
    /// A create of this service carries no `@idempotencyToken`, so the request's own
    /// members are its key. The operation is `@idempotent`, which means the effect of
    /// several identical requests is the effect of ONE: the second and later ones
    /// answer with the resource the first minted instead of minting another, so an SDK
    /// retry of a create whose response was lost cannot double what the Region holds
    /// (nor what it has spent of its quota). Only a difference in the request itself
    /// makes it a different create.
    ///
    /// A record the Region no longer holds, or one a later call has moved on, is not
    /// one a replay can answer with: a feed whose deletion has been accepted or which
    /// has since been associated, released or given a policy is passed over, and a
    /// replay after the removal fell due mints afresh — the recognition is against what
    /// the Region holds NOW, with no index to keep in step with the store.
    ///
    /// Scans at most [`MAX_FEEDS`] feeds of the Region, each with at most
    /// [`MAX_OUTPUTS_PER_FEED`] outputs.
    pub fn feed_created_from<'a>(
        &'a self,
        region: &'a str,
        name: &str,
        access_role_arn: Option<&str>,
        outputs: &[Output],
        tags: &BTreeMap<String, String>,
    ) -> Option<&'a Feed> {
        self.feeds_in(region).find(|feed| {
            feed.is_as_created()
                && feed.name == name
                && feed.access_role_arn.as_deref() == access_role_arn
                && feed.outputs == outputs
                && &feed.tags == tags
        })
    }

    /// The dictionary a `CreateDictionary` request has ALREADY minted in `region`, if
    /// this store holds it — the dictionary's counterpart of [`Self::feed_created_from`],
    /// over the members a dictionary create sets.
    ///
    /// Every member a create of a dictionary states is a stored one, so the comparison
    /// below plus [`Dictionary::is_as_created`] covers the whole record: a dictionary
    /// `UpdateDictionary` or a tag write has moved on no longer matches the request that
    /// minted it, and one whose deletion has been accepted is passed over as a feed's
    /// is. The status the clock itself moves (`CREATING` to `AVAILABLE`) is
    /// not a change to the record, so a replay is recognised on either side of it.
    pub fn dictionary_created_from<'a>(
        &'a self,
        region: &'a str,
        name: &str,
        language: &str,
        entries: &str,
        tags: &BTreeMap<String, String>,
    ) -> Option<&'a Dictionary> {
        self.dictionaries_in(region).find(|dictionary| {
            dictionary.is_as_created()
                && dictionary.name == name
                && dictionary.language == language
                && dictionary.entries == entries
                && &dictionary.tags == tags
        })
    }

    /// The resource a `resourceArn` names IN `region`, if that Region holds it.
    ///
    /// An ARN identifies a resource WHOLE: the kind of resource (`feed` or
    /// `dictionary`), its id, and the region and account it was minted in. So an ARN
    /// resolves only when it is exactly the ARN the stored record carries, and every
    /// other well-formed value — another kind, another id, another region, another
    /// account — names no resource of this store and so resolves to nothing.
    ///
    /// It must ALSO be a resource of the endpoint the request reached, which is what
    /// makes it "the resource identified by the specified resourceArn in the current
    /// region" (`TagResource`, `UntagResource`): the lookup goes through the same
    /// (Region, id) resolution every id-based operation uses ([`Self::feed_mut`],
    /// [`Self::dictionary_mut`]), so an ARN that is a perfectly good ARN of ANOTHER
    /// Region's resource names nothing here, exactly as that resource's bare id does.
    /// The two paths therefore cannot disagree about which resources a Region holds.
    ///
    /// One resolution shared by every operation that reaches a resource by its
    /// `resourceArn`, so none of them can disagree about what an ARN names.
    ///
    /// A record whose deletion has COMPLETED is not a resource this store holds at all:
    /// the ARN of a deleted feed or dictionary resolves here no more than one of a
    /// resource that never existed, so a tagging call about it is refused exactly as
    /// that one is.
    pub fn tagged_resource_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<TaggedResourceMut<'_>> {
        let (kind, id) = arn_kind_and_id(resource_arn)?;
        match kind {
            "feed" => self
                .feed_mut(region, id)
                .filter(|feed| feed.arn == resource_arn)
                .map(TaggedResourceMut::Feed),
            "dictionary" => self
                .dictionary_mut(region, id)
                .filter(|dictionary| dictionary.arn == resource_arn)
                .map(TaggedResourceMut::Dictionary),
            "function" => self
                .function_mut(region, id)
                .filter(|function| function.arn == resource_arn)
                .map(TaggedResourceMut::Function),
            QUESTION_SET_ARN_KIND => self
                .question_set_mut(region, id)
                .filter(|question_set| question_set.arn == resource_arn)
                .map(TaggedResourceMut::QuestionSet),
            _ => None,
        }
    }

    /// The resource a `resourceArn` names IN `region`, for a caller that only READS it —
    /// the reporting half of [`Self::tagged_resource_mut`], resolved by exactly the same
    /// rule and through the same ARN parse, so the reporting and the mutating tagging
    /// paths cannot disagree about what an ARN names.
    ///
    /// A record whose deletion has been ACCEPTED is still a resource this store holds
    /// and so still resolves here: it is a mutation of it that is refused, not a report
    /// of it, exactly as [`Self::feed`] and [`Self::dictionary`] still answer for one.
    /// One whose deletion has COMPLETED is gone from the store and so resolves to
    /// nothing.
    pub fn tagged_resource(&self, region: &str, resource_arn: &str) -> Option<TaggedResource<'_>> {
        let (kind, id) = arn_kind_and_id(resource_arn)?;
        match kind {
            "feed" => self
                .feed(region, id)
                .filter(|feed| feed.arn == resource_arn)
                .map(TaggedResource::Feed),
            "dictionary" => self
                .dictionary(region, id)
                .filter(|dictionary| dictionary.arn == resource_arn)
                .map(TaggedResource::Dictionary),
            "function" => self
                .function(region, id)
                .filter(|function| function.arn == resource_arn)
                .map(TaggedResource::Function),
            QUESTION_SET_ARN_KIND => self
                .question_set(region, id)
                .filter(|question_set| question_set.arn == resource_arn)
                .map(TaggedResource::QuestionSet),
            _ => None,
        }
    }

    /// The ids of the feeds that reference `dictionary_id`, in feed-id order — the
    /// `FeedReferences` list every dictionary response carries.
    ///
    /// DERIVED from the feed store, which is the only place a reference lives: a feed
    /// references a dictionary exactly while one of its outputs' subtitling configs
    /// names that dictionary ([`Feed::references_dictionary`]). So a feed that is
    /// created, reconfigured, associated or deleted changes the dictionaries it
    /// references by that one write, with no second copy to keep in step, and this list
    /// is a faithful view of the feeds `GetFeed`/`ListFeeds` report rather than a
    /// separate opinion about them.
    ///
    /// Scans at most [`MAX_FEEDS`] feeds, each with at most [`MAX_OUTPUTS_PER_FEED`]
    /// outputs.
    ///
    /// Scoped to `region`: an output names a dictionary by its bare `DictionaryId`, and
    /// an id resolves within the Region that holds it, so only the feeds of a
    /// dictionary's OWN Region can reference it.
    pub fn feeds_referencing(&self, region: &str, dictionary_id: &str) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| feed.references_dictionary(dictionary_id))
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// The ids of the feeds that are not already retiring and reference
    /// `dictionary_id`, in feed-id order: the records that make "you cannot delete a
    /// dictionary that is referenced by a feed" bite, and so both WHETHER
    /// `DeleteDictionary` is refused and the count its refusal reports
    /// ([`dictionary_referenced_message`]) — one derivation, so the figure a refusal
    /// states and the fact that refused it cannot disagree.
    ///
    /// A feed whose deletion has been ACCEPTED (`DELETING`) is on its way out and so
    /// holds nothing back: the dictionary it names may go as soon as the feed's own
    /// deletion is accepted, which is what lets a caller tear down a feed and then its
    /// dictionary without waiting for the feed's removal to fall due. It is still a
    /// live record until then, so it stays in [`Self::feeds_referencing`].
    pub fn feeds_blocking_delete(&self, region: &str, dictionary_id: &str) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| {
                feed.status != FeedStatus::Deleting && feed.references_dictionary(dictionary_id)
            })
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// The refusal an output list earns for a REFERENCE that resolves to nothing, or
    /// `None` when every reference it carries resolves.
    ///
    /// An output can name another resource of this service: a subtitling output names a
    /// dictionary ([`dictionary_reference`]) and a clipping output's data source names a
    /// fixture ([`fixture_reference`]). The service maintains that relation in the other
    /// direction already — `GetDictionary$references` reports the feeds that name a
    /// dictionary and `DeleteDictionary` refuses while one does — so an id that resolves
    /// to no record describes a reference the service's integrity rules do not admit,
    /// and the request is refused rather than stored. Every operation that writes
    /// outputs into a feed goes through this ONE scan (`CreateFeed`, `UpdateFeed`,
    /// `AssociateFeed`), so none of them can disagree about which references a stored
    /// feed may carry.
    ///
    /// A dictionary id resolves WITHIN the request's Region, as every id-based lookup of
    /// this regional service does ([`Self::dictionary`]); a fixture is not a resource of
    /// this service but an event the data source holds ([`Self::fixture`]), so its id is
    /// resolved against that catalog. Walks the supplied list once, which every writer
    /// of the list bounds by [`MAX_OUTPUTS_PER_FEED`].
    pub fn unresolved_output_reference<'a>(
        &self,
        region: &str,
        configs: impl IntoIterator<Item = &'a OutputConfig>,
    ) -> Option<String> {
        configs.into_iter().find_map(|config| {
            if let Some(dictionary_id) = dictionary_reference(config)
                .filter(|dictionary_id| self.dictionary(region, dictionary_id).is_none())
            {
                return Some(cannot_find_dictionary_message(dictionary_id));
            }
            if let Some(function_arn) = function_reference(config).filter(|function_arn| {
                arn_kind_and_id(function_arn)
                    .filter(|(kind, _)| *kind == "function")
                    .and_then(|(_, id)| self.function(region, id))
                    .is_none_or(|function| function.arn != *function_arn)
            }) {
                return Some(cannot_find_function_message(function_arn));
            }
            if let Some(message) = question_set_references(config)
                .into_iter()
                .find_map(|reference| self.unresolved_question_set_reference(region, reference))
            {
                return Some(message);
            }
            fixture_reference(config)
                .filter(|fixture_id| self.fixture(fixture_id).is_none())
                .map(cannot_find_fixture_message)
        })
    }

    /// The refusal ONE question set reference earns, or `None` when it resolves.
    ///
    /// A reference names a set and one of its versions, and both have to exist: "the
    /// question set must be in your account" (`QuestionSetReference$questionSetId`) and
    /// "the version must exist" (`QuestionSetReference$version`). The set resolves
    /// WITHIN the request's Region, as every id-based lookup of this regional service
    /// does.
    fn unresolved_question_set_reference(
        &self,
        region: &str,
        reference: &QuestionSetReference,
    ) -> Option<String> {
        let Some(question_set) = self.question_set(region, &reference.question_set_id) else {
            return Some(cannot_find_question_set_message(&reference.question_set_id));
        };
        let holds_version = reference.version == DRAFT_VERSION
            || reference
                .version
                .parse::<u32>()
                .is_ok_and(|version| question_set.version(version).is_some());
        (!holds_version).then(|| {
            cannot_find_question_set_version_message(
                &reference.question_set_id,
                &reference.version,
            )
        })
    }

    /// Materialise every transition the clock already owes at `now`, for feeds and
    /// for dictionaries alike.
    ///
    /// Called at the top of every handler, including the reporting ones: these moves
    /// were due at this instant whether or not anybody called, so applying them on a
    /// read changes nothing a later caller would not have seen anyway.
    ///
    /// A deletion that falls due COMPLETES, and completing it REMOVES the record: the
    /// resource is gone, so from that instant the id names nothing for every operation
    /// of this service, the delete that accepted the deletion included.
    ///
    /// Returns immediately while no move is owed; the scan it then does is over at
    /// most [`MAX_FEEDS`] + [`MAX_DICTIONARIES`] entries.
    pub fn settle_due_transitions(&mut self, now: Instant) {
        if self.earliest_due.is_none_or(|due| now < due) {
            return;
        }
        self.feeds.retain(|_, feed| {
            // The schedule actions this feed owes the clock are applied whatever the
            // feed's own status is doing: their scheduled times fell due at this instant
            // whether or not anybody called.
            feed.settle_due_schedule_actions(now);
            if feed.transition_due_at.is_none_or(|due| now < due) {
                return true;
            }
            match feed.status {
                // The accepted deletion has now completed, so the feed is no longer a
                // resource this store holds: nothing `GetFeed` reports, `ListFeeds`
                // lists, a tagging call reaches or a repeated `DeleteFeed` resolves.
                FeedStatus::Deleting => return false,
                // "After creation of the feed has succeeded, the status will be
                // AVAILABLE" (`CreateFeedResponse$status`): the accepted creation is
                // done, so the feed is serviceable from here on.
                FeedStatus::Creating => {
                    feed.status = FeedStatus::Available;
                    feed.transition_due_at = None;
                }
                // A feed whose reconfiguration has completed goes back into the status
                // it was reconfigured from ([`Feed::settles_into`]): the work suspended
                // that status, it did not replace it.
                FeedStatus::Updating => {
                    if let Some(settled) = feed.settles_into.take() {
                        feed.status = settled;
                    }
                    feed.transition_due_at = None;
                }
                // A settled status owes the clock nothing; clear the stale instant.
                FeedStatus::Available | FeedStatus::Active | FeedStatus::Archived => {
                    feed.transition_due_at = None;
                }
            }
            true
        });
        self.dictionaries.retain(|_, dictionary| {
            if dictionary.transition_due_at.is_none_or(|due| now < due) {
                return true;
            }
            match dictionary.status {
                // The accepted deletion has completed, on the same rule the feed's is:
                // the dictionary leaves the store outright.
                DictionaryStatus::Deleting => return false,
                // "After creation succeeds, the status will be AVAILABLE."
                DictionaryStatus::Creating => {
                    dictionary.status = DictionaryStatus::Available;
                    dictionary.transition_due_at = None;
                }
                // A settled status owes the clock nothing; clear the stale instant.
                DictionaryStatus::Available => {
                    dictionary.transition_due_at = None;
                }
            }
            true
        });
        self.functions.retain(|_, function| {
            if function.transition_due_at.is_none_or(|due| now < due) {
                return true;
            }
            match function.status {
                // The accepted deletion has completed, on the same rule the feed's and
                // the dictionary's are: the function leaves the store outright.
                FunctionLifecycle::Deleting => return false,
                // "The function is ready to use as soon as the request succeeds"
                // (`FunctionStatus`): the accepted creation is done.
                FunctionLifecycle::Creating => {
                    function.status = FunctionLifecycle::Available;
                    function.transition_due_at = None;
                }
                // A settled status owes the clock nothing; clear the stale instant.
                FunctionLifecycle::Available => {
                    function.transition_due_at = None;
                }
            }
            true
        });
        self.earliest_due = self
            .feeds
            .values()
            .filter_map(|feed| feed.transition_due_at)
            // A feed's schedule actions owe the clock their scheduled times and, once
            // settled, the instant they leave the feed, so a store that owes nothing else
            // still wakes for them.
            .chain(self.feeds.values().flat_map(|feed| {
                feed.schedule_actions
                    .iter()
                    .map(|action| action.retire_at.unwrap_or(action.due_at))
            }))
            .chain(
                self.dictionaries
                    .values()
                    .filter_map(|dictionary| dictionary.transition_due_at),
            )
            .chain(
                self.functions
                    .values()
                    .filter_map(|function| function.transition_due_at),
            )
            .min();
    }
}

/// The ARN of a feed in `region`: `arn:aws:elemental-inference:<region>:<account>:feed/<id>`.
pub fn feed_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("feed/{id}"))
}

/// The refusal a role ARN naming another account earns.
///
/// Elemental Inference does not resolve a passed role offline: it evaluates the role by
/// attempting to pass it, and passing a role that lives in another account is denied.
/// The account id is in the ARN the request supplied, so the refusal is derived from
/// the request.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// Whether `arn` names an IAM account id other than the one this emulator serves.
///
/// Reads the account segment of an ARN (`arn:<partition>:iam::<account>:<resource>`,
/// field 5) and answers true only when it is a 12-digit account id that is not
/// `engine::emulator_account_id()`. A value whose account segment is not an account id
/// at all says nothing about WHOSE role it is, so it is not a pass-role denial and is
/// left to the member's own syntax rules.
pub fn names_another_account(arn: &str) -> bool {
    let fields: Vec<&str> = arn.split(':').collect();
    let Some(account) = fields.get(4) else {
        return false;
    };
    let is_account_id =
        account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    is_account_id && *account != ::engine::emulator_account_id()
}

/// The Region this request was signed for — the endpoint it reached, and the Region
/// field of every ARN the response carries.
///
/// The value comes from the caller's own SigV4 credential scope, which is free text in
/// the `Authorization` header, so it is read through the shared Region grammar
/// (`service_common::arn::region_label`, the one place this tree states that grammar:
/// non-empty, at most 20 characters and composed only of `[a-z0-9-]`, the form every
/// model's ARN pattern states for the field — here
/// `…:elemental-inference[a-z\-]*:[a-z0-9\-]+:…`) before it names a Region of this store
/// or is spliced into an ARN. A scope whose region field is no Region label names no
/// Region at all, so such a request is attributed to [`DEFAULT_REGION`] exactly as a
/// request carrying no SigV4 credential scope is. That keeps every ARN this service
/// mints a value the modelled `resourceArn` admits, so the ARN a create hands back is
/// one the tagging operations — whose only handle is a `resourceArn` — can be called
/// with.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .filter(|scope| ::service_common::arn::region_label(&scope.region).is_some())
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// The fixture catalog: the events the external sports data source holds, as this
// emulator holds them, and the window arithmetic a search selects them with
// (see the module docs). The records themselves live in the state struct above.
// ---------------------------------------------------------------------------

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElementalinferenceElementalinferenceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElementalinferenceElementalinferenceState>)
}
