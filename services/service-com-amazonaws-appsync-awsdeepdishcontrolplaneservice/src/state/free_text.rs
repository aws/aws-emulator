// SPDX-License-Identifier: CC0-1.0

//! The members of this service whose modelled `@pattern` admits any text at all, and
//! the one bound that is left on them: their `@length`.
//!
//! A member whose pattern is `^.*$` is satisfied by every value it can hold. `.` stands
//! for any character, `^` and `$` are evaluated per LINE, and the expression is matched
//! as a search over the value, so a value carrying newlines satisfies it exactly as a
//! single-line one does — the pattern states no restriction to enforce. Such a member's
//! CONTENT is therefore never refusable, and the only thing a request can get wrong
//! about it is its SIZE.
//!
//! This service has two such shapes
//! (`smithy_query(query="shape", shape="MappingTemplate,Description")`):
//!
//! * `MappingTemplate`, `@length` 1..65536 — the request and response mapping templates
//!   of `CreateResolver`/`UpdateResolver`/`CreateFunction`/`UpdateFunction`, written in
//!   Apache Velocity (VTL), whose `#if`/`#foreach`/`#end` directives and pretty-printed
//!   JSON are line-oriented, so a real template spans several lines;
//! * `Description`, `@length` 0..255 — the description of
//!   `CreateDomainName`/`UpdateDomainName`.
//!
//! Those members are off the request boundary (`skip_validation` in `src/ops/types/`),
//! because the boundary matches a `@pattern` as a whole-string anchored match with `.`
//! stopping at a line terminator and would refuse every multi-line value; the checks
//! here take over the bound the boundary stopped reading, so a value too long (or, for a
//! template, empty) is still refused — as the service's own `BadRequestException`, the
//! only client error these operations model.

/// `MappingTemplate`'s modelled minimum length: a template is at least one character
/// long, so an empty one is no template.
pub const MAPPING_TEMPLATE_MIN_LEN: usize = 1;

/// `MappingTemplate`'s modelled maximum length.
pub const MAPPING_TEMPLATE_MAX_LEN: usize = 65_536;

/// `Description`'s modelled minimum length: the shape declares `min: 0`, so an empty
/// description is a legal value.
pub const DESCRIPTION_MIN_LEN: usize = 0;

/// `Description`'s modelled maximum length.
pub const DESCRIPTION_MAX_LEN: usize = 255;

/// The request member a mapping template arrives on for a request, and the one for a
/// response: the two members every resolver and function create or update takes, named
/// as the model names them, because a refusal names the member the caller sent.
pub const REQUEST_MAPPING_TEMPLATE_MEMBER: &str = "requestMappingTemplate";

/// The response template's member name ([`REQUEST_MAPPING_TEMPLATE_MEMBER`]).
pub const RESPONSE_MAPPING_TEMPLATE_MEMBER: &str = "responseMappingTemplate";

/// The member a domain name's description arrives on.
pub const DESCRIPTION_MEMBER: &str = "description";

/// The refusal text for a value outside `member`'s modelled `@length`: AppSync names the
/// member it refused and the bound it broke, the way the rest of this service's
/// bad-request wording does, so the caller can fix the request from the message alone.
///
/// The length is counted in CHARACTERS, which is what Smithy's `@length` bounds on a
/// string, so a multi-byte template is measured the way the model measures it.
fn length_refusal(member: &str, value: &str, min: usize, max: usize) -> Option<String> {
    let length = value.chars().count();
    if (min..=max).contains(&length) {
        return None;
    }
    Some(format!(
        "Invalid {member} given. Must be between {min} and {max} characters long."
    ))
}

/// How many characters go to a kilobyte of template, which is the unit AppSync states a
/// template's SIZE limit in.
const TEMPLATE_KB: usize = 1_024;

/// The DIRECTION a mapping template is written for, as AppSync names it in a refusal: the
/// member without the `MappingTemplate` it ends in, so `requestMappingTemplate` is stated
/// as `request` and `responseMappingTemplate` as `response` — the two the service words
/// this refusal for, each in its own direction.
fn template_direction(member: &str) -> &str {
    member.strip_suffix("MappingTemplate").unwrap_or(member)
}

/// The text AppSync states a mapping template past its modelled ceiling with: the template's
/// direction, and the ceiling in the unit the service states it in — fixed text whose
/// variable parts are the direction the member names and the bound the model declares.
///
/// A template is refused for its SIZE in its own sentence rather than by the member-and-range
/// wording every other value outside a `@length` takes ([`length_refusal`]), because a
/// template past the ceiling is the one thing the caller is told to make SMALLER rather than
/// to correct.
fn template_size_message(member: &str) -> String {
    format!(
        "Invalid {} template given, must be less than {}kb in size.",
        template_direction(member),
        MAPPING_TEMPLATE_MAX_LEN / TEMPLATE_KB
    )
}

/// The refusal text for a mapping template outside `MappingTemplate`'s modelled
/// `@length`, or `None` for one inside it — including a member the request omits, which
/// sets no template at all.
///
/// The two bounds are worded differently, as the service words them: a template past the
/// ceiling takes the size sentence ([`template_size_message`]), and one below the minimum —
/// i.e. an empty template, which is no template at all rather than an over-large one — takes
/// the member-and-range wording.
pub fn mapping_template_refusal(member: &str, template: Option<&str>) -> Option<String> {
    let template = template?;
    if template.chars().count() > MAPPING_TEMPLATE_MAX_LEN {
        return Some(template_size_message(member));
    }
    length_refusal(
        member,
        template,
        MAPPING_TEMPLATE_MIN_LEN,
        MAPPING_TEMPLATE_MAX_LEN,
    )
}

/// The refusal text for the pair of mapping templates one create or update carries,
/// taken in the order the model declares them, or `None` when both are inside the
/// modelled bound.
///
/// One call states ONE refusal, the first member the request got wrong, the way every
/// other bad request of this service is stated.
pub fn mapping_templates_refusal(
    request_mapping_template: Option<&str>,
    response_mapping_template: Option<&str>,
) -> Option<String> {
    mapping_template_refusal(REQUEST_MAPPING_TEMPLATE_MEMBER, request_mapping_template).or_else(
        || {
            mapping_template_refusal(
                RESPONSE_MAPPING_TEMPLATE_MEMBER,
                response_mapping_template,
            )
        },
    )
}

/// The refusal text for a description outside `Description`'s modelled `@length`, or
/// `None` for one inside it (an absent member included).
pub fn description_refusal(description: Option<&str>) -> Option<String> {
    length_refusal(
        DESCRIPTION_MEMBER,
        description?,
        DESCRIPTION_MIN_LEN,
        DESCRIPTION_MAX_LEN,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template of exactly `length` characters, built from the lines a real VTL
    /// template carries so the case exercises newlines as well as size.
    fn template(length: usize) -> String {
        let mut text = String::new();
        while text.chars().count() < length {
            text.push_str("#if($ctx.args.id)\n$util.toJson($ctx.result)\n#end\n");
        }
        text.chars().take(length).collect()
    }

    #[test]
    fn a_template_inside_the_modelled_length_is_not_refused() {
        for length in [
            MAPPING_TEMPLATE_MIN_LEN,
            64,
            MAPPING_TEMPLATE_MAX_LEN - 1,
            MAPPING_TEMPLATE_MAX_LEN,
        ] {
            let value = template(length);
            let refusal = mapping_templates_refusal(Some(&value), Some(&value));
            assert!(
                refusal.is_none(),
                "case a {length}-character template: expected no refusal, got {refusal:?}"
            );
        }
    }

    #[test]
    fn a_template_outside_the_modelled_length_is_refused_by_member() {
        let too_long = template(MAPPING_TEMPLATE_MAX_LEN + 1);
        // An EMPTY template is no template at all: it takes the member-and-range wording.
        let cases: &[(&str, Option<&str>, Option<&str>, &str)] = &[
            ("an empty request template", Some(""), None, "request"),
            ("an empty response template", None, Some(""), "response"),
        ];
        for (case, request, response, member) in cases {
            let refusal = mapping_templates_refusal(*request, *response);
            let expected = format!(
                "Invalid {member}MappingTemplate given. Must be between \
                 {MAPPING_TEMPLATE_MIN_LEN} and {MAPPING_TEMPLATE_MAX_LEN} characters long."
            );
            assert_eq!(
                refusal.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }

        // A template PAST the ceiling takes the size sentence, in its own direction.
        let cases: &[(&str, Option<&str>, Option<&str>, &str)] = &[
            (
                "an over-long request template",
                Some(too_long.as_str()),
                None,
                "request",
            ),
            (
                "an over-long response template",
                None,
                Some(too_long.as_str()),
                "response",
            ),
            (
                "both templates over-long names the request member first",
                Some(too_long.as_str()),
                Some(too_long.as_str()),
                "request",
            ),
        ];
        for (case, request, response, direction) in cases {
            let refusal = mapping_templates_refusal(*request, *response);
            let expected = format!(
                "Invalid {direction} template given, must be less than {}kb in size.",
                MAPPING_TEMPLATE_MAX_LEN / TEMPLATE_KB
            );
            assert_eq!(
                refusal.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }

    #[test]
    fn a_description_is_refused_only_past_the_modelled_maximum() {
        for length in [DESCRIPTION_MIN_LEN, 1, DESCRIPTION_MAX_LEN] {
            let value = "d".repeat(length);
            let refusal = description_refusal(Some(&value));
            assert!(
                refusal.is_none(),
                "case a {length}-character description: expected no refusal, got {refusal:?}"
            );
        }
        let value = "d".repeat(DESCRIPTION_MAX_LEN + 1);
        let refusal = description_refusal(Some(&value));
        let expected = format!(
            "Invalid {DESCRIPTION_MEMBER} given. Must be between {DESCRIPTION_MIN_LEN} and \
             {DESCRIPTION_MAX_LEN} characters long."
        );
        assert_eq!(
            refusal.as_deref(),
            Some(expected.as_str()),
            "case a description one character past the maximum: expected {expected:?}, got \
             {refusal:?}"
        );
    }
}
