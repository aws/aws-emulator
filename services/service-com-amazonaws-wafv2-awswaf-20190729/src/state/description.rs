// SPDX-License-Identifier: CC0-1.0

//! The `Description` an entity carries, and the refusal a description outside the
//! modelled form draws.
//!
//! `EntityDescription` is the one shape every create and update of this service
//! describes its entity with, and it is constrained: `@length(1..256)` plus a
//! `@pattern` over ASCII word characters and a small punctuation and whitespace set
//! — deliberately narrower than the Unicode-aware classes the same model writes the
//! tag patterns with (`TagKey`'s `@pattern` names `\p{L}\p{Z}\p{N}`). A Smithy
//! `@pattern` is an ECMA-262 regular expression, where `\w`, `\d` and `\s` are the
//! ASCII classes and a character outside ASCII matches none of them, so a
//! description carrying multi-byte characters does not satisfy that pattern and the
//! request is refused as a request-shape error.
//!
//! The request boundary runs every modelled constraint before a handler sees the
//! request, and the regular-expression engine it checks a `@pattern` with reads
//! those classes with UNICODE semantics, so a non-ASCII description satisfies the
//! compiled expression there. The ASCII reading is therefore made here, over the
//! same member, with the framework's own constraint walk
//! ([`validate_members`]) and the framework's own `ValidationException` envelope
//! ([`constraint_envelope`]) — so a description refused here reads exactly like a
//! member refused at the boundary, and the clause names the pattern the MODEL
//! declares rather than the ASCII-pinned spelling the check is run with.

use super::*;

/// `EntityDescription`'s `@pattern`, verbatim
/// (`smithy_query(query="shape",shape="EntityDescription")`): the text a refusal's
/// clause names, since it is the requirement the model states.
pub const ENTITY_DESCRIPTION_PATTERN: &str =
    r"^[\w+=:#@/\-,\.][\w+=:#@/\-,\.\s]+[\w+=:#@/\-,\.]$";

/// [`ENTITY_DESCRIPTION_PATTERN`] with its character classes PINNED to the ASCII
/// ones a Smithy `@pattern` is written in.
///
/// The same expression, with the regex crate's Unicode mode turned off for it
/// (`(?-u)`), which is what makes `\w` and `\s` match the ASCII characters the
/// Smithy dialect's classes name and nothing else. Only the check is run with this
/// spelling; the clause a refusal carries names the model's own pattern.
const ENTITY_DESCRIPTION_ASCII_PATTERN: &str =
    r"(?-u)^[\w+=:#@/\-,\.][\w+=:#@/\-,\.\s]+[\w+=:#@/\-,\.]$";

/// `EntityDescription`: `@length(1..256)` plus
/// [`ENTITY_DESCRIPTION_ASCII_PATTERN`].
static ENTITY_DESCRIPTION_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(256))));
    traits.insert(Box::new(PatternTrait::new(ENTITY_DESCRIPTION_ASCII_PATTERN)));
    traits
});

/// `Description`, as every operation that describes its entity declares it. The
/// walk reads only a member schema's NAME (for the violation path) and its trait
/// map, so one declaration serves every such operation — each of them names the
/// member `Description`.
pub static M_DESCRIPTION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$Description",
        NAMESPACE,
        "Description",
    ),
    ShapeType::String,
    "Description",
    3,
)
.with_traits(&ENTITY_DESCRIPTION_TRAITS);

/// The `ValidationException` message a `Description` outside `EntityDescription`'s
/// modelled form earns, or `None` when the request's description is inside it (and
/// when the request carries none at all, which the member being optional allows).
///
/// The envelope is the framework's own, so the clause, the elision of a long value
/// and the member path read exactly as they do for a member refused at the request
/// boundary; a `@pattern` violation's clause names
/// [`ENTITY_DESCRIPTION_PATTERN`], the requirement the model states, rather than
/// the ASCII-pinned expression the check runs with.
pub fn entity_description_envelope(description: Option<&str>) -> Option<String> {
    let description = description?;
    let violations: Vec<Violation> =
        validate_members(&[(&M_DESCRIPTION, ConstrainedValue::Text(description))])
            .into_iter()
            .map(|violation| match violation.kind {
                ViolationKind::Pattern(_) => Violation {
                    kind: ViolationKind::Pattern(ENTITY_DESCRIPTION_PATTERN.to_string()),
                    ..violation
                },
                _ => violation,
            })
            .collect();
    (!violations.is_empty()).then(|| constraint_envelope(&violations))
}
