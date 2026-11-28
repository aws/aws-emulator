// SPDX-License-Identifier: CC0-1.0

//! The rules an OUTPUT LIST a request supplies is held to, and the references an output's
//! configuration names.
//!
//! `CreateFeed`, `UpdateFeed` and `AssociateFeed` all write a feed's outputs, so which
//! lists a stored feed may carry — its bound, its duplicate names, its appendability —
//! is stated once here ([`output_list_refusal`], [`appended_output_list_refusal`]) and
//! none of the three can disagree. The reference readers beside them
//! ([`dictionary_reference`], [`fixture_reference`]) are likewise the one definition of
//! which other resources an output config names.

use super::*;

/// `SearchFixtures` with `endDate` before `startDate`: the rule is the ORDER of the two
/// members, so the recorded refusal names the members and not the dates they carried.
pub const END_DATE_BEFORE_START_DATE_MESSAGE: &str = "endDate must not be earlier than startDate.";

/// The first name `names` yields twice, if any.
///
/// An output's `name` IDENTIFIES it within its feed: it is how `AssociateFeed` decides
/// that an appended output would collide with one the feed holds, and how a caller
/// reading a feed tells one output from another. So a request that lists one name twice
/// describes a feed in which an output is not addressable at all, and every operation
/// that supplies a feed's output list goes through this one scan — `CreateFeed` and
/// `UpdateFeed`, which REPLACE the list, and `AssociateFeed`, which appends to it — so
/// none of them can disagree about what a feed's outputs may look like.
pub fn first_duplicate_output_name<'a>(
    mut names: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut seen = std::collections::BTreeSet::new();
    names.find(|name| !seen.insert(*name))
}

/// A request that lists more than one output under `name`: the rule is about ONE value's
/// own validity (the name that is not unique), so the message names the value the
/// REQUEST supplied, and the same sentence answers whichever operation carried the list.
pub fn duplicate_output_name_message(name: &str) -> String {
    format!(
        "The request lists more than one output named {name}. \
         Output names must be unique within a feed."
    )
}

/// An APPEND that asks a feed to hold an output under a name it already holds under a
/// DIFFERENT definition ([`Feed::outputs_to_append`]).
///
/// The other half of the rule [`first_duplicate_output_name`] states — an output's `name`
/// IDENTIFIES it within its feed — so it is worded here, beside its sibling and in the
/// same convention, rather than built in the handler: appending that output would MODIFY
/// the one the feed holds, which the append cannot do ("You can't modify an existing
/// output (for example to change its name). Instead, use UpdateFeed"). The sentence is
/// the rule's, so it carries no value read out of the stored record — not the feed, not
/// its status, and nothing of the held output's definition — and names at most the output
/// name the REQUEST supplied, exactly as [`duplicate_output_name_message`] does: the same
/// text whichever feed refused the append and whatever that feed holds.
pub fn output_not_appendable_message(name: &str) -> String {
    format!(
        "The feed already has an output named {name}. AssociateFeed can only append new \
         outputs; use UpdateFeed to modify an existing one."
    )
}

/// An `UpdateFeed` output whose `fromAssociation` disagrees with the value the service
/// holds for the output of that name.
///
/// `UpdateOutput$fromAssociation` is set by the service — true for an output
/// `AssociateFeed` appended, false for one a create or an update introduced — and the
/// member exists so a caller can echo the value it read back; the request may not change
/// it. The violated rule is about ONE value's own validity, so the message names the
/// value the request supplied and the member it sat on, with the list position (1-based,
/// as a validation message indexes a list) rather than the stored value, which is not
/// the request's to know from the refusal.
pub fn from_association_not_changeable_message(position: usize, supplied: bool) -> String {
    format!(
        "1 validation error detected: Value '{supplied}' at \
         'outputs.{position}.member.fromAssociation' failed to satisfy constraint: Member is \
         set by Elemental Inference and must not be changed. Use GetFeed to read its current \
         value."
    )
}

/// The `OutputStatus` value a disabled output reports — one of the two values that
/// enum admits (`OUTPUT_STATUS_VALUES`: `DISABLED`, `ENABLED`).
pub const OUTPUT_STATUS_DISABLED: &str = "DISABLED";

/// Whether a feed LEFT HOLDING `count` outputs would be past [`MAX_OUTPUTS_PER_FEED`].
///
/// The bound is ONE rule about the list a feed ends up holding, so it is measured in one
/// place for every operation that carries such a list: what the carriers differ in is
/// which list the feed is left holding — the request's own, for the operations that
/// REPLACE it ([`output_list_refusal`]), the stored one plus what the call really adds,
/// for the one that APPENDS to it ([`appended_output_list_refusal`]) — never the figure
/// they compare it against.
pub(super) fn exceeds_max_outputs_per_feed(count: usize) -> bool {
    count > MAX_OUTPUTS_PER_FEED
}

/// A request whose output list is longer than [`MAX_OUTPUTS_PER_FEED`], for the
/// operations that SET the list (`CreateFeed`, `UpdateFeed`).
///
/// The quota is the rule, so the message states the quota and nothing about the request
/// or the stored feed — exactly as the append's own refusal does.
pub fn too_many_outputs_message() -> String {
    format!(
        "A feed holds at most {MAX_OUTPUTS_PER_FEED} outputs. Specify fewer outputs or \
         request a quota increase."
    )
}

/// A request that would carry a feed past [`MAX_OUTPUTS_PER_FEED`] by APPENDING to the
/// list it already holds (`AssociateFeed`).
///
/// The same bound as [`too_many_outputs_message`], stating the same quota and, like it,
/// nothing about the request or the stored feed — not even how many of the bound the feed
/// is already using. What differs is the remedy the sentence names: the caller of an
/// append cannot shorten a list it did not supply, and removes an output the feed holds
/// with `UpdateFeed` instead. The figure comes from the one constant either way, so the
/// two sentences cannot come to state different bounds.
pub fn too_many_appended_outputs_message() -> String {
    format!(
        "You can't add these outputs because a feed holds at most {MAX_OUTPUTS_PER_FEED} \
         outputs. Remove outputs with UpdateFeed or request a quota increase."
    )
}

/// A request whose output list would leave a feed holding NO output at all.
///
/// "The key contents of the feed is an array of outputs", and a create "must specify at
/// least one output" (`CreateFeedRequest$outputs`), so a feed always holds one: the
/// operations that SET the feed's whole list — `CreateFeed`, which mints it, and
/// `UpdateFeed`, whose list is the one the feed is left holding — refuse an empty list,
/// because the feed it describes holds nothing. An APPEND is not one of them: it leaves
/// the feed holding what it already holds, so a list that adds nothing takes nothing
/// away either (the rules an append IS held to are [`appended_output_list_refusal`]). The
/// rule is about the shape of the request rather than about any one value it carried, so
/// the message is the rule's invariant sentence and interpolates nothing.
pub const MISSING_OUTPUTS_MESSAGE: &str = "You must specify at least one output.";

/// Why a supplied output list is not one a feed may hold.
///
/// An operation whose request carries a feed's output list holds it to three rules: the
/// list leaves the feed holding at least one output, an output's name identifies it
/// within the list, and the feed is left holding no more than [`MAX_OUTPUTS_PER_FEED`]
/// outputs. Which rules are checked, in which order, and which message and modelled error
/// class each failure earns are stated ONCE here, so no two operations that carry such a
/// list can come to disagree about what a feed's outputs may look like — the operations
/// that REPLACE the list through [`output_list_refusal`] and the one that APPENDS to it
/// through [`appended_output_list_refusal`]. A handler only lifts the outcome into its
/// own modelled error enum, which is per-operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputListRefusal<'a> {
    /// The list is EMPTY and would be the feed's WHOLE list, so it describes a feed
    /// holding no output ([`MISSING_OUTPUTS_MESSAGE`]). Reported as the modelled
    /// `ValidationException` (HTTP 400).
    MissingOutputs,
    /// The list names one output twice, under the name carried here — the first one it
    /// repeats ([`first_duplicate_output_name`]). Reported as the modelled
    /// `ConflictException` (HTTP 409).
    DuplicateName(&'a str),
    /// The list is the feed's WHOLE list and is longer than [`MAX_OUTPUTS_PER_FEED`]
    /// ([`too_many_outputs_message`]). Reported as the modelled
    /// `ServiceQuotaExceededException` (HTTP 402).
    TooManyOutputs,
    /// The list ADDS to the one the feed holds, and together they are longer than
    /// [`MAX_OUTPUTS_PER_FEED`] ([`too_many_appended_outputs_message`]). The same bound as
    /// [`Self::TooManyOutputs`] and the same modelled
    /// `ServiceQuotaExceededException` (HTTP 402); what the two carriers differ in is the
    /// remedy their own operation offers the caller.
    TooManyAppendedOutputs,
}

impl OutputListRefusal<'_> {
    /// The message this refusal is reported with: the one sentence each of the three
    /// rules has, whichever operation supplied the list.
    pub fn message(self) -> String {
        match self {
            Self::MissingOutputs => MISSING_OUTPUTS_MESSAGE.to_string(),
            Self::DuplicateName(name) => duplicate_output_name_message(name),
            Self::TooManyOutputs => too_many_outputs_message(),
            Self::TooManyAppendedOutputs => too_many_appended_outputs_message(),
        }
    }
}

/// Which refusal, if any, an output list that REPLACES a feed's whole stored list earns.
///
/// The operations that replace the list are `CreateFeed`, which mints it, and
/// `UpdateFeed`, whose list is the one the feed is left holding, so the list the REQUEST
/// carries is the list the feed ends up with: all three rules are decided from it alone.
/// The order below is the order they are decided in: the SHAPE of the list first (a list
/// carrying no output describes a feed holding none, whatever else the request says), then
/// a name that is not unique (a property of the request's own values, which an empty list
/// cannot carry), then the bound on the list the feed would be left holding, which for a
/// replacement is the request's own length. `names` is only walked, so a handler can run
/// this before it takes the store and still owns the outputs it goes on to store.
///
/// An APPEND (`AssociateFeed`) is held to its own two of the three, stated in
/// [`appended_output_list_refusal`].
pub fn output_list_refusal<'a>(
    names: impl Iterator<Item = &'a str> + Clone,
) -> Option<OutputListRefusal<'a>> {
    if names.clone().next().is_none() {
        return Some(OutputListRefusal::MissingOutputs);
    }
    if let Some(name) = first_duplicate_output_name(names.clone()) {
        return Some(OutputListRefusal::DuplicateName(name));
    }
    if exceeds_max_outputs_per_feed(names.count()) {
        return Some(OutputListRefusal::TooManyOutputs);
    }
    None
}

/// Which refusal, if any, an output list APPENDED to a feed's stored list earns.
///
/// The one operation that appends is `AssociateFeed`, whose outputs "supplement any
/// outputs that you specified when you created or updated the feed": the feed is left
/// holding what it already holds PLUS the supplied list, so of the three rules above it is
/// held to two. The name rule is one of them — through the very scan a replacement goes
/// through ([`first_duplicate_output_name`]), because an output's name identifies it
/// whichever operation supplied it. The bound is the other, measured against what the feed
/// would be LEFT holding: `held` plus what the call really adds. The list's cardinality is
/// NOT a rule here: a list that adds no output leaves the feed holding the outputs it
/// already held — at least one, which the create made sure of — so it is served, which is
/// also why a list that adds nothing is never past the bound.
///
/// An append decides the two from different things, so it reaches them as its inputs
/// arrive: `held` is the stored count once the feed is resolved, and `None` while it is
/// not — with `None` only the rules the supplied names settle are decided, so a request
/// naming a feed this Region does not hold still earns the not-found refusal rather than
/// one about the list. `names` is only walked, so a handler can run this and still own the
/// outputs it goes on to store.
pub fn appended_output_list_refusal<'a>(
    held: Option<usize>,
    names: impl Iterator<Item = &'a str> + Clone,
) -> Option<OutputListRefusal<'a>> {
    if let Some(name) = first_duplicate_output_name(names.clone()) {
        return Some(OutputListRefusal::DuplicateName(name));
    }
    let appended = names.count();
    if appended > 0 && held.is_some_and(|held| exceeds_max_outputs_per_feed(held + appended)) {
        return Some(OutputListRefusal::TooManyAppendedOutputs);
    }
    None
}

/// A `FixtureId` the data source does not hold.
///
/// One sentence for every operation that resolves a fixture id — `GetFixture`, and the
/// operations that store an output whose clipping config names one — so none of them can
/// disagree about how the service words a fixture id that resolves to nothing. The rule
/// is about ONE value the request supplied, so the message is built from the request and
/// names that identifier.
pub fn cannot_find_fixture_message(fixture_id: &str) -> String {
    format!("Can't find fixture {fixture_id}. Use a different fixtureId and try again.")
}

/// A `DictionaryId` no dictionary of the request's Region holds, named by an output's
/// subtitling config — the dictionary's counterpart of [`cannot_find_fixture_message`],
/// and likewise built from the identifier the request supplied.
pub fn cannot_find_dictionary_message(dictionary_id: &str) -> String {
    format!("Can't find dictionary {dictionary_id}. Use a different dictionary and try again.")
}

/// An output name the feed does not hold, named by a media data-plane request — the
/// output's counterpart of [`cannot_find_dictionary_message`], and likewise built from
/// the name the request supplied.
///
/// A metadata query is for the metadata of ONE of the feed's outputs, so a name that is
/// not one of them names nothing the feed has metadata for, and the query is refused
/// rather than answered for an output that does not exist.
pub fn cannot_find_output_message(output_name: &str) -> String {
    format!("Can't find output {output_name} in the feed. Use a different outputName and try again.")
}

/// The id of the dictionary an output config references, if it references one.
///
/// Only a subtitling output can: `SubtitlingConfig$dictionary` is "The ID of a custom
/// dictionary … Use the CreateDictionary operation to create a dictionary", and no other
/// member of the `OutputConfig` union names a dictionary.
pub fn dictionary_reference(config: &OutputConfig) -> Option<&str> {
    match config {
        OutputConfig::Subtitling(subtitling) => subtitling.dictionary.as_deref(),
        OutputConfig::Cropping(_)
        | OutputConfig::Clipping(_)
        | OutputConfig::contextualMetadata(_)
        | OutputConfig::Function(_) => None,
    }
}

/// The id of the fixture an output config references, if it references one.
///
/// Only a clipping output can, and only when it carries the optional data source:
/// `DataSourceConfiguration$fixtureId` is "The ID of the fixture whose event data you
/// want Elemental Inference to map onto this clipping output", obtained from a
/// `SearchFixtures` result.
pub fn fixture_reference(config: &OutputConfig) -> Option<&str> {
    match config {
        OutputConfig::Clipping(clipping) => clipping
            .data_source_configuration
            .as_ref()
            .map(|data_source| data_source.fixture_id.as_str()),
        OutputConfig::Cropping(_)
        | OutputConfig::Subtitling(_)
        | OutputConfig::contextualMetadata(_)
        | OutputConfig::Function(_) => None,
    }
}
