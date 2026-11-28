// SPDX-License-Identifier: CC0-1.0

//! How a `resourceArn` RESOLVES to a resource of this store, for the tagging operations.
//!
//! An ARN identifies a resource whole — its kind, its id, its Region and its account — so
//! it resolves only when it is exactly the ARN the stored record carries AND that record
//! belongs to the endpoint the request reached. The reporting and the mutating halves go
//! through the one parse here, so the two tagging paths cannot disagree about what an ARN
//! names.

use super::*;

/// The resource KIND and id a `resourceArn` names, or `None` when the value carries no
/// resource segment of the form a resource of this service has.
///
/// `arn:<partition>:<service>:<region>:<account>:<kind>/<id>` — field 5 is the resource
/// segment, which the modelled `resourceArn` pattern spells
/// `(feed|dictionary)/[a-zA-Z0-9]{1,19}`. One parse shared by every ARN-based lookup of
/// this store ([`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource`],
/// [`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource_mut`]), so
/// reading and writing a resource's tags read the same fields out of the same ARN.
pub(super) fn arn_kind_and_id(resource_arn: &str) -> Option<(&str, &str)> {
    resource_arn.split(':').nth(5)?.split_once('/')
}

/// A resource one of this service's tagging operations has reached by its
/// `resourceArn`, borrowed out of the store it lives in
/// ([`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource`]), for a
/// caller that only reports it.
///
/// The reporting half of [`TaggedResourceMut`]: tags are a property of the RESOURCE,
/// stored on the one record every other handler reads ([`Feed::tags`],
/// [`Dictionary::tags`]), so the tag set `ListTagsForResource` reports is the one
/// `GetFeed`/`GetDictionary` report and `TagResource`/`UntagResource` wrote.
#[derive(Debug)]
pub enum TaggedResource<'a> {
    Feed(&'a Feed),
    Dictionary(&'a Dictionary),
    Function(&'a Function),
    QuestionSet(&'a QuestionSet),
}

impl TaggedResource<'_> {
    /// The tags stored on this resource, as the `TagMap` a tagging response carries —
    /// the same projection the resource's own reader reports ([`Feed::tag_map`],
    /// [`Dictionary::tag_map`]), so an untagged resource reports an EMPTY map rather
    /// than an absent member.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        match self {
            Self::Feed(feed) => feed.tag_map(),
            Self::Dictionary(dictionary) => dictionary.tag_map(),
            Self::Function(function) => function.tag_map(),
            Self::QuestionSet(question_set) => question_set.tag_map(),
        }
    }
}

/// A resource one of this service's tagging operations has reached by its
/// `resourceArn`, borrowed mutably out of the store it lives in
/// ([`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource_mut`]).
///
/// Tags are a property of the RESOURCE, stored on the one record every other handler
/// reads ([`Feed::tags`], [`Dictionary::tags`]) — there is no second tag store — so a
/// tag written through `TagResource` is the tag `GetFeed`/`GetDictionary` reports. This
/// enum is what lets the tagging operations work on either kind of resource without
/// either of them holding its own copy of which kinds exist or where a kind keeps its
/// tags.
#[derive(Debug)]
pub enum TaggedResourceMut<'a> {
    Feed(&'a mut Feed),
    Dictionary(&'a mut Dictionary),
    Function(&'a mut Function),
    QuestionSet(&'a mut QuestionSet),
}

impl TaggedResourceMut<'_> {
    /// Whether this resource's deletion has already been accepted, so it is on its way
    /// out of the store.
    pub fn is_being_deleted(&self) -> bool {
        match self {
            Self::Feed(feed) => feed.status == FeedStatus::Deleting,
            Self::Dictionary(dictionary) => dictionary.status == DictionaryStatus::Deleting,
            Self::Function(function) => function.status == FunctionLifecycle::Deleting,
            // A question set has no transient lifecycle: `DeleteQuestionSet` removes it
            // when it answers, so there is no window in which a set is on its way out.
            Self::QuestionSet(_) => false,
        }
    }

    /// The sentence a mutation of this resource earns while its deletion is pending —
    /// the same rule, and the same wording, that `UpdateFeed` and `UpdateDictionary`
    /// already refuse with ([`FEED_BEING_DELETED_MESSAGE`],
    /// [`DICTIONARY_BEING_DELETED_MESSAGE`]).
    pub fn being_deleted_message(&self) -> &'static str {
        match self {
            Self::Feed(_) => FEED_BEING_DELETED_MESSAGE,
            Self::Dictionary(_) => DICTIONARY_BEING_DELETED_MESSAGE,
            Self::Function(_) | Self::QuestionSet(_) => FUNCTION_BEING_DELETED_MESSAGE,
        }
    }

    /// The tags stored on this resource, for a caller that writes them.
    pub fn tags_mut(&mut self) -> &mut BTreeMap<String, String> {
        match self {
            Self::Feed(feed) => &mut feed.tags,
            Self::Dictionary(dictionary) => &mut dictionary.tags,
            Self::Function(function) => &mut function.tags,
            Self::QuestionSet(question_set) => &mut question_set.tags,
        }
    }
}
