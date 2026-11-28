// SPDX-License-Identifier: CC0-1.0

//! The rules a supplied TAG MAP is held to, wherever it arrives.
//!
//! A create's tag map and a `TagResource` tag map are judged by the one statement here
//! ([`invalid_tags_message`]): the modelled `TagKey` bounds, the reserved `aws:`
//! namespace, and the cap a resource's tag set is bounded by
//! ([`MAX_TAGS_PER_RESOURCE`]) — which is also what keeps a stored record's tag map from
//! growing by one entry per request forever.

use super::*;

/// The `@length` bounds `TagMap`'s KEY carries: a `TagKey` of length 1..128
/// (`smithy_query(query="shape",shape="TagMap")`).
///
/// A map's key is the one constrained value the framework's boundary walk does not
/// reach — it captures an entry's key as the path segment the entry's VALUE is reported
/// under rather than checking it ("the key itself is not a constrained leaf",
/// `ValidatingSerializer::write_leaf`) — so an operation that takes a `TagMap` enforces
/// the key's own bounds itself, with the modelled `ValidationException` the boundary
/// would have answered with. The value's bounds need no such copy: the boundary already
/// refuses a `TagValue` past its maximum.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// The longest `TagKey` the model admits; see [`TAG_KEY_MIN_LENGTH`].
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// The refusal a tag key outside [`TAG_KEY_MIN_LENGTH`]..[`TAG_KEY_MAX_LENGTH`] earns.
///
/// The rule is about ONE value's own validity, so the message states the constraint the
/// value failed and the length the request's key actually had — built from the request,
/// as a constraint refusal is. The key itself is NOT echoed: its length is what is at
/// fault and a caller controls it, so echoing it would turn an arbitrarily long key into
/// an arbitrarily long message.
pub fn invalid_tag_key_message(length: usize) -> String {
    format!(
        "A tag key must have length greater than or equal to {TAG_KEY_MIN_LENGTH} and less \
         than or equal to {TAG_KEY_MAX_LENGTH}; this request carries a key of length {length}."
    )
}

/// The refusal a `TagResource` whose tags would take a resource past
/// [`MAX_TAGS_PER_RESOURCE`] earns.
///
/// The rule is about ONE value's own validity — how many tags the request asks the
/// resource to carry — so the message names that count and the cap it exceeds, which is
/// what a caller has to know to fix the request. `resulting` is the size of the tag set
/// the request would leave behind, so a request that only re-sets keys the resource
/// already holds is never reported as adding to it.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "These tags would leave the resource with {resulting} tags, more than the \
         {MAX_TAGS_PER_RESOURCE} tags a resource can have. Remove tags before adding more."
    )
}

/// The tag-key prefix AWS reserves for its own tags.
///
/// AWS's tag restrictions reserve this prefix on every taggable resource: the keys under
/// it name AWS-generated tags, so a user request may not supply one and a request that
/// does is refused rather than stored. The restriction covers editing and DELETING such
/// a tag as well as creating one, so a key in this namespace is refused wherever a
/// request names it — in a supplied tag map ([`invalid_tags_message`]) and in the keys an
/// untag request asks the service to remove alike. The model constrains a `TagKey`'s length and
/// nothing else, so the reserved namespace is the service's own rule to enforce, with
/// the `ValidationException` (HTTP 400) a request that "fails to satisfy the constraints
/// specified by the service" earns.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// The prefix is reserved at the START of a key and the namespace is claimed whatever
/// LETTER CASE the prefix arrives in, so a key whose leading characters are that prefix
/// ignoring case asks for an AWS-generated tag and is refused. A key that merely
/// contains the prefix further along, or begins with its letters without the separator,
/// is a key of the caller's own namespace and is stored like any other.
///
/// Case is ignored only in deciding WHICH namespace a key claims: a tag key is still
/// case-sensitive as an identifier, so two keys differing in case remain two keys.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// The refusal a tag key in the reserved namespace earns.
///
/// The rule is about ONE value's own validity, so the message is built from the request:
/// it names the key at fault and the prefix that is reserved, which is what a caller has
/// to know to fix the request. Naming the key is bounded because the key's own
/// `@length` is decided FIRST ([`invalid_tags_message`]), so by here it is at most
/// [`TAG_KEY_MAX_LENGTH`] characters.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!(
        "The tag key '{key}' begins with '{RESERVED_TAG_KEY_PREFIX}', a prefix reserved for \
         AWS-generated tags, so it cannot be specified in a request."
    )
}

/// The refusal the `tags` of a request earn, or `None` when the request asks for a tag
/// set this service stores.
///
/// ONE copy of every rule a supplied tag map has to satisfy, so `CreateFeed`,
/// `CreateDictionary` and `TagResource` cannot drift in which tags they accept: a key
/// outside the modelled `TagKey` bounds ([`TAG_KEY_MIN_LENGTH`]), a key in the reserved
/// namespace ([`RESERVED_TAG_KEY_PREFIX`]), and more tags than a resource may carry
/// ([`MAX_TAGS_PER_RESOURCE`]). The request's own validity is decided before any
/// resource is looked up, so a malformed tag map is refused whatever resource the rest
/// of the request names.
///
/// `TagResource` ACCUMULATES onto a stored set, so it checks the resulting size against
/// the cap as well ([`too_many_tags_message`]); the size checked here is the one the
/// request itself asks for, which is the whole set of a resource a create mints.
pub fn invalid_tags_message(tags: &HashMap<String, String>) -> Option<String> {
    // Several keys can be at fault at once and one request earns one answer, so the key
    // reported is chosen in key order rather than by the map's iteration order.
    let mut keys: Vec<&str> = tags.keys().map(String::as_str).collect();
    keys.sort_unstable();

    // The modelled bounds of the key first: `@length` counts characters, not bytes,
    // exactly as the boundary walk does for the values it reaches.
    if let Some(length) = keys
        .iter()
        .map(|key| key.chars().count())
        .find(|length| !(TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(length))
    {
        return Some(invalid_tag_key_message(length));
    }

    if let Some(key) = keys.iter().find(|key| is_reserved_tag_key(key)) {
        return Some(reserved_tag_key_message(key));
    }

    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Some(too_many_tags_message(tags.len()));
    }

    None
}
