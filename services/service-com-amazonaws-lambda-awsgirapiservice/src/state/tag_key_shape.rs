// SPDX-License-Identifier: CC0-1.0

//! The constraints a TAG KEY carries, and the refusal a request naming one outside them
//! gets — the one half of a tag map's modelled shape no request boundary applies.
//!
//! # Why a handler applies it
//!
//! The framework's constraint walk captures a map entry's key as the path segment for that
//! entry's VALUE and never checks the key against the key shape's own constraints
//! (`vendor/smithy-schema-server/src/constraints.rs`, `write_leaf`: "the key itself is not a
//! constrained leaf"). So every `@length` and `@pattern` the key shape of a map declares has
//! to be applied by the operation that accepts the map; nothing else in the request walk
//! will. A key outside them is otherwise STORED and reported back as persisted, which tells
//! a caller an unusable tag succeeded.
//!
//! # What is stated here
//!
//! The key shape's constraints, re-declared once ([`TAG_KEY_PATTERN`] and the two bounds
//! beside it) as the trait map the framework's own walk reads, so the DECISION is that
//! walk's — the same regular-expression engine and the same character counting a reported
//! member gets — and the sentence is `service-common`'s one wording of a map-key clause.
//! Every operation that accepts a `Tags` map asks [`tag_key_shape_refusal`], so one tag key
//! cannot be judged two ways.
//!
//! The rule is about the SHAPE of the request, so it is answered the way the boundary walk
//! answers one (a 400 `ValidationException` over the clause, through
//! [`crate::scaffolded::Scaffolded::validation`]), and it is applied AFTER the rules about
//! what a tag MEANS — the reserved namespace, an empty key and a key named twice
//! ([`super::tag_keys_are_storable_from`]), which carry Lambda's own sentence about a tag it
//! will not store ([`super::INVALID_TAGS_MESSAGE`]) and claim the conditions they name: an
//! empty key is refused as the tag it is not, rather than as a key under the shape's length
//! minimum. What reaches the clause here is a key that is named and outside the shape.

/// `TagKey`'s modelled `@length` minimum, in characters.
///
/// Source: `smithy_query(query="shape", shape="TagKey")` — `@length(min: 1, max: 128)` and
/// the `@pattern` below, which is also what the `Tags` map declares for its key and what
/// `TagKeyList`'s element declares (the member `UntagResource` takes, whose ELEMENTS the
/// boundary walk does reach).
const TAG_KEY_MIN_LEN: i64 = 1;

/// `TagKey`'s modelled `@length` maximum, in characters. See [`TAG_KEY_MIN_LEN`].
const TAG_KEY_MAX_LEN: i64 = 128;

/// `TagKey`'s modelled `@pattern`, exactly as the model writes it: the Unicode letter,
/// separator and number categories plus the eight punctuation characters a tag key admits.
/// See [`TAG_KEY_MIN_LEN`] for where it is read from.
const TAG_KEY_PATTERN: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$";

/// How a clause about a tag key names the map it was held in: the `Tags` member as a
/// violation path spells it, which is the same for every operation that takes one.
const TAGS_MEMBER: &str = "tags";

/// [`TAG_KEY_MIN_LEN`]/[`TAG_KEY_MAX_LEN`]/[`TAG_KEY_PATTERN`] as the framework's
/// constraint walk reads them.
static TAG_KEY_TRAITS: ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    ::std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::LengthTrait::new(
                Some(TAG_KEY_MIN_LEN),
                Some(TAG_KEY_MAX_LEN),
            ),
        ));
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::PatternTrait::new(TAG_KEY_PATTERN),
        ));
        traits
    });

/// The key leaf of a `Tags` map, as the member it arrives on: a string shape carrying the
/// key shape's own traits, which is what [`violations_of`] runs the walk over.
static TAG_KEY_SCHEMA: ::std::sync::LazyLock<::aws_smithy_schema::Schema> =
    ::std::sync::LazyLock::new(|| {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.lambda#Tags$key",
                "com.amazonaws.lambda",
                TAGS_MEMBER,
            ),
            ::aws_smithy_schema::ShapeType::String,
            TAGS_MEMBER,
            0,
        )
        .with_traits(&TAG_KEY_TRAITS)
    });

/// The constraint violations `value` raises against `schema`, judged by the framework's own
/// constraint walk — the one place this crate drives that walk over a single string.
fn violations_of(
    schema: &::aws_smithy_schema::Schema,
    value: &str,
) -> Vec<::aws_smithy_schema_server::constraints::Violation> {
    use ::aws_smithy_schema::serde::ShapeSerializer as _;

    let mut validator = ::aws_smithy_schema_server::constraints::ValidatingSerializer::new();
    // Infallible: the validator accumulates violations instead of failing.
    let _ = validator.write_string(schema, value);
    validator.into_violations()
}

/// Whether `key` is inside the shape the model declares for a tag key.
#[must_use]
pub fn is_shaped_tag_key(key: &str) -> bool {
    violations_of(&TAG_KEY_SCHEMA, key).is_empty()
}

/// The refusal a request whose `Tags` map carries a key outside its modelled shape gets, or
/// `None` when every key is inside it.
///
/// One clause for the MAP member, quoting the map the offending key was held in and listing
/// the whole set of requirements a key of it has — which is what a clause about a map key
/// states, whatever a key got wrong and however many keys got it wrong
/// (`service_common::validation_message::map_key_refusal_quoting`). The expression is
/// rendered the way every other clause this service states renders one
/// (`service_common::validation::unanchored`), so a requirement about a key reads like a
/// requirement about a reported member.
///
/// The map is quoted the way the walk this decision stands in for quotes one
/// (`service_common::validation::rendered_map`), in KEY order: that walk reads the BUILT
/// input, where a string map keeps no wire order. A rendering past the largest value one
/// clause of this service quotes ([`crate::service::LARGEST_QUOTED_VALUE_BYTES`]) leaves the
/// clause without its value half rather than growing the response with the request.
#[must_use]
pub fn tag_key_shape_refusal<'a, I>(tags: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut entries: Vec<(&str, &str)> = tags.into_iter().collect();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
    if entries.iter().all(|(key, _)| is_shaped_tag_key(key)) {
        return None;
    }
    let rendered = ::service_common::validation::rendered_map(&entries);
    let quoted = (rendered.len() <= crate::service::LARGEST_QUOTED_VALUE_BYTES)
        .then_some(rendered.as_str());
    Some(::service_common::validation_message::map_key_refusal_quoting(
        TAGS_MEMBER,
        quoted,
        TAG_KEY_MIN_LEN,
        TAG_KEY_MAX_LEN,
        Some(&::service_common::validation::unanchored(TAG_KEY_PATTERN)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a tag key is inside its modelled shape when it is 1 to 128 characters of the
    /// Unicode letter, separator and number categories and the eight punctuation characters
    /// the pattern names, and outside it otherwise.
    ///
    /// The rows span what the shape generalises over: each admitted character class, a
    /// character outside every one of them, both `@length` bounds and one past each of them.
    /// None of them is the key the report cited.
    #[test]
    fn a_tag_key_is_judged_by_the_shape_the_model_declares() {
        // (case, the key, whether the shape admits it)
        let rows: &[(&str, String, bool)] = &[
            ("letters", "Owner".to_string(), true),
            ("numbers and letters", "team7".to_string(), true),
            ("a space, which is a separator", "cost centre".to_string(), true),
            (
                "every punctuation character the pattern names",
                "_.:/=+-@".to_string(),
                true,
            ),
            ("a non-ASCII letter", "équipe".to_string(), true),
            ("the length minimum", "a".to_string(), true),
            (
                "the length maximum",
                "k".repeat(usize::try_from(TAG_KEY_MAX_LEN).expect("a positive maximum")),
                true,
            ),
            (
                "one character past the length maximum",
                "k".repeat(usize::try_from(TAG_KEY_MAX_LEN).expect("a positive maximum") + 1),
                false,
            ),
            ("one character under the minimum", String::new(), false),
            ("an exclamation mark", "bad!key".to_string(), false),
            ("a percent sign", "100%".to_string(), false),
            ("a brace", "{owner}".to_string(), false),
            ("a comma", "a,b".to_string(), false),
            ("a newline", "a\nb".to_string(), false),
        ];
        for (case, key, admitted) in rows {
            let shaped = is_shaped_tag_key(key);
            assert_eq!(
                shaped, *admitted,
                "case {case:?}: expected the shape to admit it = {admitted}, got {shaped}"
            );
        }
    }

    /// One row of the refusal table: the case name, the entries the request carried and the
    /// map the clause quotes (`None` for a request that is not refused at all).
    type RefusalRow<'row> = (&'row str, &'row [(&'row str, &'row str)], Option<&'row str>);

    /// RULE: a map carrying any key outside the shape is refused with one clause at the map
    /// member, quoting the map and listing every requirement a key has; a map whose keys are
    /// all inside it is not refused at all.
    #[test]
    fn the_refusal_states_every_requirement_a_tag_key_has() {
        let requirements = "[Member must have length less than or equal to 128, Member must \
                            have length greater than or equal to 1, Member must satisfy \
                            regular expression pattern: \
                            ([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)]";
        let rows: &[RefusalRow<'_>] = &[
            ("no entries at all", &[], None),
            (
                "keys the shape admits",
                &[("Owner", "platform"), ("cost centre", "7")],
                None,
            ),
            (
                "one key outside the shape",
                &[("bad!key", "true")],
                Some("{bad!key=true}"),
            ),
            (
                "one admitted key and one outside the shape, quoted in key order",
                &[("team", "payments"), ("100%", "of it")],
                Some("{100%=of it, team=payments}"),
            ),
            (
                "two keys outside the shape, stated once",
                &[("bad!key", "true"), ("100%", "of it")],
                Some("{100%=of it, bad!key=true}"),
            ),
        ];
        for (case, entries, quoted) in rows {
            let stated = tag_key_shape_refusal(entries.iter().copied());
            let expected = quoted.map(|map| {
                format!(
                    "1 validation error detected: Value '{map}' at 'tags' failed to satisfy \
                     constraint: Map keys must satisfy constraint: {requirements}"
                )
            });
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
