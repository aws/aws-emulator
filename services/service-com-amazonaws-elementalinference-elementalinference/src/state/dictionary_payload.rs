// SPDX-License-Identifier: CC0-1.0

//! A dictionary's entries PAYLOAD: the canonical document this service stores and
//! exports one as, and the ARN it is named by.
//!
//! The payload a client sends states an entry list, and what the dictionary holds is that
//! LIST rather than the bytes the client happened to format — so every client that states
//! one entry list reads one document back. The parse is total: a payload that states no
//! entry list is kept as submitted rather than rejected or guessed at.

use super::*;

/// The input member a dictionary's language is carried by
/// (`CreateDictionaryRequest$language`, `UpdateDictionaryRequest$language`), as a
/// refusal names it.
pub(super) const LANGUAGE_MEMBER: &str = "language";

/// The member an entry of a dictionary payload carries its word or phrase under, as the
/// exported document spells it.
pub(super) const ENTRY_CONTENT: &str = "content";

/// The member an entry of a dictionary payload carries its alternate pronunciations
/// under, as the exported document spells it.
pub(super) const ENTRY_SOUNDS_LIKE: &str = "sounds_like";

/// One entry of a dictionary payload: a word or phrase, and the alternates it sounds
/// like.
#[derive(Debug, Clone)]
pub(super) struct DictionaryEntry {
    pub(super) content: String,
    /// The alternates the entry was stored with, or `None` when it carries no list at
    /// all — a member the service reports only for an entry that has one.
    pub(super) sounds_like: Option<Vec<String>>,
}

/// The CANONICAL form of a dictionary entries payload: the entry list as this service
/// renders it, whatever formatting the client that stored it used.
///
/// The service does not keep a submitted payload's bytes: the payload states an entry
/// list, and what a reader is given back is that list rendered as a COMPACT JSON
/// document — no whitespace between tokens — one object per entry carrying the entry's
/// content and, when it has one, its alternates, in the order the entries were
/// supplied. So two clients that state the same entry list store the same dictionary
/// and export the same document, however either of them spaced or indented its
/// request.
///
/// A payload that states no entry list at all — it is not a JSON array of entry
/// objects — has no entry list to render, so it is kept exactly as submitted.
pub fn canonical_entries(payload: &str) -> String {
    match parsed_entries(payload) {
        Some(entries) => rendered_entries(&entries),
        None => payload.to_string(),
    }
}

/// The entry list a payload states, or `None` when it states none.
pub(super) fn parsed_entries(payload: &str) -> Option<Vec<DictionaryEntry>> {
    let serde_json::Value::Array(elements) = serde_json::from_str::<serde_json::Value>(payload).ok()?
    else {
        return None;
    };
    elements.iter().map(parsed_entry).collect()
}

/// The one entry an element of the payload states, or `None` when it states none:
/// anything but an object carrying a string content and an optional list of string
/// alternates, and nothing else.
pub(super) fn parsed_entry(element: &serde_json::Value) -> Option<DictionaryEntry> {
    let members = element.as_object()?;
    if members
        .keys()
        .any(|member| member != ENTRY_CONTENT && member != ENTRY_SOUNDS_LIKE)
    {
        return None;
    }
    let content = members.get(ENTRY_CONTENT)?.as_str()?.to_string();
    let sounds_like = match members.get(ENTRY_SOUNDS_LIKE) {
        Some(stated) => Some(
            stated
                .as_array()?
                .iter()
                .map(|alternate| alternate.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()?,
        ),
        None => None,
    };
    Some(DictionaryEntry {
        content,
        sounds_like,
    })
}

/// `entries` as the compact document this service exports.
pub(super) fn rendered_entries(entries: &[DictionaryEntry]) -> String {
    let rendered: Vec<String> = entries
        .iter()
        .map(|entry| {
            let content = format!("\"{ENTRY_CONTENT}\":{}", json_string(&entry.content));
            match &entry.sounds_like {
                Some(alternates) => {
                    let list: Vec<String> = alternates.iter().map(|a| json_string(a)).collect();
                    format!(
                        "{{{content},\"{ENTRY_SOUNDS_LIKE}\":[{}]}}",
                        list.join(",")
                    )
                }
                None => format!("{{{content}}}"),
            }
        })
        .collect();
    format!("[{}]", rendered.join(","))
}

/// `value` as a JSON string literal, escaped the way a JSON document escapes one.
pub(super) fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// The ARN of a dictionary in `region`:
/// `arn:aws:elemental-inference:<region>:<account>:dictionary/<id>`, the form the
/// modelled `resourceArn` pattern `…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` admits.
pub fn dictionary_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("dictionary/{id}"))
}
