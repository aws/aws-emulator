// SPDX-License-Identifier: CC0-1.0

//! The DICTIONARY record: a named, language-tagged payload of custom words a subtitling
//! output can reference.
//!
//! `CreateDictionary` mints one, `GetDictionary` reports it, `UpdateDictionary` mutates it
//! and `DeleteDictionary` retires it, all on this one record. WHICH feeds reference a
//! dictionary is NOT stored on it: that is derived from the feed store, so one copy of
//! that fact backs the `references` list, the `REFERENCED` status and the delete's
//! refusal alike.

use super::*;

/// The `DictionaryStatus` values this slice of the service produces.
///
/// These are the LIFECYCLE statuses a dictionary record holds. The model's enum also
/// carries `REFERENCED`, which is not a stored value: it is DERIVED from the feeds whose
/// subtitling outputs name the dictionary (see [`Dictionary::reported_status`]). `DELETED`
/// is not one either: a deletion that completes REMOVES the dictionary, so no record is
/// left to report it (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryStatus {
    /// The dictionary has been accepted and is not usable yet.
    Creating,
    /// The dictionary is settled and can be referenced by an output.
    Available,
    /// A `DeleteDictionary` has been accepted; the deletion completes when it falls
    /// due.
    Deleting,
}

impl DictionaryStatus {
    /// The value this status takes on the wire, as spelled by the `DictionaryStatus`
    /// enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Deleting => "DELETING",
        }
    }
}

/// A dictionary: custom words and phrases a subtitling output can reference.
#[derive(Debug, Clone)]
pub struct Dictionary {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region whose endpoint this dictionary was created through, and so the only
    /// one it exists for: the Region of the request that minted it
    /// ([`request_region`]), which is also the Region field of [`Self::arn`].
    pub region: String,
    /// An ISO 639-2/T code from the modelled `DictionaryLanguage` enum.
    pub language: String,
    /// The entry list the dictionary holds, in the canonical document this service
    /// exports it as ([`canonical_entries`]); empty when the request carried no
    /// payload.
    pub entries: String,
    pub status: DictionaryStatus,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this dictionary's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
}

impl Dictionary {

    /// Put this dictionary into a transient status the clock will move on by itself,
    /// and return the instant that move falls due — the dictionary's counterpart of
    /// [`Feed::enter_transient`]. The caller hands that instant to
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::note_due`].
    #[must_use]
    pub fn enter_transient(&mut self, status: DictionaryStatus, now: Instant) -> Instant {
        let due = now + TRANSITION_SPAN;
        self.status = status;
        self.transition_due_at = Some(due);
        due
    }

    /// The `DictionaryStatus` this dictionary reports given `references`, the feeds
    /// that currently reference it.
    ///
    /// A dictionary in a transient lifecycle status reports that status; a settled one
    /// reports `REFERENCED` while any feed's subtitling output names it (which is also
    /// why `DeleteDictionary` refuses it) and `AVAILABLE` while none does.
    pub fn reported_status(&self, references: &[String]) -> &'static str {
        match self.status {
            DictionaryStatus::Available if !references.is_empty() => "REFERENCED",
            status => status.as_wire(),
        }
    }

    /// The tags of this dictionary as the `TagMap` every dictionary response carries.
    ///
    /// A dictionary that carries no tags reports an EMPTY map, not an absent member:
    /// the tag set is a property every dictionary HAS, so a client reading it gets an
    /// empty collection rather than null, and the member does not start existing the
    /// moment the first tag is added.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
    }

    /// Whether this dictionary is still EXACTLY what a `CreateDictionary` left behind,
    /// in the one respect a create does not state — its lifecycle: a dictionary is
    /// minted transient and settles on the clock, so either of those statuses is what a
    /// create produces, while one whose deletion has been accepted is on its way out.
    ///
    /// The dictionary's counterpart of [`Feed::is_as_created`], read by
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::dictionary_created_from`]
    /// for the same reason: every other member a create of a dictionary states is a
    /// stored one, so comparing those members with the request covers the rest of the
    /// record. What it reads is the record's CURRENT content, not its history: a
    /// dictionary a feed's subtitling output has since named is one of these — the
    /// stored members are untouched, only the status it REPORTS is derived
    /// ([`Self::reported_status`]) — so a replay answers with it and reports
    /// `REFERENCED`.
    pub fn is_as_created(&self) -> bool {
        matches!(
            self.status,
            DictionaryStatus::Creating | DictionaryStatus::Available
        )
    }

    /// The `DictionarySummary` projection `ListDictionaries` reports — the dictionary's
    /// counterpart of [`Feed::summary`].
    ///
    /// `references` is the feeds that currently reference this dictionary
    /// ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_referencing`]),
    /// so the summary's `status` is the SAME reported status `GetDictionary` answers
    /// with ([`Self::reported_status`]) and a list and a get cannot disagree about one
    /// stored dictionary.
    pub fn summary(&self, references: &[String]) -> DictionarySummary {
        DictionarySummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            language: self.language.clone(),
            status: self.reported_status(references).to_string(),
        }
    }
}
