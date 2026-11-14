// SPDX-License-Identifier: CC0-1.0

//! What the AUTOMATIC LANGUAGE IDENTIFICATION this service performs reads: the
//! language the stored media object a job was pointed at evidences, and how that
//! evidence is narrowed to a language the request allowed.
//!
//! Two job families ask the service to identify a language rather than naming one: a
//! transcription job started with `IdentifyLanguage` or `IdentifyMultipleLanguages`,
//! and a Call Analytics job, which carries no language member at all. For both, the
//! finished job's `LanguageCode` is "the language code used to create your
//! transcription job" — the service's own determination about the MEDIA it processed
//! ("the confidence score associated with the language identified in your media file",
//! `smithy_query(query="doc", shape="TranscriptionJob",
//! member="IdentifiedLanguageScore")`) — so it is a function of the media object the
//! job was pointed at, not of the order the request happened to list its options in:
//! two jobs identical but for their media may identify different languages.
//!
//! No audio is decoded here (the service transcribes nothing, see
//! [`super::jobs`]), so the determination is made from what the stored object itself
//! EVIDENCES about its language, in the order the evidence is authoritative:
//!
//! 1. the language the stored object declares — the `Content-Language` the object
//!    store holds for it, which is where stored content states "the natural language
//!    of the intended audience";
//! 2. failing that, the language tag the object's own address carries
//!    ([`address_language_tag`]) — the same kind of evidence the format of a stored
//!    media object is read from in this service
//!    ([`super::media_format_of`]: "nothing here opens that file, and the one thing
//!    the request says about it is the name it is stored under").
//!
//! Evidence is only ever taken from an object this emulator's object store ANSWERED
//! for ([`super::MediaVerdict::Resolved`]): the service has to have seen the media to
//! have identified anything in it. Media that evidences no language leaves
//! identification with nothing to go on, and it answers from the options the request
//! allowed, exactly as it did before any evidence existed.

use super::*;
use crate::ops::types::LANGUAGE_CODE_VALUES;

/// The language a stored media object declares for itself, as a tag
/// [`narrowed_identified_language`] can narrow — `None` for a declaration naming no
/// language this service models.
///
/// `Content-Language` is a list; the FIRST entry is the language of the audio for a
/// media object that names more than one, and this service identifies ONE language per
/// media file ("use this parameter if your media file contains only one language",
/// model doc for `StartTranscriptionJobRequest$IdentifyLanguage`).
pub fn declared_language_tag(content_language: Option<&str>) -> Option<String> {
    let declared = content_language?
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    is_language_tag(&declared).then_some(declared)
}

/// The language tag the ADDRESS of a stored media object carries — `None` for an
/// address carrying none.
///
/// A language tag is a tag of the model's own `LanguageCode` catalog
/// ([`crate::ops::types::LANGUAGE_CODE_VALUES`]): a whole code (`fr-FR`), or the
/// primary language subtag a group of those codes share (`fr`), which is how a stored
/// object is commonly labelled with its language. It counts only where the address
/// states it on its own — delimited by something that is not a letter or a digit on
/// both sides — so a tag that merely happens to be spelled inside a longer word is not
/// read as one.
///
/// The LAST tag the address carries is the one it evidences, and the longest tag at
/// that position: the name an object is stored under is more specific than the prefixes
/// above it, and `clip-fr-FR` states the whole code rather than the bare subtag that
/// ends it. The final extension is not searched — that is what the object's FORMAT is
/// read from ([`super::media_format_of`]).
pub fn address_language_tag(uri: &str) -> Option<String> {
    let address = uri.trim().split(['?', '#']).next().unwrap_or_default();
    // The final extension evidences the format, not the language.
    let searched = match address.rsplit_once('.') {
        Some((stem, extension)) if !extension.contains('/') => stem,
        _ => address,
    }
    .to_ascii_lowercase();
    let mut best: Option<(usize, String)> = None;
    for tag in language_tags() {
        for (offset, _) in searched.match_indices(tag.as_str()) {
            if !is_delimited(&searched, offset, tag.len()) {
                continue;
            }
            let end = offset + tag.len();
            // End-most first, then longest: both orders are the same comparison on
            // (end, length), because a longer tag ending at the same place starts
            // earlier.
            if best
                .as_ref()
                .is_none_or(|(best_end, best_tag)| (end, tag.len()) > (*best_end, best_tag.len()))
            {
                best = Some((end, tag.clone()));
            }
        }
    }
    best.map(|(_, tag)| tag)
}

/// The language a finished job IDENTIFIED, given what its media evidenced (`tag`,
/// from [`declared_language_tag`] or [`address_language_tag`]) and the
/// `LanguageOptions` the request allowed identification to choose between — `None`
/// where that evidence names no language identification was allowed to answer with.
///
/// "Including `LanguageOptions` restricts [identification] to only the language options
/// that you specify" (model doc for `StartTranscriptionJobRequest$IdentifyLanguage`),
/// so an option list is what the answer is drawn from: the option the evidence names,
/// or — where the evidence is a bare primary subtag — the first option sharing that
/// subtag, since a subtag names a language and an option names the regional variant of
/// it that identification may report. A request that listed no options narrows nothing,
/// and then only a WHOLE code is evidence enough to answer with: a bare subtag leaves
/// the variant unstated, and this service reports a modelled `LanguageCode`, which
/// always names one.
pub fn narrowed_identified_language(
    tag: &str,
    language_options: Option<&[String]>,
) -> Option<String> {
    let tag = tag.trim().to_ascii_lowercase();
    let subtag = primary_subtag(&tag);
    match language_options.filter(|options| !options.is_empty()) {
        Some(options) => options
            .iter()
            .find(|option| option.to_ascii_lowercase() == tag)
            .or_else(|| {
                options
                    .iter()
                    .find(|option| primary_subtag(&option.to_ascii_lowercase()) == subtag)
            })
            .cloned(),
        None => LANGUAGE_CODE_VALUES
            .iter()
            .find(|code| code.to_ascii_lowercase() == tag)
            .map(|code| (*code).to_string()),
    }
}

/// The primary language subtag of a language tag: everything before its first `-`
/// (RFC 5646 §2.2.1, the shape every code of the modelled catalog follows).
fn primary_subtag(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

/// Whether `tag` is a language tag of this service's catalog: a modelled `LanguageCode`
/// or the primary subtag of one, compared in lower case.
fn is_language_tag(tag: &str) -> bool {
    !tag.is_empty() && language_tags().iter().any(|known| known == tag)
}

/// Every language tag a media object may be labelled with, in lower case: each code the
/// model's `LanguageCode` enum carries and each primary subtag those codes share.
fn language_tags() -> Vec<String> {
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for code in LANGUAGE_CODE_VALUES {
        let code = code.to_ascii_lowercase();
        tags.insert(primary_subtag(&code).to_string());
        tags.insert(code);
    }
    tags.into_iter().collect()
}

/// Whether the `length` bytes of `text` at `offset` stand on their own: bounded by the
/// ends of the text or by something that is neither a letter nor a digit (a path
/// separator, a hyphen, an underscore, a dot).
fn is_delimited(text: &str, offset: usize, length: usize) -> bool {
    let before = text[..offset].chars().next_back();
    let after = text[offset + length..].chars().next();
    [before, after]
        .into_iter()
        .flatten()
        .all(|boundary| !boundary.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_evidences_the_language_tag_it_states() {
        let cases: &[(&str, Option<&str>)] = &[
            // The bare subtag a stored object is labelled with.
            ("s3://bucket/speech-fr.mp3", Some("fr")),
            // A whole code, which wins over the bare subtag ending it.
            ("s3://bucket/clip-fr-FR.mp3", Some("fr-fr")),
            ("s3://bucket/es-ES/interview.wav", Some("es-es")),
            // The name an object is stored under is more specific than its prefixes.
            ("s3://bucket/fr/call-ja-JP.flac", Some("ja-jp")),
            // A tag spelled inside a longer word is not one.
            ("s3://bucket/afresh.mp3", None),
            ("s3://bucket/dictation.wav", None),
            // The extension evidences the format, not the language.
            ("s3://bucket/recording.amr", None),
        ];
        for (uri, expected) in cases {
            let actual = address_language_tag(uri);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {uri:?}: expected the evidenced tag {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn evidence_is_narrowed_to_a_language_the_request_allowed() {
        let options = ["en-US".to_string(), "es-ES".to_string(), "fr-FR".to_string()];
        /// One row: the tag the media evidenced, the options the request allowed and
        /// the code identification may answer with.
        type Row<'a> = (&'a str, Option<&'a [String]>, Option<&'a str>);
        let cases: &[Row<'_>] = &[
            // A subtag names the option that is a variant of it.
            ("fr", Some(&options), Some("fr-FR")),
            // A whole code names the option that spells it.
            ("es-es", Some(&options), Some("es-ES")),
            // A code of another variant still names the language.
            ("es-mx", Some(&options), Some("es-ES")),
            // A language no option allowed is not something identification may answer.
            ("ja-jp", Some(&options), None),
            // With nothing narrowing the choice, a whole code answers and a bare
            // subtag leaves the variant unstated.
            ("ja-jp", None, Some("ja-JP")),
            ("ja", None, None),
        ];
        for (tag, allowed, expected) in cases {
            let actual = narrowed_identified_language(tag, *allowed);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {tag:?} over {allowed:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_declared_language_is_read_from_the_stored_declaration() {
        let cases: &[(Option<&str>, Option<&str>)] = &[
            (Some("fr-FR"), Some("fr-fr")),
            (Some("ja"), Some("ja")),
            (Some(" de-DE , en-US "), Some("de-de")),
            (Some("klingon"), None),
            (Some(""), None),
            (None, None),
        ];
        for (declared, expected) in cases {
            let actual = declared_language_tag(*declared);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {declared:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
