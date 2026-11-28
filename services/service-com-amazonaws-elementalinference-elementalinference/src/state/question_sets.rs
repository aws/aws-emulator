// SPDX-License-Identifier: CC0-1.0

//! The QUESTION SET resource: its DRAFT version, the numbered versions published from
//! it, and the hash that identifies each one.
//!
//! `CreateQuestionSet` mints the set "with a DRAFT version that contains the questions
//! in the request", `UpdateQuestionSet` rewrites DRAFT in place,
//! `CreateQuestionSetVersion` publishes DRAFT as the next number, `GetQuestionSet` and
//! `ListQuestionSetVersions` report one or all of them, and `DeleteQuestionSetVersion` /
//! `DeleteQuestionSet` retire a version or the whole set. All of them work on the single
//! [`QuestionSet`] record stored here, so the questions a client reads back at a version
//! are the ones that version was published with and a published version never changes.
//!
//! WHICH feeds reference a version is not stored on the set: it is DERIVED from the feed
//! store
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_referencing_question_set_version`]),
//! exactly as a dictionary's and a function's references are, so one copy of that fact
//! backs a version's `references` list and both deletes' refusals.

use super::*;

use crate::ops::types::{
    ChooseOneQuestion, QuestionSetSummary, QuestionSetVersionSummary, YesNoPerOptionQuestion,
    YesNoQuestion,
};

/// How the DRAFT version is spelled on the wire: the modelled `QuestionSetVersion` is
/// `^(DRAFT|[1-9][0-9]*)$`, so a version is either this word or a positive number.
pub const DRAFT_VERSION: &str = "DRAFT";

/// The questions one version of a question set holds, by type.
///
/// The three lists are the version's whole content: `CreateQuestionSet` stores the
/// request's lists in DRAFT, `UpdateQuestionSet` "replaces questions one type at a time"
/// against them, and a published version keeps the copy taken when it was published.
#[derive(Debug, Clone, Default)]
pub struct QuestionSetContent {
    pub yes_no: Vec<YesNoQuestion>,
    pub yes_no_per_option: Vec<YesNoPerOptionQuestion>,
    pub choose_one: Vec<ChooseOneQuestion>,
}

impl QuestionSetContent {
    /// How many SCORED ITEMS this content counts as: "each yesNo question, each option
    /// of a yesNoPerOption question, and each chooseOne question counts as one item"
    /// (`CreateQuestionSet`).
    pub fn scored_items(&self) -> usize {
        self.yes_no.len()
            + self
                .yes_no_per_option
                .iter()
                .map(|question| question.options.len())
                .sum::<usize>()
            + self.choose_one.len()
    }

    /// Whether this content holds no question at all — "You must specify at least one
    /// question." (`CreateQuestionSet`).
    pub fn is_empty(&self) -> bool {
        self.yes_no.is_empty() && self.yes_no_per_option.is_empty() && self.choose_one.is_empty()
    }

    /// The names of every question in this content, in the order the three lists are
    /// declared in — what the uniqueness rule is read over ("The name of each question
    /// must be unique across all three types").
    pub fn question_names(&self) -> Vec<&str> {
        self.yes_no
            .iter()
            .map(|question| question.name.as_str())
            .chain(
                self.yes_no_per_option
                    .iter()
                    .map(|question| question.name.as_str()),
            )
            .chain(self.choose_one.iter().map(|question| question.name.as_str()))
            .collect()
    }

    /// A list for a response member that is reported only when the version HAS questions
    /// of that type: "Elemental Inference doesn't return this property if the version has
    /// no yesNo questions" (`GetQuestionSetResponse$yesNo`, and likewise for the other
    /// two).
    pub fn reported<T: Clone>(questions: &[T]) -> Option<Vec<T>> {
        (!questions.is_empty()).then(|| questions.to_vec())
    }
}

/// The refusal a version's content earns, or `None` when it satisfies every rule a
/// version is held to.
///
/// The one place the rules of `CreateQuestionSet` are stated, which `UpdateQuestionSet`
/// is held to as well: "The updated DRAFT must follow the same rules as in
/// CreateQuestionSet, and Elemental Inference applies those rules to the questions that
/// DRAFT keeps as well as to the questions in the request." So neither operation can
/// disagree about which content a stored version may hold.
pub fn invalid_question_set_message(content: &QuestionSetContent) -> Option<String> {
    if content.is_empty() {
        return Some(
            "A question set must contain at least one question. Specify at least one of yesNo, \
             yesNoPerOption, and chooseOne, and try again."
                .to_string(),
        );
    }
    let names = content.question_names();
    let mut seen = BTreeSet::new();
    if let Some(duplicate) = names.iter().find(|name| !seen.insert(**name)) {
        return Some(format!(
            "The question name {duplicate} is used more than once. The name of each question \
             must be unique across all three types."
        ));
    }
    let items = content.scored_items();
    (items > MAX_SCORED_ITEMS).then(|| {
        format!(
            "A version can contain up to {MAX_SCORED_ITEMS} scored items, and this one contains \
             {items}. Remove questions or options and try again."
        )
    })
}

/// One published version of a question set: the content it was published with, and the
/// name the set had at that moment.
///
/// "The new version has the same name and questions as DRAFT, and you can't change it"
/// (`CreateQuestionSetVersion`), which is why the name is copied in here rather than read
/// off the set: `GetQuestionSetResponse$name` for a published version is "the name that
/// the question set had when the version was published".
#[derive(Debug, Clone)]
pub struct PublishedQuestionSetVersion {
    pub name: String,
    pub content: QuestionSetContent,
    pub description: Option<String>,
    /// The hash of this version ([`question_set_sha256`]), computed when it was
    /// published and never recomputed: a published version cannot change, so neither can
    /// its hash.
    pub sha256: String,
    /// When this version was published, in epoch seconds —
    /// `QuestionSetVersionSummary$lastModifiedAt`: "for a published version, the time
    /// when you published it".
    pub published_at: i64,
}

/// A question set: its DRAFT version, every published version, and the tags on the set.
#[derive(Debug, Clone)]
pub struct QuestionSet {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region whose endpoint this question set was created through, and so the only
    /// one it exists for — the Region field of [`Self::arn`].
    pub region: String,
    /// The DRAFT version's questions. `UpdateQuestionSet` rewrites them, and a feed that
    /// references DRAFT "uses the updated questions".
    pub draft: QuestionSetContent,
    /// When DRAFT was last written, in epoch seconds —
    /// `QuestionSetVersionSummary$lastModifiedAt`: "for DRAFT, the time when you created
    /// the question set or last updated DRAFT".
    pub draft_last_modified_at: i64,
    /// The published versions this set still holds, by version number. `BTreeMap` so
    /// `ListQuestionSetVersions` has one stable, ascending order to page over, and
    /// BOUNDED at [`MAX_QUESTION_SET_VERSIONS`] entries.
    pub versions: BTreeMap<u32, PublishedQuestionSetVersion>,
    /// The next version number to publish. "Version numbers start at 1, and each new
    /// version has the next number. Elemental Inference never reuses a version number,
    /// even after you delete that version." (`CreateQuestionSetVersion`), so this only
    /// ever grows.
    pub next_version: u32,
    /// The highest version ever published, "even if you have since deleted that version"
    /// (`GetQuestionSetResponse$latestVersion`), and `None` while none has been
    /// published. Kept beside [`Self::versions`] because deleting a version "doesn't
    /// change the latestVersion of the question set, even if you delete the highest
    /// version" (`DeleteQuestionSetVersion`), so it cannot be derived from what the set
    /// still holds.
    pub latest_version: Option<u32>,
    pub tags: BTreeMap<String, String>,
}

impl QuestionSet {
    /// The hash of the DRAFT version ([`question_set_sha256`]), computed from the name
    /// and questions DRAFT holds NOW: DRAFT changes, so its hash is derived on every read
    /// rather than stored, and a client can hand it back to
    /// `CreateQuestionSetVersion$sha256`.
    pub fn draft_sha256(&self) -> String {
        question_set_sha256(&self.name, &self.draft)
    }

    /// The `latestVersion` member every question set response carries, or `None` while
    /// nothing has been published.
    pub fn reported_latest_version(&self) -> Option<String> {
        self.latest_version.map(|version| version.to_string())
    }

    /// The tags of this question set as the `TagMap` every response carries; an untagged
    /// set reports an EMPTY map rather than an absent member, as [`Dictionary::tag_map`]
    /// does.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
    }

    /// The published version `version` names, or `None` when this set holds no such
    /// version.
    pub fn version(&self, version: u32) -> Option<&PublishedQuestionSetVersion> {
        self.versions.get(&version)
    }

    /// The `QuestionSetSummary` projection `ListQuestionSets` reports. The response
    /// "doesn't include the questions in each question set" (`ListQuestionSets`), so the
    /// summary carries the set's identity and its latest version alone.
    pub fn summary(&self) -> QuestionSetSummary {
        QuestionSetSummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            latest_version: self.reported_latest_version(),
        }
    }

    /// Every version of this set as `ListQuestionSetVersions` reports them: "The list
    /// starts with the DRAFT version, followed by the published versions from the highest
    /// version number to the lowest. The list doesn't include deleted versions."
    /// (`ListQuestionSetVersions`) — so DRAFT first, then the published versions this set
    /// still holds, in DESCENDING number order.
    pub fn version_summaries(&self) -> Vec<QuestionSetVersionSummary> {
        let draft = QuestionSetVersionSummary {
            version: DRAFT_VERSION.to_string(),
            // "Elemental Inference doesn't return this property for DRAFT"
            // (`QuestionSetVersionSummary$description`).
            description: None,
            sha256: self.draft_sha256(),
            last_modified_at: ::aws_smithy_types::DateTime::from_secs(
                self.draft_last_modified_at,
            ),
        };
        // `BTreeMap` walks its keys ascending, so the published versions are reversed to
        // the highest-to-lowest order the operation reports them in.
        let published = self.versions.iter().rev().map(|(number, version)| {
            QuestionSetVersionSummary {
                version: number.to_string(),
                description: version.description.clone(),
                sha256: version.sha256.clone(),
                last_modified_at: ::aws_smithy_types::DateTime::from_secs(version.published_at),
            }
        });
        std::iter::once(draft).chain(published).collect()
    }
}

/// The SHA-256 hash of one version of a question set, as the 64 lowercase hexadecimal
/// characters the modelled `QuestionSetSha256` is.
///
/// "The hash covers the name of the question set and its questions at this version"
/// (`GetQuestionSetResponse$sha256`), so it is computed over exactly those: the name,
/// then each question of each type in order, each field length-delimited so no two
/// different contents can serialise to the same bytes. One function for DRAFT and for a
/// published version alike, so a client that publishes the hash a read reported is
/// publishing the content that read described.
pub fn question_set_sha256(name: &str, content: &QuestionSetContent) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    absorb(&mut hasher, name);
    for question in &content.yes_no {
        absorb(&mut hasher, "yesNo");
        absorb(&mut hasher, &question.name);
        absorb(&mut hasher, &question.question);
    }
    for question in &content.yes_no_per_option {
        absorb(&mut hasher, "yesNoPerOption");
        absorb(&mut hasher, &question.name);
        absorb(&mut hasher, &question.question);
        for option in &question.options {
            absorb(&mut hasher, &option.name);
            absorb(&mut hasher, &option.text);
        }
    }
    for question in &content.choose_one {
        absorb(&mut hasher, "chooseOne");
        absorb(&mut hasher, &question.name);
        absorb(&mut hasher, &question.question);
        for option in &question.options {
            absorb(&mut hasher, &option.name);
            absorb(&mut hasher, &option.text);
        }
    }
    ::hex::encode(hasher.finalize())
}

/// Absorb one field into the hash, preceded by its byte length, so the boundary between
/// two fields cannot be moved by their contents.
fn absorb(hasher: &mut sha2::Sha256, field: &str) {
    use sha2::Digest;

    hasher.update((field.len() as u64).to_be_bytes());
    hasher.update(field.as_bytes());
}

/// The ARN of a question set in `region`:
/// `arn:aws:elemental-inference:<region>:<account>:question-set/<id>`.
pub fn question_set_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("question-set/{id}"))
}

/// The resource segment a question set's ARN carries, which is also the kind an
/// ARN-based lookup matches on.
pub const QUESTION_SET_ARN_KIND: &str = "question-set";

/// The question set versions an output config references, if it references any.
///
/// Only a contextual-metadata output can, and only through its custom questions:
/// `CustomQuestionsConfig` holds "the question sets that you ask Elemental Inference
/// about each shot, each scene, or both", each naming a set and one of its versions.
pub fn question_set_references(config: &OutputConfig) -> Vec<&QuestionSetReference> {
    match config {
        OutputConfig::contextualMetadata(contextual_metadata) => contextual_metadata
            .custom_questions
            .as_ref()
            .map(|custom| {
                custom
                    .shot
                    .iter()
                    .chain(custom.scene.iter())
                    .collect::<Vec<&QuestionSetReference>>()
            })
            .unwrap_or_default(),
        OutputConfig::Cropping(_)
        | OutputConfig::Clipping(_)
        | OutputConfig::Subtitling(_)
        | OutputConfig::Function(_) => Vec::new(),
    }
}

/// A `questionSetId` that names no question set of the request's Region, named by an
/// output's custom questions — the question set's counterpart of
/// [`cannot_find_dictionary_message`], built from the identifier the request supplied.
///
/// "The question set must be in your account" (`QuestionSetReference$questionSetId`).
pub fn cannot_find_question_set_message(question_set_id: &str) -> String {
    format!(
        "Can't find question set {question_set_id}. Use a different questionSetId and try again."
    )
}

/// A `version` the named question set does not hold — "The version must exist"
/// (`QuestionSetReference$version`), so a reference to a version that resolves to nothing
/// is refused rather than stored.
pub fn cannot_find_question_set_version_message(question_set_id: &str, version: &str) -> String {
    format!(
        "Can't find version {version} of question set {question_set_id}. Use a different version \
         and try again."
    )
}

/// The refusal a delete of a question set, or of one of its versions, earns while a feed
/// references it.
///
/// "You can't delete a question set while any feed references any of its versions,
/// including an archived feed. You must first remove the question set from the
/// customQuestions configuration of each feed output that references it, or delete those
/// feeds." (`DeleteQuestionSet`), and the same rule per version for
/// `DeleteQuestionSetVersion`. The sentence reports the NUMBER of feeds the refusal was
/// decided from and names none of them.
pub fn question_set_referenced_message(reference_count: usize) -> String {
    format!(
        "This question set is referenced by {reference_count} feed(s). Remove the question set \
         from the customQuestions configuration of each feed output that references it, or \
         delete those feeds, and try again."
    )
}

/// The refusal `CreateQuestionSetVersion` answers for a `sha256` that is not the hash of
/// the DRAFT it would publish.
///
/// The member is the hash "you can specify in a CreateQuestionSetVersion request"
/// (`GetQuestionSetResponse$sha256`), so a request that states one is asking to publish
/// the DRAFT it last read: a DRAFT that has moved on since is not that content, and
/// publishing it would publish something the caller never saw.
pub fn question_set_sha256_mismatch_message(expected: &str) -> String {
    format!(
        "The DRAFT version of this question set has the SHA-256 hash {expected}. Read the \
         question set again and retry with the hash of the DRAFT you want to publish."
    )
}

/// The refusal `CreateQuestionSetVersion` answers once a question set holds the most
/// versions one may hold.
pub fn too_many_question_set_versions_message() -> String {
    format!(
        "This question set already holds the maximum of {MAX_QUESTION_SET_VERSIONS} versions. \
         Delete a version you no longer need, or create a new question set, and try again."
    )
}

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// Mint a fresh `QuestionSetId`, through the same seed-plus-sequence construction as
    /// every other id of this store (`QuestionSetId`'s pattern is `^[a-z0-9]{19}$`).
    pub fn mint_question_set_id(&mut self) -> String {
        self.mint_id()
    }

    /// The one question set `id` names IN `region`, or `None` when that Region holds no
    /// such set — the question set's counterpart of [`Self::dictionary`], with the same
    /// Region rule.
    pub fn question_set(&self, region: &str, id: &str) -> Option<&QuestionSet> {
        self.question_sets
            .get(id)
            .filter(|question_set| question_set.region == region)
    }

    /// The one question set `id` names in `region`, for a caller that writes it.
    pub fn question_set_mut(&mut self, region: &str, id: &str) -> Option<&mut QuestionSet> {
        self.question_sets
            .get_mut(id)
            .filter(|question_set| question_set.region == region)
    }

    /// Every question set of `region`, in `QuestionSetId` order — what
    /// `ListQuestionSets` pages over.
    pub fn question_sets_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a QuestionSet> + 'a {
        self.question_sets
            .values()
            .filter(move |question_set| question_set.region == region)
    }

    /// The refusal `CreateQuestionSet` answers in `region`, or `None` while that Region
    /// may take one more question set — on the one rule every create of this service
    /// follows ([`create_quota_refusal`]), against the question set store and
    /// [`MAX_QUESTION_SETS`].
    pub fn question_set_quota_refusal(&self, region: &str) -> Option<String> {
        self.quota_refusal(
            self.question_sets
                .values()
                .map(|question_set| question_set.region.as_str()),
            region,
            MAX_QUESTION_SETS,
            "question sets",
        )
    }

    /// The question set a `CreateQuestionSet` request has ALREADY minted in `region`, if
    /// this store holds it — the question set's counterpart of
    /// [`Self::dictionary_created_from`].
    ///
    /// A create carries no `@idempotencyToken`, so the request's own members are its key:
    /// the name, the questions (compared through the hash that covers exactly those two,
    /// [`question_set_sha256`]) and the tags. A set some later call has moved on is not
    /// one a replay can answer with, so one whose DRAFT has been updated or which has
    /// published a version is passed over.
    pub fn question_set_created_from<'a>(
        &'a self,
        region: &'a str,
        name: &str,
        draft_sha256: &str,
        tags: &BTreeMap<String, String>,
    ) -> Option<&'a QuestionSet> {
        self.question_sets_in(region).find(|question_set| {
            question_set.name == name
                && question_set.latest_version.is_none()
                && question_set.draft_sha256() == draft_sha256
                && &question_set.tags == tags
        })
    }

    /// The ids of the feeds that reference ONE version of `question_set_id`, in feed-id
    /// order — the `FeedReferences` list a question set response carries, which "doesn't
    /// include feeds that reference other versions"
    /// (`GetQuestionSetResponse$references`).
    pub fn feeds_referencing_question_set_version(
        &self,
        region: &str,
        question_set_id: &str,
        version: &str,
    ) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| feed.references_question_set_version(question_set_id, version))
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// The ids of the feeds that reference ANY version of `question_set_id`, in feed-id
    /// order — what `DeleteQuestionSet` is refused by: "you can't delete a question set
    /// while any feed references any of its versions".
    pub fn feeds_referencing_question_set(
        &self,
        region: &str,
        question_set_id: &str,
    ) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| feed.references_question_set(question_set_id))
            .map(|feed| feed.id.clone())
            .collect()
    }
}
