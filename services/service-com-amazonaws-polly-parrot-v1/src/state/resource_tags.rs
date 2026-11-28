// SPDX-License-Identifier: CC0-1.0

//! WHICH resource a caller-supplied ARN addresses, and what a tag set has to satisfy.
//!
//! The three tagging operations (`TagResource`, `UntagResource`,
//! `ListTagsForResource`) and the two operations that take `Tags` at creation time
//! (`PutLexicon`, `CreateVoice`) all answer by the same rules, so the rules live here
//! once rather than in five handlers:
//!
//! * an `AmazonResourceName` the model constrains by LENGTH alone has to be read as an
//!   ARN by the service. It addresses something here only when it is a well-formed ARN
//!   ([`service_common::arn`]) of THIS vendor, in the account and Region of the request,
//!   naming one of the two resource types the model's `ResourceType` enum declares. The
//!   model's own words for anything else are `InvalidTagParameterException`: "Specify
//!   the ARN of a supported resource in the Amazon Web Services account and Region of
//!   the request." A well-formed ARN of a resource that does not exist is a different
//!   answer — `ResourceNotFoundException` — and that is the distinction
//!   [`addressed_resource`] draws;
//! * a tag key may not sit in the prefix AWS reserves for its own tags
//!   ([`RESERVED_TAG_KEY_PREFIX`]); the model states the same rule for the one other
//!   key-value map it declares ("Don't use keys that start with `aws:`, which are
//!   reserved" — `CustomerManagedEncryption$EncryptionContext`). The key and value
//!   LENGTHS are modelled (`TagKey` is 1..128, `TagValue` 0..256), so the request
//!   boundary refuses those before a handler runs;
//! * a resource carries at most [`MAX_TAGS_PER_RESOURCE`] tags, which is the number
//!   `TooManyTagsException` states.
//!
//! Nothing here reads or writes the store: a resource's tags live on the resource
//! itself, so one read of a lexicon or a voice answers both what it is and how it is
//! tagged.

use std::collections::BTreeMap;

/// The `ResourceType` of a pronunciation lexicon.
pub const RESOURCE_TYPE_LEXICON: &str = "lexicon";
/// The `ResourceType` of a custom voice.
pub const RESOURCE_TYPE_VOICE: &str = "voice";

/// The ARN namespace of this service — the `arnNamespace` its model publishes, which is
/// the third field of every ARN it mints and the only one it addresses a resource
/// through.
pub const ARN_VENDOR: &str = "polly";

/// The most tags one resource carries.
///
/// Not a number this emulator chooses: `TooManyTagsException` states it — "The maximum
/// number of tags that can be applied to a resource is 50" — so a request that would
/// leave a resource holding more than this is refused with that error.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix AWS reserves for its own tags.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Which resource of this service an ARN addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressedResource {
    /// The `ResourceType` the ARN names: [`RESOURCE_TYPE_LEXICON`] or
    /// [`RESOURCE_TYPE_VOICE`].
    pub resource_type: &'static str,
    /// The resource's own identifier — a lexicon's name, a custom voice's id.
    pub id: String,
}

/// The resource `arn` addresses in `region`, or `None` for an ARN that addresses no
/// resource of this service at all.
///
/// `None` is the `InvalidTagParameterException` case and says nothing about whether a
/// resource EXISTS: an ARN that passes here still has to be looked up, and a lookup that
/// finds nothing is `ResourceNotFoundException`. The Region and the account are part of
/// the reading because a resource of another Region or another account is not one this
/// request can address — the ARN has to name "a supported resource in the Amazon Web
/// Services account and Region of the request".
pub fn addressed_resource(arn: &str, region: &str) -> Option<AddressedResource> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != ARN_VENDOR
        || parsed.region != region
        || parsed.account != ::engine::emulator_account_id()
    {
        return None;
    }
    let resource_type = match parsed.resource_type() {
        RESOURCE_TYPE_LEXICON => RESOURCE_TYPE_LEXICON,
        RESOURCE_TYPE_VOICE => RESOURCE_TYPE_VOICE,
        _ => return None,
    };
    // Every ARN this service mints writes the resource as `<type>/<id>`, so the id is
    // what follows the one separator; an ARN carrying the type alone addresses no
    // single resource.
    let id = parsed.resource.strip_prefix(resource_type)?.strip_prefix('/')?;
    (!id.is_empty()).then(|| AddressedResource {
        resource_type,
        id: id.to_string(),
    })
}

/// Whether every key of `tags` is one a caller may set.
pub fn tag_keys_are_settable<'a>(keys: impl IntoIterator<Item = &'a String>) -> bool {
    keys.into_iter()
        .all(|key| !key.starts_with(RESERVED_TAG_KEY_PREFIX))
}

/// The tag set a resource already holding `existing` is left with after `incoming` is
/// applied, or `None` when that would pass [`MAX_TAGS_PER_RESOURCE`].
///
/// "Existing tags on a resource are not changed" by a tag that is not in the request
/// (`TagResource`), and a key already on the resource is overwritten rather than added,
/// so what the limit counts is the UNION of the two key sets.
pub fn tags_after_adding(
    existing: &BTreeMap<String, String>,
    incoming: &std::collections::HashMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    let mut merged = existing.clone();
    for (key, value) in incoming {
        merged.insert(key.clone(), value.clone());
    }
    (merged.len() <= MAX_TAGS_PER_RESOURCE).then_some(merged)
}

/// The tags a resource reports: the stored set, or `None` when it carries none.
///
/// `TagMap` is declared with a minimum length of one entry, so the empty tag set is an
/// ABSENT member rather than an empty object — the same rule every optional list member
/// of this service's reported shapes follows ([`crate::state::reported_list`]).
pub fn reported_tags(
    tags: &BTreeMap<String, String>,
) -> Option<std::collections::HashMap<String, String>> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an ARN addresses a resource of this service only when it is a well-formed
    /// ARN of this vendor, in the account and Region of the request, naming one of the
    /// two modelled resource types with an id.
    #[test]
    fn only_a_polly_resource_arn_of_this_account_and_region_addresses_anything() {
        let region = "eu-west-1";
        let account = ::engine::emulator_account_id();
        let rows: &[(&str, String, Option<AddressedResource>)] = &[
            (
                "a lexicon of this account and Region",
                format!("arn:aws:polly:{region}:{account}:lexicon/Colors"),
                Some(AddressedResource {
                    resource_type: RESOURCE_TYPE_LEXICON,
                    id: "Colors".to_string(),
                }),
            ),
            (
                "a custom voice of this account and Region",
                format!("arn:aws:polly:{region}:{account}:voice/vc-9a8fa85dd3"),
                Some(AddressedResource {
                    resource_type: RESOURCE_TYPE_VOICE,
                    id: "vc-9a8fa85dd3".to_string(),
                }),
            ),
            (
                "another Region's lexicon",
                format!("arn:aws:polly:us-east-1:{account}:lexicon/Colors"),
                None,
            ),
            (
                "another account's lexicon",
                format!("arn:aws:polly:{region}:111122223333:lexicon/Colors"),
                None,
            ),
            (
                "another vendor's resource",
                format!("arn:aws:s3:{region}:{account}:lexicon/Colors"),
                None,
            ),
            (
                "a resource type this service does not tag",
                format!("arn:aws:polly:{region}:{account}:synthesisTask/abc"),
                None,
            ),
            (
                "the resource type with no id",
                format!("arn:aws:polly:{region}:{account}:lexicon"),
                None,
            ),
            (
                "the resource type with an empty id",
                format!("arn:aws:polly:{region}:{account}:lexicon/"),
                None,
            ),
            ("not an ARN at all", "Colors".to_string(), None),
            (
                "an ARN missing its fields",
                "arn:aws:polly:lexicon/Colors".to_string(),
                None,
            ),
        ];

        for (case, arn, expected) in rows {
            let actual = addressed_resource(arn, region);
            assert_eq!(
                actual, *expected,
                "case {case:?}: ARN {arn:?} addressed {actual:?}"
            );
        }
    }

    /// RULE: the limit counts the union of the stored and the requested keys, so
    /// overwriting a key adds nothing to the count.
    #[test]
    fn the_tag_limit_counts_the_union_of_the_stored_and_requested_keys() {
        let full: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
            .map(|i| (format!("key{i}"), i.to_string()))
            .collect();

        let overwrite: std::collections::HashMap<String, String> =
            [("key0".to_string(), "new".to_string())].into_iter().collect();
        let merged = tags_after_adding(&full, &overwrite)
            .expect("overwriting a stored key adds no entry, so the limit holds");
        assert_eq!(
            merged.len(),
            MAX_TAGS_PER_RESOURCE,
            "overwriting must not change the count, got {merged:?}"
        );
        assert_eq!(
            merged.get("key0").map(String::as_str),
            Some("new"),
            "the requested value must replace the stored one, got {merged:?}"
        );

        let one_more: std::collections::HashMap<String, String> =
            [("extra".to_string(), String::new())].into_iter().collect();
        assert_eq!(
            tags_after_adding(&full, &one_more),
            None,
            "one key beyond the limit must be refused"
        );
        assert!(
            tags_after_adding(&BTreeMap::new(), &one_more).is_some(),
            "a resource with no tags takes the same key"
        );
    }

    /// RULE: a key in the reserved prefix is not one a caller may set; everything else
    /// is.
    #[test]
    fn a_reserved_key_is_not_settable() {
        for (case, keys, expected) in [
            ("an ordinary key", vec!["Project".to_string()], true),
            ("a key naming aws without the colon", vec!["aws".to_string()], true),
            ("a reserved key", vec!["aws:created-by".to_string()], false),
            (
                "one reserved key among settable ones",
                vec!["Project".to_string(), "aws:cloudformation:stack".to_string()],
                false,
            ),
            ("no keys at all", Vec::new(), true),
        ] {
            assert_eq!(
                tag_keys_are_settable(keys.iter()),
                expected,
                "case {case:?}: wrong verdict for {keys:?}"
            );
        }
    }

    /// RULE: the empty tag set is an absent member, not an empty map.
    #[test]
    fn no_tags_is_an_absent_member() {
        assert_eq!(
            reported_tags(&BTreeMap::new()),
            None,
            "a resource with no tags reports no map at all"
        );
        let one: BTreeMap<String, String> =
            [("Project".to_string(), "Parrot".to_string())].into_iter().collect();
        assert_eq!(
            reported_tags(&one).map(|tags| tags.len()),
            Some(1),
            "a tagged resource reports its tags"
        );
    }
}
