// SPDX-License-Identifier: CC0-1.0

//! @op DetectTargetedSentiment
//!
//! IMPLEMENTED. The analysis runs in-process: the document is segmented into words
//! and sentences by the scanner this file states on [`segment`], mentions are
//! placed by the rule-based recogniser below, mentions that name the same entity are
//! grouped, and each mention's sentiment comes from the VADER sentiment engine
//! applied to the CLAUSE the mention sits in. Every reported offset, surface form,
//! type and score is therefore computed from the submitted text — nothing is matched
//! out of the request or returned as a constant response.
//!
//! ## What the recogniser places, and what it does not
//!
//! Amazon Comprehend's targeted sentiment runs a learned entity model; this emulator
//! has no model weights and performs no network call, so it places a mention only
//! where a deterministic rule finds evidence for one in the text:
//!
//! | rule | evidence | type |
//! |---|---|---|
//! | proper-noun span | a run of adjacent capitalised, non-function words | this service's entity recogniser's typing of the name, else `ORGANIZATION` for a legal-entity suffix, `DATE` for a calendar name, else `OTHER` |
//! | honorific + name | `Dr`, `Ms`, … before a capitalised span | `PERSONAL_TITLE` for the honorific, `PERSON` for the span |
//! | pronoun | a member of English's closed pronoun class | its group's type, `PERSON` for a personal pronoun |
//! | determiner-headed noun phrase | a determiner followed by content words | `OTHER` |
//! | numeral | a token made of digits (and `.`/`,`) | `QUANTITY` |
//!
//! A mention is a WHOLE named mention: the recogniser answers which CHARACTERS it
//! typed, and its rules fire over the parts of a compound name independently, so a
//! recognition that covers only part of the name the document writes neither cuts the
//! mention short nor leaves the rest of that name to open a mention — or an entity
//! group — of its own. See [`place_mentions`].
//!
//! What is GROUPED rather than placed: a document mentions one entity in more than one
//! way, and every such mention belongs to that entity's group instead of opening a
//! group of its own — the same surface form again, and a pronoun referring back to it.
//! See [`group_mentions`]; a group's `GroupScore` is the confidence of the link that
//! placed each mention in it.
//!
//! What is not placed at all: a noun phrase that REFERS to nothing. The predicate a
//! copula states of its subject ascribes a category to it rather than naming a further
//! thing (`Amazon Comprehend is a natural language processing service` writes ONE
//! entity and says what kind of thing it is), so such a phrase is no target — while the
//! same words standing as a clause's subject or object refer, and are.
//!
//! The honest limits of that, which a reader of the response should know: an
//! adjective in a noun phrase can be taken for its head, a capitalised word that is
//! not a proper noun is taken for one, and the finer types a learned model assigns
//! (`BRAND`, `FACILITY`, `MOVIE`, …) are reported as the modelled catch-all `OTHER`
//! rather than guessed. `Score` is the recogniser's own confidence for the mention it
//! typed, computed from the evidence it weighed over those characters; where no
//! recognition reached the mention it is the placing rule's precision rank, which
//! this file says rather than pretending to be a learned confidence.
//!
//! Offsets are CHARACTER offsets into the document, as AWS documents an entity
//! offset ("the zero-based offset from the beginning of the source text to the first
//! character"), so a document carrying multi-byte characters does not report byte
//! positions: `Text` is always the document's own substring at
//! `[BeginOffset, EndOffset)`.
//!
//! The work is bounded by the input: `Text` is capped at 5 KB, so the token, mention
//! and group counts are all bounded by it, and the analysis persists nothing — the
//! answer is a function of `Text` and `LanguageCode` alone, so repeating a call
//! returns the same entities with the same offsets.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DetectTargetedSentiment::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DetectTargetedSentiment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-comprehend-comprehend-20171127/smithy-model/com-amazonaws-comprehend-comprehend-20171127.json")
//! ```
//!
//! Inspects the input text and returns a sentiment analysis for each entity identified in
//! the text. For more information about targeted sentiment, see Targeted sentiment in the
//! Amazon Comprehend Developer Guide.
//!
//! ## Input
//!
//! * `LanguageCode` (LanguageCode, one of: ar | de | en | es | fr | hi | it | ja | ko | pt |
//!   zh | zh-TW) **required** — The language of the input documents. Currently, English is
//!   the only supported language.
//! * `Text` (CustomerInputString, len 1..) **required** — A UTF-8 text string. The maximum
//!   string length is 5 KB.
//!
//! ## Output
//!
//! * `Entities` (ListOfTargetedSentimentEntities) — Targeted sentiment analysis for each of
//!   the entities identified in the input text.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid.
//! * `TextSizeLimitExceededException` (HTTP 400) — The size of the input text exceeds the
//!   limit. Use a smaller document.
//! * `UnsupportedLanguageException` (HTTP 400) — Amazon Comprehend can't process the language
//!   of the input text. For a list of supported languages, Supported languages in the
//!   Comprehend Developer Guide.

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use crate::ops::detect_entities::{Reading, EVIDENCE_PER_CAPITALIZED_CHAR};
use crate::ops::detect_sentiment::score_sentiment;

/// The largest `Text` this operation accepts, in UTF-8 bytes.
///
/// From the modelled member documentation of `DetectTargetedSentimentRequest$Text`:
/// "The maximum string length is 5 KB." Anything larger is refused with the
/// `TextSizeLimitExceededException` the operation declares.
const MAX_TEXT_BYTES: usize = 5 * 1024;

/// The `LanguageCode` values this operation analyses.
///
/// The modelled documentation of `DetectTargetedSentimentRequest$LanguageCode` says
/// "Currently, English is the only supported language", so AWS's own set for this
/// operation is narrower than the `LanguageCode` enum the member targets. A value
/// outside it is therefore refused the way an input-validation constraint is — HTTP
/// 400 `ValidationException`, naming the member, the offending value and this set —
/// and NOT with the modelled `UnsupportedLanguageException`, which answers a
/// language AWS supports but this emulator's engine cannot read.
const ANALYSABLE_LANGUAGES: &[&str] = &["en"];

/// `Score` — "model confidence that the entity is relevant" — as the tally of what
/// placed THIS mention.
///
/// A mention this service's entity recogniser placed is reported with the confidence
/// THAT recognition carries ([`mention_score`] takes it from
/// `crate::ops::detect_entities::recognised_spans`), so one mention has one confidence
/// whichever operation reports it. A mention no recognition placed is tallied here,
/// from the cue that placed it weighed over the mention's own characters: a cue that
/// accounts for a longer mention is less likely to have been a coincidence, so two
/// mentions placed by the same rule are reported differently when the document gives
/// them different support.
///
/// What one character of the mention weighs, by what read it:
/// a closed-class word the lexicon spells out (a pronoun) names the whole mention and
/// is the strongest reading here.
const EVIDENCE_PER_CLOSED_CLASS_CHAR: f64 = 1.5;

/// A typing cue standing inside the mention or in front of it — an honorific, a
/// legal-entity suffix, a calendar name, the closed form of a figure — says WHAT KIND
/// of thing is named there without spelling the mention out. See
/// [`EVIDENCE_PER_CLOSED_CLASS_CHAR`].
const EVIDENCE_PER_TYPING_CUE_CHAR: f64 = 1.0;

// Bare capitalisation: in running English text a capital marks a name, which is weak
// on its own and says less about a short token than a long one. The weight is the
// recogniser's own (`crate::ops::detect_entities::EVIDENCE_PER_CAPITALIZED_CHAR`,
// imported above), since it is the same orthographic evidence read about the same
// characters.

/// The head of a determiner-headed noun phrase: the position says something is named
/// there and the noun says what, which is the reading most easily fooled by an
/// adjective. See [`EVIDENCE_PER_CLOSED_CLASS_CHAR`].
const EVIDENCE_PER_PHRASE_HEAD_CHAR: f64 = 0.2;

/// The weight, per character, of the reading in which a capital is orthography rather
/// than a name: a sentence's first word and an abbreviation carry one too.
const READING_CAPITAL_EXPLAINED_PER_CHAR: f64 = 0.1;

/// The weight, per character, of the reading in which a common noun names no entity
/// at all — the generic reading of `the service`, which the phrase rule cannot tell
/// from a mention of one.
const READING_NO_ENTITY_NAMED_PER_CHAR: f64 = 0.3;

/// The weight of the reading in which the span is not a relevant entity at all, on
/// the scale the evidence above is tallied in. Always open, so no mention is reported
/// as certain, and its size is what puts the tally on the scale a confidence is read
/// on.
const READING_NOT_A_TARGET: f64 = 0.04;

/// Evidence one character of an identical surface form holds for the grouping: a
/// document that writes the same name again is writing about the same entity, and the
/// longer the spelling the two mentions share the less likely the agreement is a
/// coincidence.
const EVIDENCE_PER_SHARED_FORM_CHAR: f64 = 1.5;

/// Evidence one character that two names DISAGREE on weighs against the grouping:
/// `Lennox` joining `Alice Lennox` leaves the given name unaccounted for.
const READING_ANOTHER_ENTITY_PER_CHAR: f64 = 0.4;

/// Evidence a pronoun's antecedent holds for the grouping, for the sentence the
/// pronoun stands in: English resolves a pronoun to the nearest admissible referent,
/// so the link rests on the antecedent being the most recent one.
const EVIDENCE_PER_NEAREST_ANTECEDENT: f64 = 3.0;

/// How much of that evidence is left for each further sentence between the pronoun and
/// its antecedent's last mention: the further back the referent was last written, the
/// more room there is for another one in between, so the confidence of the link decays
/// with the distance rather than stopping at a fixed floor.
const ANTECEDENT_DISTANCE_DECAY: f64 = 0.5;

/// The weight of the reading in which the mentions grouped together name DIFFERENT
/// entities, on the scale the grouping evidence is tallied in. Always open, so no
/// grouping of two mentions is reported as certain.
const READING_DIFFERENT_ENTITIES: f64 = 0.04;

/// The `GroupScore` of a mention that opened its own group.
///
/// `GroupScore` is "the confidence that all the entities mentioned in the group relate
/// to the same entity". A group of one relates its mention to nothing else, so there
/// is no grouping claim that could be wrong: the confidence is a certainty, and it
/// stays one for every further mention whose surface form the group already holds only
/// because that agreement is itself tallied ([`EVIDENCE_PER_SHARED_FORM_CHAR`]).
const GROUP_OF_ONE: f32 = 1.0;

// The confidences below are tallied with the service's ONE reading tally
// (`crate::ops::detect_entities::Reading`): a confidence is the supported reading's
// share of everything the evidence leaves open, weighed against the one reading
// nothing closes off, so the value FOLLOWS the detection instead of naming the rule
// that made it. What these rules choose is the WEIGHTS below.

/// The confidence a mention a closed-class lexicon entry placed is reported with.
fn closed_class_confidence(mention: &str) -> f32 {
    Reading::over(
        mention,
        EVIDENCE_PER_CLOSED_CLASS_CHAR,
        0.0,
        READING_NOT_A_TARGET,
    )
    .confidence()
}

/// The confidence a mention a typing cue placed is reported with.
fn typing_cue_confidence(mention: &str) -> f32 {
    Reading::over(
        mention,
        EVIDENCE_PER_TYPING_CUE_CHAR,
        0.0,
        READING_NOT_A_TARGET,
    )
    .confidence()
}

/// The confidence a mention bare capitalisation placed is reported with.
fn capitalisation_confidence(mention: &str) -> f32 {
    Reading::over(
        mention,
        EVIDENCE_PER_CAPITALIZED_CHAR,
        READING_CAPITAL_EXPLAINED_PER_CHAR,
        READING_NOT_A_TARGET,
    )
    .confidence()
}

/// The confidence a determiner-headed common noun is reported with.
fn phrase_head_confidence(mention: &str) -> f32 {
    Reading::over(
        mention,
        EVIDENCE_PER_PHRASE_HEAD_CHAR,
        READING_NO_ENTITY_NAMED_PER_CHAR,
        READING_NOT_A_TARGET,
    )
    .confidence()
}

/// The most words a determiner-headed noun phrase may span before its head.
///
/// A longer run is almost always the rule running past the phrase, so it is cut.
const MAX_NOUN_PHRASE_WORDS: usize = 4;

/// English honorifics, the closed set that marks the capitalised span after it as a
/// person's name. Matched case-insensitively and without the abbreviating period,
/// which the word segmentation leaves out of the token.
const HONORIFICS: &[&str] = &[
    "mr", "mrs", "ms", "miss", "mx", "dr", "prof", "professor", "sir", "madam",
    "lord", "lady", "rev", "capt", "sgt", "gov", "sen",
];

/// Legal-entity suffixes: a proper-noun span ending in one names an organisation.
const ORGANISATION_SUFFIXES: &[&str] = &[
    "inc", "incorporated", "corp", "corporation", "company", "co", "llc", "llp",
    "ltd", "limited", "plc", "gmbh", "ag", "nv", "bv", "sa", "spa", "oy", "ab",
];

/// Calendar names: a proper-noun span containing one is a date.
const CALENDAR_NAMES: &[&str] = &[
    "january", "february", "march", "april", "may", "june", "july", "august",
    "september", "october", "november", "december", "jan", "feb", "mar", "apr",
    "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec", "monday", "tuesday",
    "wednesday", "thursday", "friday", "saturday", "sunday", "mon", "tue", "tues",
    "wed", "thu", "thur", "thurs", "fri", "sat", "sun",
];

/// English determiners: the words that open a noun phrase, so the content words
/// after one are that phrase and its head is a mention.
///
/// The possessive determiners (`my`, `her`, …) are in [`PRONOUNS`] as well, and are
/// read as both: a mention of the possessor, and the opening of the possessed
/// phrase.
const DETERMINERS: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "my", "your", "his", "her",
    "its", "our", "their", "some", "any", "each", "every", "no", "another", "both",
];

/// The possessive determiners of [`DETERMINERS`] that are also pronouns.
const POSSESSIVE_PRONOUNS: &[&str] = &["my", "your", "his", "her", "its", "our", "their"];

/// Words that start a clause with a polarity of its own, so the clause scored for a
/// mention's sentiment ends at them ("the food was great but the service was
/// terrible").
const CLAUSE_BREAKS: &[&str] = &[
    "but", "however", "although", "though", "yet", "whereas", "while", "despite",
];

/// What kind of rule placed a mention, which decides how it may be grouped and
/// whether it can name its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A capitalised span, or an honorific's name.
    ProperNoun,
    /// A determiner-headed noun phrase's head, or a numeral.
    CommonNoun,
    /// A member of English's closed pronoun class.
    Pronoun,
}

/// What a pronoun can refer to, which is what decides the group it joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PronounClass {
    /// First person singular: the writer, who is never named in the text.
    Speaker,
    /// First person plural: the writer's group.
    SpeakerGroup,
    /// Second person: the reader.
    Addressee,
    /// Third person singular with a human referent: resolves to a person group.
    PersonAnaphor,
    /// Third person singular with a non-human referent: resolves to a non-person
    /// group.
    ThingAnaphor,
    /// Third person plural: resolves to any earlier group.
    PluralAnaphor,
}

/// English's closed pronoun class, each form with what it can refer to.
const PRONOUNS: &[(&str, PronounClass)] = &[
    ("i", PronounClass::Speaker),
    ("me", PronounClass::Speaker),
    ("my", PronounClass::Speaker),
    ("mine", PronounClass::Speaker),
    ("myself", PronounClass::Speaker),
    ("we", PronounClass::SpeakerGroup),
    ("us", PronounClass::SpeakerGroup),
    ("our", PronounClass::SpeakerGroup),
    ("ours", PronounClass::SpeakerGroup),
    ("ourselves", PronounClass::SpeakerGroup),
    ("you", PronounClass::Addressee),
    ("your", PronounClass::Addressee),
    ("yours", PronounClass::Addressee),
    ("yourself", PronounClass::Addressee),
    ("yourselves", PronounClass::Addressee),
    ("he", PronounClass::PersonAnaphor),
    ("him", PronounClass::PersonAnaphor),
    ("his", PronounClass::PersonAnaphor),
    ("himself", PronounClass::PersonAnaphor),
    ("she", PronounClass::PersonAnaphor),
    ("her", PronounClass::PersonAnaphor),
    ("hers", PronounClass::PersonAnaphor),
    ("herself", PronounClass::PersonAnaphor),
    ("it", PronounClass::ThingAnaphor),
    ("its", PronounClass::ThingAnaphor),
    ("itself", PronounClass::ThingAnaphor),
    ("they", PronounClass::PluralAnaphor),
    ("them", PronounClass::PluralAnaphor),
    ("their", PronounClass::PluralAnaphor),
    ("theirs", PronounClass::PluralAnaphor),
    ("themselves", PronounClass::PluralAnaphor),
];

/// The English function words — the closed-class vocabulary.
///
/// A function word is never a mention on its own, and it is where a noun phrase
/// ends. The inventory is English's closed classes, enumerated by class so a reader
/// can check it: determiners and quantifiers, pronouns, prepositions,
/// conjunctions, the copula, the auxiliaries and modals, and the degree and
/// discourse adverbs that stand between a determiner and its head. It is
/// enumerated here rather than taken from a word-list crate so that implementing
/// this operation needs no new workspace dependency.
const FUNCTION_WORDS: &[&str] = &[
    // Determiners, articles and quantifiers
    "a", "an", "the", "this", "that", "these", "those", "all", "another", "any",
    "both", "each", "either", "enough", "every", "few", "fewer", "less", "little",
    "many", "more", "most", "much", "neither", "no", "none", "other", "others",
    "same", "several", "some", "such",
    // Pronouns (the full closed class is in `PRONOUNS` as well, with what each
    // refers to; both are needed, because a pronoun is a mention and a function
    // word at once)
    "i", "me", "my", "mine", "myself", "we", "us", "our", "ours", "ourselves",
    "you", "your", "yours", "yourself", "yourselves", "he", "him", "his",
    "himself", "she", "her", "hers", "herself", "it", "its", "itself", "they",
    "them", "their", "theirs", "themselves", "who", "whom", "whose", "which",
    "what", "whatever", "whoever", "anyone", "anything", "everyone", "everything",
    "nobody", "nothing", "someone", "something",
    // Prepositions
    "about", "above", "across", "after", "against", "along", "among", "around",
    "at", "before", "behind", "below", "beneath", "beside", "between", "beyond",
    "by", "despite", "down", "during", "except", "for", "from", "in", "inside",
    "into", "like", "near", "of", "off", "on", "onto", "out", "outside", "over",
    "past", "per", "since", "through", "throughout", "to", "toward", "towards",
    "under", "until", "up", "upon", "via", "with", "within", "without",
    // Conjunctions and subordinators
    "although", "and", "as", "because", "but", "else", "however", "if", "nor",
    "once", "or", "so", "than", "that", "then", "therefore", "though", "unless",
    "until", "when", "whenever", "where", "whereas", "wherever", "while",
    "whether", "yet",
    // Copula, auxiliaries and modals
    "am", "are", "be", "been", "being", "can", "could", "did", "do", "does",
    "doing", "done", "had", "has", "have", "having", "is", "may", "might", "must",
    "ought", "shall", "should", "was", "were", "will", "would",
    // Degree, negation and discourse adverbs
    "again", "almost", "already", "also", "always", "anyway", "even", "ever",
    "here", "indeed", "instead", "just", "maybe", "merely", "never", "not", "now",
    "often", "only", "perhaps", "quite", "rather", "really", "seldom", "somewhat",
    "soon", "still", "then", "there", "too", "very", "why", "yes",
];

/// [`FUNCTION_WORDS`] as a set, built once: the list is immutable and looking a
/// word up in it happens once per token per rule.
fn function_words() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| FUNCTION_WORDS.iter().copied().collect())
}

/// What a pronoun form refers to, or `None` when the word is not a pronoun.
fn pronoun_class(lower: &str) -> Option<PronounClass> {
    PRONOUNS
        .iter()
        .find(|(form, _)| *form == lower)
        .map(|(_, class)| *class)
}

/// One word token of the document, with the byte range it occupies and the sentence
/// it belongs to.
#[derive(Debug, Clone, Copy)]
struct Word<'a> {
    text: &'a str,
    start: usize,
    end: usize,
    sentence: usize,
}

/// The characters that end a sentence, and the closing marks that may follow one
/// before the boundary ("… wonderful!'" or "… wonderful.)").
const SENTENCE_TERMINATORS: &[char] = &['.', '!', '?', '\u{2026}'];
/// Marks that may trail a sentence terminator and still belong to that sentence.
const SENTENCE_CLOSERS: &[char] = &[
    '"', '\'', ')', ']', '\u{2019}', '\u{201d}', '\u{00bb}',
];

/// Segment the document into its word tokens and its sentence byte ranges.
///
/// A WORD is a maximal run of alphanumeric characters, keeping an apostrophe inside
/// the token when a letter follows it (`don't` is one word) and a `.` or `,` inside
/// it when digits stand on both sides (`1,250` and `3.5` are one word each), so the
/// punctuation that ends a sentence is never part of a word. A mention is made of
/// words, so punctuation-only and whitespace-only runs are dropped.
///
/// A SENTENCE ends after a run of terminal punctuation — and the closing quotes or
/// brackets that trail it — when the end of the document or whitespace follows; the
/// whitespace belongs to the sentence it closes, so the ranges tile the document and
/// every mention sits in exactly one of them. The last sentence runs to the end of
/// the document.
///
/// Both are scanned here rather than taken from a segmentation crate, so this
/// operation adds no workspace dependency; the rules are stated above and the
/// documents they are wrong about (an abbreviation that ends in a period, say) are
/// wrong only in where a clause boundary falls, never in an offset — every mention's
/// `Text` is sliced from the document at the offsets reported for it.
fn segment(text: &str) -> (Vec<Word<'_>>, Vec<(usize, usize)>) {
    let characters: Vec<(usize, char)> = text.char_indices().collect();

    let mut sentences: Vec<(usize, usize)> = Vec::new();
    let mut sentence_start = 0usize;
    let mut index = 0usize;
    while index < characters.len() {
        if !SENTENCE_TERMINATORS.contains(&characters[index].1) {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < characters.len()
            && (SENTENCE_TERMINATORS.contains(&characters[end].1)
                || SENTENCE_CLOSERS.contains(&characters[end].1))
        {
            end += 1;
        }
        let closes = end >= characters.len()
            || characters
                .get(end)
                .is_some_and(|(_, next)| next.is_whitespace());
        if !closes {
            index = end;
            continue;
        }
        // The whitespace after the terminator belongs to the sentence it closes.
        let mut after = end;
        while after < characters.len() && characters[after].1.is_whitespace() {
            after += 1;
        }
        let sentence_end = characters.get(after).map_or(text.len(), |(byte, _)| *byte);
        sentences.push((sentence_start, sentence_end));
        sentence_start = sentence_end;
        index = after;
    }
    if sentence_start < text.len() || sentences.is_empty() {
        sentences.push((sentence_start, text.len()));
    }

    let mut words: Vec<Word<'_>> = Vec::new();
    let mut index = 0usize;
    let mut sentence = 0usize;
    while index < characters.len() {
        let (start, first) = characters[index];
        if !first.is_alphanumeric() {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < characters.len() {
            let candidate = characters[end].1;
            let following = characters.get(end + 1).map(|(_, next)| *next);
            let joins = candidate.is_alphanumeric()
                || (matches!(candidate, '\'' | '\u{2019}')
                    && following.is_some_and(char::is_alphanumeric))
                || (matches!(candidate, '.' | ',')
                    && characters[end - 1].1.is_ascii_digit()
                    && following.is_some_and(|next| next.is_ascii_digit()));
            if !joins {
                break;
            }
            end += 1;
        }
        let stop = characters.get(end).map_or(text.len(), |(byte, _)| *byte);
        // The sentence ranges tile the document in order, and so do the words, so
        // the index only ever moves forward.
        while sentence + 1 < sentences.len() && start >= sentences[sentence].1 {
            sentence += 1;
        }
        words.push(Word {
            text: &text[start..stop],
            start,
            end: stop,
            sentence,
        });
        index = end;
    }

    (words, sentences)
}

/// Whether `token` starts with an upper-case letter.
fn is_capitalised(token: &str) -> bool {
    token.chars().next().is_some_and(char::is_uppercase)
}

/// Whether `token` is a numeral: digits, with the group and decimal separators a
/// written number may carry.
fn is_numeric(token: &str) -> bool {
    token.chars().any(|c| c.is_ascii_digit())
        && token
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

/// Whether the document between `from` and `to` is whitespace only, i.e. the two
/// tokens either side are adjacent words of one span.
fn adjacent(text: &str, from: usize, to: usize) -> bool {
    from <= to && text[from..to].chars().all(char::is_whitespace)
}

/// Whether `word` can be part of a mention span at all: a function word never is.
fn is_content_word(word: &Word<'_>) -> bool {
    !function_words().contains(word.text.to_lowercase().as_str())
}

/// Whether a capitalised word may stand in a name span.
///
/// A content word may, and so may one of the closed-class typing cues the
/// function-word list also holds — `Inc` and `May` are function words of English as well as a
/// legal-entity suffix and a month, and a name carries them.
fn is_span_word(word: &Word<'_>) -> bool {
    let lower = word.text.to_lowercase();
    is_capitalised(word.text)
        && (is_content_word(word)
            || ORGANISATION_SUFFIXES.contains(&lower.as_str())
            || CALENDAR_NAMES.contains(&lower.as_str()))
}

/// The end (exclusive) of the capitalised span starting at `from`, or `None` when no
/// span starts there.
///
/// A span is a run of capitalised content words separated by whitespace only and
/// inside one sentence, which is what a multi-word proper name looks like.
fn capitalised_span(text: &str, words: &[Word<'_>], from: usize) -> Option<usize> {
    let first = words.get(from)?;
    if !is_span_word(first) {
        return None;
    }
    let mut end = from + 1;
    while let Some(candidate) = words.get(end) {
        if candidate.sentence != first.sentence
            || !is_span_word(candidate)
            || !adjacent(text, words[end - 1].end, candidate.start)
        {
            break;
        }
        end += 1;
    }
    Some(end)
}

/// The type a proper-noun span carries, from the typing cues its words show.
///
/// The LAST resort: what types a name here is this service's own entity recogniser
/// ([`recognised_entities`]), which reads the whole document rather than the cues
/// inside one span. These cues answer a span that recogniser placed nothing on.
fn proper_noun_type(span: &[Word<'_>]) -> &'static str {
    let last = span
        .last()
        .map(|word| word.text.to_lowercase())
        .unwrap_or_default();
    if ORGANISATION_SUFFIXES.contains(&last.as_str()) {
        return "ORGANIZATION";
    }
    if span
        .iter()
        .any(|word| CALENDAR_NAMES.contains(&word.text.to_lowercase().as_str()))
    {
        return "DATE";
    }
    "OTHER"
}

/// One entity this service's recogniser placed in the document, as
/// [`crate::ops::detect_entities::Recognition`] — here the type it carries is the
/// modelled `TargetedSentimentEntityType` this operation reports it under, which
/// [`targeted_type_of`] maps the recogniser's `EntityType` onto.
type Recognition = crate::ops::detect_entities::Recognition;

/// Every entity this service's entity recogniser places in `text`, each as a
/// [`Recognition`].
///
/// What this operation targets is the things the document NAMES — the modelled
/// `TargetedSentimentEntityType` values are the kinds of named thing, and the
/// operation's own documentation is about "the sentiment … towards each entity" — so
/// the targets are the entities `DetectEntities` places, read through
/// [`crate::ops::detect_entities::recognised_spans`]. One recogniser answers what a
/// document names and how well each name is evidenced, so the two operations cannot
/// disagree about it, and a named thing with no capital of its own (an e-mail
/// address, a figure) is targeted too.
///
/// What that recogniser answers is which CHARACTERS it typed, not where one name
/// ends: it types the parts of a compound name separately where its rules fire
/// separately. A mention is a whole name, so [`place_mentions`] reads these as
/// evidence about the name it is placing rather than as the name's extent.
fn recognised_entities(text: &str) -> Vec<Recognition> {
    crate::ops::detect_entities::recognised_spans(text)
        .into_iter()
        .map(|(span, entity_type, confidence)| (span, targeted_type_of(entity_type), confidence))
        .collect()
}

/// The modelled `TargetedSentimentEntityType` an `EntityType` is targeted under.
///
/// Every `EntityType` value is a `TargetedSentimentEntityType` value too, under the
/// same name, with one exception: `TITLE` names a creative work, which the targeted
/// enum splits by medium (`MOVIE`, `MUSIC`, `BOOK`, `GAME`) — nothing says WHICH, so
/// the work is targeted as `OTHER` rather than assigned a medium no evidence names.
fn targeted_type_of(entity_type: &str) -> &'static str {
    match entity_type {
        "PERSON" => "PERSON",
        "LOCATION" => "LOCATION",
        "ORGANIZATION" => "ORGANIZATION",
        "COMMERCIAL_ITEM" => "COMMERCIAL_ITEM",
        "EVENT" => "EVENT",
        "DATE" => "DATE",
        "QUANTITY" => "QUANTITY",
        _ => "OTHER",
    }
}

/// The entity the recogniser placed over the characters `[start, end)`, when it placed
/// one: the typed span sharing the most characters with them.
fn placed_entity(entities: &[Recognition], start: usize, end: usize) -> Option<Recognition> {
    entities
        .iter()
        .filter_map(|(span, entity_type, confidence)| {
            let overlap = end.min(span.end).saturating_sub(start.max(span.start));
            (overlap > 0).then_some((overlap, span.clone(), *entity_type, *confidence))
        })
        .max_by_key(|(overlap, _, _, _)| *overlap)
        .map(|(_, span, entity_type, confidence)| (span, entity_type, confidence))
}

/// The head of the noun phrase starting at `from`, and the index just past the
/// phrase.
///
/// The phrase runs over the words this service's part-of-speech tagger reads as
/// standing inside a noun phrase (`crate::ops::detect_syntax::NOUN_PHRASE_MODIFIER_TAGS`
/// — a modifier, a numeral or a noun) and ends at the first word that cannot be in
/// one, at a capitalised word (which the proper-noun rule claims instead), at a
/// sentence boundary, or after [`MAX_NOUN_PHRASE_WORDS`] words. Its HEAD is the LAST
/// noun of that run, as an English noun phrase is head-final: `a natural language
/// processing service` is a mention of the SERVICE, and the nouns modifying it inside
/// the compound are not mentions of their own.
fn noun_phrase_head<'a>(
    words: &[Word<'a>],
    tags: &HashMap<usize, &'static str>,
    from: usize,
) -> Option<(usize, Word<'a>)> {
    let sentence = words.get(from)?.sentence;
    let mut end = from;
    let mut head: Option<(usize, Word<'a>)> = None;
    while end - from < MAX_NOUN_PHRASE_WORDS {
        let Some(candidate) = words.get(end) else { break };
        let tag = tags.get(&candidate.start).copied().unwrap_or_default();
        if candidate.sentence != sentence
            || !is_content_word(candidate)
            || is_capitalised(candidate.text)
            || is_numeric(candidate.text)
            || !crate::ops::detect_syntax::NOUN_PHRASE_MODIFIER_TAGS.contains(&tag)
        {
            break;
        }
        end += 1;
        if crate::ops::detect_syntax::NOUN_PHRASE_HEAD_TAGS.contains(&tag) {
            head = Some((end, *candidate));
        }
    }
    head
}

/// The confidence a mention is reported with: the recogniser's own confidence for the
/// recognition that reached the mention's FIRST word — the part of a name that says
/// what is named — and `tallied`, what the cue that placed the mention accounts for of
/// its own characters ([`EVIDENCE_PER_CLOSED_CLASS_CHAR`] and the weights beside it),
/// where it reached none.
fn mention_score(entities: &[Recognition], word: &Word<'_>, tallied: f32) -> f32 {
    placed_entity(entities, word.start, word.end).map_or(tallied, |(_, _, confidence)| confidence)
}

/// One mention the recogniser placed, before it is grouped.
#[derive(Debug, Clone)]
struct Mention {
    /// Byte range in the document; rendered as character offsets on the wire.
    start: usize,
    end: usize,
    text: String,
    kind: Kind,
    entity_type: &'static str,
    /// Whether the type came from this service's entity RECOGNISER rather than from a
    /// cue inside the mention. A recognised type is evidence about what the mention
    /// names, so a mention carrying one does not join a group of another type.
    typed: bool,
    score: f32,
    sentence: usize,
    /// What the mention can refer to, when it is a pronoun.
    pronoun: Option<PronounClass>,
}

impl Mention {
    /// A mention over the document's own `[start, end)` substring, so its `Text` and
    /// its offsets can never disagree.
    #[allow(clippy::too_many_arguments)]
    fn new(
        text: &str,
        start: usize,
        end: usize,
        kind: Kind,
        entity_type: &'static str,
        score: f32,
        sentence: usize,
        pronoun: Option<PronounClass>,
    ) -> Self {
        Self {
            start,
            end,
            text: text[start..end].to_string(),
            kind,
            entity_type,
            typed: false,
            score,
            sentence,
            pronoun,
        }
    }
}

/// Place every mention the rules find, in document order and without overlap.
///
/// `entities` is what this service's entity recogniser placed in the document
/// ([`recognised_entities`]): it TYPES and SCORES every name span placed here, and
/// every entity it placed that no rule here reached is a mention of its own, so a
/// named thing the document writes without a capital of its own is targeted as well.
///
/// A mention is a WHOLE named mention. The recogniser types characters, and the rules
/// that type them fire over the parts of a compound name independently — a brand and
/// the product beside it, a name and the title in front of it — so a recognition may
/// cover only part of the name the document writes. Where that happens the name's own
/// extent wins: the mention runs to the end of the adjacent run of name words (and
/// further, when a recognition reaches past it), and no word inside it opens a mention
/// or an entity group of its own.
fn place_mentions(text: &str, words: &[Word<'_>], entities: &[Recognition]) -> Vec<Mention> {
    let tags = crate::ops::detect_syntax::tags_by_offset(text);
    let mut found: Vec<Mention> = Vec::new();
    let mut index = 0usize;
    while let Some(word) = words.get(index).copied() {
        let lower = word.text.to_lowercase();

        // An honorific names the capitalised span after it as a person. Matched only
        // on a capitalised token, so a common-noun use ("I spoke to the sir") is not
        // read as a title.
        if is_capitalised(word.text) && HONORIFICS.contains(&lower.as_str()) {
            found.push(Mention::new(
                text,
                word.start,
                word.end,
                Kind::ProperNoun,
                "PERSONAL_TITLE",
                mention_score(entities, &word, typing_cue_confidence(word.text)),
                word.sentence,
                None,
            ));
            index += 1;
            if let Some(span_end) = capitalised_span(text, words, index) {
                let end = words
                    .get(span_end - 1)
                    .map_or(word.end, |last| last.end);
                let start = words.get(index).map_or(word.end, |first| first.start);
                // The honorific IS the evidence that the span after it names a
                // person, so the typing is the grammar's; how confident the mention
                // is, is still the recogniser's where it placed a recognition on
                // those characters, and otherwise the tally of what the honorific
                // accounts for.
                let score = words.get(index).map_or_else(
                    || typing_cue_confidence(&text[start..end]),
                    |first| {
                        mention_score(entities, first, typing_cue_confidence(&text[start..end]))
                    },
                );
                found.push(Mention::new(
                    text,
                    start,
                    end,
                    Kind::ProperNoun,
                    "PERSON",
                    score,
                    word.sentence,
                    None,
                ));
                index = span_end;
            }
            continue;
        }

        // A pronoun is a mention of whatever it refers to; a possessive one also
        // opens the phrase it possesses.
        if let Some(class) = pronoun_class(&lower) {
            let entity_type = match class {
                PronounClass::ThingAnaphor | PronounClass::PluralAnaphor => "OTHER",
                _ => "PERSON",
            };
            found.push(Mention::new(
                text,
                word.start,
                word.end,
                Kind::Pronoun,
                entity_type,
                mention_score(entities, &word, closed_class_confidence(word.text)),
                word.sentence,
                Some(class),
            ));
            index += 1;
            if POSSESSIVE_PRONOUNS.contains(&lower.as_str()) {
                if let Some((next, head)) = noun_phrase_head(words, &tags, index) {
                    found.push(Mention::new(
                        text,
                        head.start,
                        head.end,
                        Kind::CommonNoun,
                        "OTHER",
                        mention_score(entities, &head, phrase_head_confidence(head.text)),
                        head.sentence,
                        None,
                    ));
                    index = next;
                }
            }
            continue;
        }

        // A capitalised word opens a proper-noun span.
        if is_span_word(&word) {
            let span_end = capitalised_span(text, words, index).unwrap_or(index + 1);
            let span = &words[index..span_end];
            let run_start = word.start;
            let run_end = span.last().map_or(word.end, |last| last.end);
            // What the entity recogniser placed over this run, when it placed
            // anything. It typed CHARACTERS, and its rules fire over the parts of a
            // compound name independently, so a recognition is read as evidence about
            // the name rather than as the name's extent:
            //
            // * the mention OPENS at the first word of the run a recognition reached.
            //   A leading word none of them reached is not part of the name
            //   ("Contact John Smith" is a mention of John Smith), and the recogniser
            //   read the whole document, so that is its answer rather than the
            //   capitalised run's.
            // * the mention ENDS at the end of the run, or further out when a
            //   recognition reaches past it (a date whose year sits outside the run of
            //   name words). A recognition that stops inside the run does NOT cut the
            //   name short, and the words after it do not open a mention of their own.
            // * it is TYPED and SCORED by the recognition that reached the name's
            //   first word: a compound name is written bearer first, so the part that
            //   opens it is the part that says what is named, and the parts after it
            //   qualify that.
            //
            // A run no recognition reached keeps its own extent and the cue typing.
            let opening = span
                .iter()
                .find_map(|word| placed_entity(entities, word.start, word.end));
            // The first word a recognition reached is where the name opens, so a
            // recognition the recogniser read as starting before this run — already
            // reported by the mention that run opened — cannot pull the mention back
            // over it.
            let begin = opening
                .as_ref()
                .map_or(run_start, |(span, _, _)| span.start.max(run_start));
            let mut end = run_end;
            if let Some(reach) = entities
                .iter()
                .filter(|(span, _, _)| span.start < run_end && begin < span.end)
                .map(|(span, _, _)| span.end)
                .max()
            {
                end = end.max(reach);
            }
            let (entity_type, score) = match opening.as_ref() {
                Some((_, entity_type, confidence)) => (*entity_type, *confidence),
                None => {
                    let entity_type = proper_noun_type(span);
                    // A run no recognition reached rests on what this rule read: a
                    // typing cue inside the span, or the capital alone.
                    let score = if entity_type == "OTHER" {
                        capitalisation_confidence(&text[begin..end])
                    } else {
                        typing_cue_confidence(&text[begin..end])
                    };
                    (entity_type, score)
                }
            };
            let mut next = span_end;
            // A numeral right after a calendar name belongs to the date it dates.
            if entity_type == "DATE" {
                if let Some(numeral) = words.get(next).copied() {
                    if numeral.sentence == word.sentence
                        && is_numeric(numeral.text)
                        && adjacent(text, end, numeral.start)
                    {
                        end = numeral.end;
                        next += 1;
                    }
                }
            }
            // No word inside the reported span opens a mention of its own.
            while words.get(next).is_some_and(|word| word.start < end) {
                next += 1;
            }
            let mut mention = Mention::new(
                text,
                begin,
                end,
                Kind::ProperNoun,
                entity_type,
                score,
                word.sentence,
                None,
            );
            mention.typed = opening.is_some();
            found.push(mention);
            index = next;
            continue;
        }

        // A determiner opens a noun phrase whose head is a mention — unless that
        // phrase refers to nothing. The predicate a copula states of its subject
        // ascribes a category to it (`Amazon Comprehend is a natural language
        // processing service`): the words name what KIND of thing the subject is
        // rather than a further thing the document writes about, so no sentiment is
        // directed at them and they are no target. The phrase is still consumed, so no
        // word inside it opens a mention of its own.
        if DETERMINERS.contains(&lower.as_str()) {
            if let Some((next, head)) = noun_phrase_head(words, &tags, index + 1) {
                let predicative = index
                    .checked_sub(1)
                    .and_then(|before| words.get(before))
                    .is_some_and(|before| {
                        before.sentence == word.sentence
                            && crate::ops::detect_syntax::COPULA_FORMS
                                .contains(&before.text.to_lowercase().as_str())
                    });
                if !predicative {
                    found.push(Mention::new(
                        text,
                        head.start,
                        head.end,
                        Kind::CommonNoun,
                        "OTHER",
                        mention_score(entities, &head, phrase_head_confidence(head.text)),
                        head.sentence,
                        None,
                    ));
                }
                index = next;
                continue;
            }
        }

        // A numeral is a quantity.
        if is_numeric(word.text) {
            found.push(Mention::new(
                text,
                word.start,
                word.end,
                Kind::CommonNoun,
                "QUANTITY",
                mention_score(entities, &word, typing_cue_confidence(word.text)),
                word.sentence,
                None,
            ));
            index += 1;
            continue;
        }

        index += 1;
    }

    // Every entity the recogniser placed that no rule above reached: a named thing
    // the document writes with no capital of its own (an e-mail address, an
    // identifier) is a target too, and the operation reports the sentiment directed
    // at it like any other.
    for (span, entity_type, confidence) in entities {
        if found
            .iter()
            .any(|mention| mention.start < span.end && span.start < mention.end)
        {
            continue;
        }
        let sentence = words
            .iter()
            .find(|word| word.end > span.start)
            .map_or(0, |word| word.sentence);
        let mut mention = Mention::new(
            text,
            span.start,
            span.end,
            Kind::ProperNoun,
            entity_type,
            *confidence,
            sentence,
            None,
        );
        mention.typed = true;
        found.push(mention);
    }
    // The rules above run left to right, so they place their mentions in document
    // order; the entities added after them are put back into it.
    found.sort_by_key(|mention| mention.start);
    found
}

/// A set of mentions this analysis reads as naming ONE entity.
///
/// The type belongs to the GROUP, not to each mention: every mention of one entity
/// is a mention of the same kind of thing, so a pronoun reports the type of what it
/// refers to.
#[derive(Debug)]
struct Group {
    entity_type: &'static str,
    /// Whether any mention in the group is a NAME rather than a pronoun or a common
    /// noun, which is what a personal pronoun may refer back to.
    named: bool,
    /// The normalised form of the longest name in the group, which is what a later
    /// mention is matched against.
    canonical: String,
    canonical_words: Vec<String>,
    /// Set for a group a pronoun opened whose referent is outside the document (the
    /// writer, the writer's group, the reader), which no other mention may join.
    speaker: Option<PronounClass>,
    /// Indices into the mention list, in document order.
    mentions: Vec<usize>,
    /// The last mention placed in this group, so "the most recent group" is
    /// well-defined.
    last_order: usize,
    last_sentence: usize,
}

/// The surface form a mention is matched on: lower-cased and without the possessive
/// clitic, so `Amazon's` and `Amazon` are the same name.
fn normalise(text: &str) -> String {
    let lower = text.to_lowercase();
    let stripped = lower
        .trim_end_matches("'s")
        .trim_end_matches('\u{2019}')
        .trim_end_matches("\u{2019}s");
    stripped.trim().to_string()
}

/// The words of a normalised name: its runs of alphanumeric characters.
fn name_words(canonical: &str) -> Vec<String> {
    canonical
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The index of the most recently mentioned group `accept` admits.
fn most_recent(groups: &[Group], accept: impl Fn(&Group) -> bool) -> Option<usize> {
    groups
        .iter()
        .enumerate()
        .filter(|(_, group)| accept(group))
        .max_by_key(|(_, group)| group.last_order)
        .map(|(index, _)| index)
}

/// Group the mentions that name one entity, and give every mention the `GroupScore`
/// of the link that placed it: the confidence that link's OWN evidence leaves — the
/// characters two forms share, the words they differ on, the sentences between a
/// pronoun and its antecedent — or [`GROUP_OF_ONE`] for a mention that opened its own
/// group.
fn group_mentions(mentions: &[Mention]) -> (Vec<Group>, Vec<f32>) {
    let mut groups: Vec<Group> = Vec::new();
    let mut group_scores = vec![GROUP_OF_ONE; mentions.len()];

    for (order, mention) in mentions.iter().enumerate() {
        let canonical = normalise(&mention.text);
        let resolved = match mention.pronoun {
            Some(class) => resolve_pronoun(&groups, class, mention),
            None => resolve_name(&groups, mention, &canonical),
        };

        match resolved {
            Some((index, link)) => {
                group_scores[order] = link;
                let group = &mut groups[index];
                group.mentions.push(order);
                group.last_order = order;
                group.last_sentence = mention.sentence;
                if mention.kind == Kind::Pronoun {
                    // A personal pronoun referring back to a name is the evidence
                    // that the name is a person's: no gazetteer places that, the
                    // grammar does.
                    if mention.pronoun == Some(PronounClass::PersonAnaphor) {
                        group.entity_type = "PERSON";
                    }
                } else {
                    group.named = group.named || mention.kind == Kind::ProperNoun;
                    // The longest name in the group is the one later mentions are
                    // matched against, and the one that names the group.
                    if canonical.chars().count() > group.canonical.chars().count() {
                        group.canonical_words = name_words(&canonical);
                        group.canonical = canonical;
                    }
                }
            }
            None => groups.push(Group {
                entity_type: mention.entity_type,
                named: mention.kind == Kind::ProperNoun,
                canonical_words: name_words(&canonical),
                canonical,
                speaker: mention.pronoun.filter(|class| {
                    matches!(
                        class,
                        PronounClass::Speaker
                            | PronounClass::SpeakerGroup
                            | PronounClass::Addressee
                    )
                }),
                mentions: vec![order],
                last_order: order,
                last_sentence: mention.sentence,
            }),
        }
    }

    (groups, group_scores)
}

/// The group a pronoun refers into, with the confidence of that LINK.
///
/// A pronoun whose referent is outside the document (the writer, the reader) joins
/// the group its own class opened, so every `I` in a document is one entity. An
/// anaphor joins the most recent group its class admits — a personal pronoun a
/// person or a name (which it then types as a person's), `it` something that is not
/// a person, a plural pronoun anything — and the further back that group was last
/// mentioned, the lower the confidence.
fn resolve_pronoun(
    groups: &[Group],
    class: PronounClass,
    mention: &Mention,
) -> Option<(usize, f32)> {
    match class {
        PronounClass::Speaker | PronounClass::SpeakerGroup | PronounClass::Addressee => groups
            .iter()
            .position(|group| group.speaker == Some(class))
            // Every writing of `I` in a document is the same writer: the pronoun's
            // own class is what the group is keyed on, so the link is the same
            // agreement of surface forms a repeated name carries.
            .map(|index| (index, closed_class_confidence(&mention.text))),
        PronounClass::PersonAnaphor | PronounClass::ThingAnaphor | PronounClass::PluralAnaphor => {
            let index = most_recent(groups, |group| {
                group.speaker.is_none()
                    && match class {
                        PronounClass::PersonAnaphor => {
                            group.entity_type == "PERSON"
                                || (group.named && group.entity_type == "OTHER")
                        }
                        PronounClass::ThingAnaphor => {
                            group.entity_type != "PERSON" && group.entity_type != "PERSONAL_TITLE"
                        }
                        _ => true,
                    }
            })?;
            // What the link rests on: the antecedent being the nearest admissible
            // referent. Each further sentence between the two leaves room for another
            // referent in between, so the evidence decays with the distance instead of
            // stopping at a fixed floor.
            let distance = mention.sentence.saturating_sub(groups[index].last_sentence) as f64;
            let score = Reading {
                support: EVIDENCE_PER_NEAREST_ANTECEDENT
                    * ANTECEDENT_DISTANCE_DECAY.powf(distance),
                competing: 0.0,
                never_excluded: READING_DIFFERENT_ENTITIES,
            }
            .confidence();
            Some((index, score))
        }
    }
}

/// The group a named mention joins, with the confidence of that LINK: the same
/// surface form is the same entity, and a name sharing every word with a name
/// already in the group is likely to be it.
fn resolve_name(groups: &[Group], mention: &Mention, canonical: &str) -> Option<(usize, f32)> {
    // An identical surface form is the same entity whatever type the group has
    // settled on: a pronoun referring into the group may have retyped it ("Maria …
    // She … Maria"), and keying on the mutable type would open a second entity for
    // the one name.
    if let Some(index) = most_recent(groups, |group| {
        group.speaker.is_none() && group.canonical == canonical
    }) {
        // The two mentions agree character for character: the longer the spelling
        // they share, the less likely the agreement is a coincidence.
        return Some((
            index,
            Reading::over(
                canonical,
                EVIDENCE_PER_SHARED_FORM_CHAR,
                0.0,
                READING_DIFFERENT_ENTITIES,
            )
            .confidence(),
        ));
    }
    if mention.kind != Kind::ProperNoun {
        return None;
    }
    let words = name_words(canonical);
    if words.is_empty() {
        return None;
    }
    let index = most_recent(groups, |group| {
        group.speaker.is_none()
            && group.named
            && admits_type(group, mention)
            && !group.canonical_words.is_empty()
            && (words.iter().all(|word| group.canonical_words.contains(word))
                || group
                    .canonical_words
                    .iter()
                    .all(|word| words.contains(word)))
    })?;
    // A partial name: the words the two names share support the link and the words
    // only one of them writes are what it leaves open, so `Lennox` joining
    // `Alice Lennox` is read with the confidence the shared surname leaves against
    // the given name nothing accounted for.
    let shared: usize = words
        .iter()
        .filter(|word| groups[index].canonical_words.contains(word))
        .map(|word| word.chars().count())
        .sum();
    let differing: usize = groups[index]
        .canonical_words
        .iter()
        .filter(|word| !words.contains(word))
        .map(|word| word.chars().count())
        .sum();
    let score = Reading {
        support: shared as f64 * EVIDENCE_PER_SHARED_FORM_CHAR,
        competing: differing as f64 * READING_ANOTHER_ENTITY_PER_CHAR,
        never_excluded: READING_DIFFERENT_ENTITIES,
    }
    .confidence();
    Some((index, score))
}

/// Whether a group still admits a name the recogniser typed: the two agree, or the
/// group was retyped `PERSON` by a pronoun referring into it and the name carries no
/// type of its own — a type the entity recogniser PLACED is evidence about what the
/// mention names, so a mention carrying one stays out of a group of another type.
fn admits_type(group: &Group, mention: &Mention) -> bool {
    group.entity_type == mention.entity_type
        || (group.entity_type == "PERSON" && mention.entity_type == "OTHER" && !mention.typed)
}

/// Byte offset -> character offset for every character boundary of the document,
/// including its end.
///
/// AWS documents an entity offset as the offset "to the first character" of the
/// mention, so a document carrying multi-byte characters must not report byte
/// positions.
fn character_offsets(text: &str) -> BTreeMap<usize, i32> {
    let mut offsets: BTreeMap<usize, i32> = text
        .char_indices()
        .enumerate()
        .map(|(characters, (byte, _))| (byte, characters as i32))
        .collect();
    offsets.insert(text.len(), text.chars().count() as i32);
    offsets
}

/// Where the clause carrying the mention at byte `at` begins and ends.
///
/// Targeted sentiment is the sentiment expressed TOWARDS a mention, so what is
/// scored for it is the clause it sits in rather than the whole document: its
/// sentence, cut again at `;` and at the words that start a clause with its own
/// polarity.
fn clause_of(text: &str, words: &[Word<'_>], sentences: &[(usize, usize)], at: usize) -> (usize, usize) {
    let (start, end) = sentences
        .iter()
        .copied()
        .find(|(start, end)| at >= *start && at < *end)
        .unwrap_or((0, text.len()));

    let mut from = start;
    let mut to = end;
    for word in words
        .iter()
        .filter(|word| word.start >= start && word.end <= end)
    {
        if CLAUSE_BREAKS.contains(&word.text.to_lowercase().as_str()) {
            if word.start <= at {
                from = from.max(word.start);
            } else {
                to = to.min(word.start);
            }
        }
    }
    for (offset, _) in text[start..end].match_indices(';') {
        let position = start + offset;
        if position < at {
            from = from.max(position + 1);
        } else {
            to = to.min(position);
        }
    }
    (from, to.max(from))
}

/// The indices into one entity's `Mentions` that "provide the best name for the
/// entity group": the longest mention that is a name, and every mention tied with
/// it. A group of nothing but pronouns has no name, so its first mention is the best
/// name there is.
fn descriptive_mention_index(order: &[usize], mentions: &[Mention]) -> Vec<i32> {
    let named: Vec<usize> = (0..order.len())
        .filter(|position| mentions[order[*position]].kind != Kind::Pronoun)
        .collect();
    let pool = if named.is_empty() {
        (0..order.len()).collect()
    } else {
        named
    };
    let longest = pool
        .iter()
        .map(|position| mentions[order[*position]].text.chars().count())
        .max()
        .unwrap_or_default();
    let picked: Vec<i32> = pool
        .iter()
        .filter(|position| mentions[order[**position]].text.chars().count() == longest)
        .map(|position| *position as i32)
        .collect();
    if picked.is_empty() {
        vec![0]
    } else {
        picked
    }
}

/// Analyse `text` into the entities targeted sentiment reports for it: every mention
/// with its offsets, its type, the confidence of the rules that placed and grouped
/// it, and the sentiment of the clause it sits in, read in `language_code`.
fn analyse(text: &str, language_code: &str) -> Vec<TargetedSentimentEntity> {
    let (words, sentences) = segment(text);
    let entities = recognised_entities(text);
    let mentions = place_mentions(text, &words, &entities);
    let (groups, group_scores) = group_mentions(&mentions);
    let offsets = character_offsets(text);

    let mut entities: Vec<(usize, TargetedSentimentEntity)> = Vec::with_capacity(groups.len());
    for group in &groups {
        let mut order = group.mentions.clone();
        order.sort_by_key(|index| mentions[*index].start);
        let rendered: Vec<TargetedSentimentMention> = order
            .iter()
            .map(|index| {
                let mention = &mentions[*index];
                let (from, to) = clause_of(text, &words, &sentences, mention.start);
                let (sentiment, sentiment_score) =
                    score_sentiment(&text[from..to], language_code);
                TargetedSentimentMention {
                    score: Some(mention.score),
                    group_score: Some(group_scores[*index]),
                    text: Some(mention.text.clone()),
                    r#type: Some(group.entity_type.to_string()),
                    mention_sentiment: Some(MentionSentiment {
                        sentiment: Some(sentiment),
                        sentiment_score: Some(sentiment_score),
                    }),
                    begin_offset: offsets.get(&mention.start).copied(),
                    end_offset: offsets.get(&mention.end).copied(),
                }
            })
            .collect();
        let first = order.first().map_or(0, |index| mentions[*index].start);
        entities.push((
            first,
            TargetedSentimentEntity {
                descriptive_mention_index: Some(descriptive_mention_index(&order, &mentions)),
                mentions: Some(rendered),
            },
        ));
    }

    // Entities in the order the document first mentions them.
    entities.sort_by_key(|(first, _)| *first);
    entities.into_iter().map(|(_, entity)| entity).collect()
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DetectTargetedSentimentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-comprehend-comprehend-20171127.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DetectTargetedSentimentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.comprehend#DetectTargetedSentiment")]
#[derive(Debug)]
pub enum DetectTargetedSentimentErrorModeled {
    InternalServerException(InternalServerException),
    InvalidRequestException(InvalidRequestException),
    TextSizeLimitExceededException(TextSizeLimitExceededException),
    UnsupportedLanguageException(UnsupportedLanguageException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::detect_targeted_sentiment::DetectTargetedSentimentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DetectTargetedSentimentError = crate::scaffolded::Scaffolded<DetectTargetedSentimentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DetectTargetedSentimentErrorModeled> for DetectTargetedSentimentError {
    fn from(modeled: DetectTargetedSentimentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DetectTargetedSentiment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DetectTargetedSentiment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DetectTargetedSentiment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DetectTargetedSentiment<Eng>
{
    type Input = DetectTargetedSentimentInput;
    type Output = DetectTargetedSentimentOutput;
    type Error = DetectTargetedSentimentError;

    const NAME: &'static str = "DetectTargetedSentiment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_TARGETED_SENTIMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_TARGETED_SENTIMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Synchronous inference holds no state: the entities are a function of `Text`
        // and `LanguageCode` alone, so repeating a call returns the same offsets.
        let _ = (ctx, &self.engine);

        // The model's `@length` floor of 1 is enforced at the boundary; the byte
        // ceiling it documents in prose is not, so it is enforced here.
        if input.text.len() > MAX_TEXT_BYTES {
            return Err(
                DetectTargetedSentimentErrorModeled::TextSizeLimitExceededException(
                    TextSizeLimitExceededException {
                        message: Some(format!(
                            "Text is {} bytes, which exceeds the {MAX_TEXT_BYTES} byte limit \
                             for DetectTargetedSentiment. Use a smaller document.",
                            input.text.len()
                        )),
                    },
                )
                .into(),
            );
        }

        // The `LanguageCode` enum the member targets is enforced at the boundary;
        // this operation's OWN set is narrower than that enum ("Currently, English
        // is the only supported language"), so the narrowing is enforced here, as
        // the same kind of refusal the boundary validator answers an out-of-enum
        // value with: `ValidationException`, in the validator's own enum-constraint
        // wording, naming the member path, the value that broke it and the set it
        // had to come from ([`crate::state::enum_value_set_refusal`], the one
        // sentence every operation of this service states that refusal with). The
        // modelled `UnsupportedLanguageException` is not this condition: it reports
        // a language the service cannot process, where this is a value the
        // operation does not accept at all.
        if !ANALYSABLE_LANGUAGES.contains(&input.language_code.as_str()) {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::enum_value_set_refusal(
                    crate::state::LANGUAGE_CODE_PATH,
                    &input.language_code,
                    ANALYSABLE_LANGUAGES,
                ),
            ));
        }

        Ok(DetectTargetedSentimentOutput {
            entities: Some(analyse(&input.text, &input.language_code)),
        })
    }
}
