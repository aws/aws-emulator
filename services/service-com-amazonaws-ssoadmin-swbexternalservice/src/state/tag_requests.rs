// SPDX-License-Identifier: CC0-1.0

//! The ONE validation of the caller-supplied `Tags` of a write, asked before anything is
//! resolved or stored.
//!
//! # The rule
//!
//! A tag set is a MAP keyed by `Tag$Key`: a resource carries one value per key. So a
//! request's tag list names each key AT MOST ONCE, and one that repeats a key is refused
//! with the modelled `ValidationException` (HTTP 400) rather than stored with one of the two
//! values silently winning — the same reason an ABAC configuration refuses a repeated
//! attribute key ([`super::duplicate_attribute_key`]). Keys are compared exactly, because
//! AWS tag keys are case-sensitive, so two keys differing only in case are two keys.
//!
//! The model cannot express it: `TagList`
//! (`smithy_query(query="shape",shape="Tag,TagList")`) declares a length bound and no
//! `@uniqueItems`, so the boundary validator passes a list carrying one key twice and the
//! uniqueness is the handler's to ask.
//!
//! WHERE IT IS ASKED: by every operation of this service that takes caller-supplied tags —
//! the four creates that carry `Tags` and `TagResource` — through this one entry point,
//! which also asks the SYSTEM-tag question ([`super::mutates_system_tag`]) those writes
//! share, so one tag list is judged the same way whichever operation the client called.
//! Judged on the REQUEST alone, for the whole list at once, so a refused write stores
//! nothing: a create mints no resource, and a tagging write leaves the set the resource
//! already carries exactly as it was.

use crate::ops::types::Tag;

use super::{mutates_system_tag, SYSTEM_TAG_MUTATION_MESSAGE};

/// The first `Tag$Key` that `tags` carries more than once, if any.
///
/// The FIRST repeat, in the order the request sent its tags, so the refusal names the key
/// the walk reached — and one key is enough: the write is refused whole.
#[must_use]
pub fn duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    tags.iter()
        .find(|tag| !seen.insert(tag.key.as_str()))
        .map(|tag| tag.key.as_str())
}

/// The refusal a request naming one tag key twice earns, as the message of the modelled
/// `ValidationException`.
///
/// Built from the request: the fixed half states the requirement — a tag key is unique
/// within one request, the AWS-wide tagging rule — and the key half is the key the request
/// repeated.
#[must_use]
pub fn duplicate_tag_key_message(key: &str) -> String {
    format!("Duplicate tag key {key} found in the request. Tag keys must be unique.")
}

/// The refusal the caller-supplied `Tags` of a write earns, or `None` when the list is one
/// the service serves.
///
/// The two questions a tag list is asked, in the order the module header states: a key in
/// the namespace Amazon Web Services reserves for itself may not be written at all, and
/// then no key may be named twice. An empty list — which `TagList`'s lower bound of 0
/// admits, and which a create that carries no `Tags` is read as — earns neither.
#[must_use]
pub fn tag_list_refusal(tags: &[Tag]) -> Option<String> {
    if mutates_system_tag(tags.iter().map(|tag| tag.key.as_str())) {
        return Some(SYSTEM_TAG_MUTATION_MESSAGE.to_string());
    }
    duplicate_tag_key(tags).map(duplicate_tag_key_message)
}
