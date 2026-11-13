// SPDX-License-Identifier: CC0-1.0

//! The header fields an Application Load Balancer REMOVES from a request before anything
//! behind it sees them, and the attribute that turns the removal on.
//!
//! `LoadBalancerAttribute$Key` carries the switch:
//! "`routing.http.drop_invalid_header_fields.enabled` - Indicates whether HTTP headers with
//! invalid header fields are removed by the load balancer (true) or routed to targets
//! (false). The default is false." So the removal is the LOAD BALANCER's, it is per load
//! balancer rather than per listener or per target group, and a load balancer carrying the
//! attribute at its documented default removes nothing at all — every header the client sent
//! reaches the target.
//!
//! What makes a field name VALID is the character set Elastic Load Balancing documents for a
//! header name: the alphanumeric characters and the hyphen ([`names_a_valid_field`]). A name
//! carrying anything else — the underscore an application framework spells a variable with,
//! a dot, or any other character HTTP's own grammar admits in a token — is an invalid header
//! field, and is what the attribute drops. The value is not read: the switch is about the
//! NAME, which is what the documented character set is stated for.
//!
//! The removal happens where the load balancer RECEIVES the request
//! ([`super::data_plane::serve`]), so nothing behind it — neither the rules its listener
//! evaluates nor the target the request is forwarded to — is handed a field the load balancer
//! removed, and every valid header of the same request travels on untouched.

use crate::state::LoadBalancerRecord;

/// The attribute that decides whether invalid header fields are removed:
/// `LoadBalancerAttribute$Key`'s `routing.http.drop_invalid_header_fields.enabled`.
const DROP_INVALID_HEADER_FIELDS_ATTRIBUTE: &str = "routing.http.drop_invalid_header_fields.enabled";

/// The one character HTTP's token grammar and a load balancer's header names agree on beside
/// the alphanumerics: the hyphen.
const HEADER_NAME_SEPARATOR: u8 = b'-';

/// Whether `record` removes invalid header fields: what
/// [`DROP_INVALID_HEADER_FIELDS_ATTRIBUTE`] holds for it — a caller's own write, else the
/// value the attribute is carried at — read through the ONE catalog of load balancer
/// attributes, so what `DescribeLoadBalancerAttributes` reports and what the data plane does
/// cannot drift.
///
/// A load balancer of a family that carries no such attribute removes nothing: the attribute
/// is documented "supported by only Application Load Balancers".
pub(super) fn drops_invalid_header_fields(record: &LoadBalancerRecord) -> bool {
    let Some(spec) = crate::state::load_balancer_attribute(DROP_INVALID_HEADER_FIELDS_ATTRIBUTE)
    else {
        return false;
    };
    if !spec.applies_to(record) {
        return false;
    }
    record
        .attributes
        .get(DROP_INVALID_HEADER_FIELDS_ATTRIBUTE)
        .cloned()
        .or_else(|| spec.default_for(record))
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
}

/// Whether `name` is a valid header field name: the alphanumeric characters and the hyphen,
/// which is the character set a header name a load balancer passes on is written in. A name
/// with no characters at all is no field name either.
pub(super) fn names_a_valid_field(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == HEADER_NAME_SEPARATOR)
}

/// Remove every header whose name is no valid field name, keeping the rest in the order they
/// arrived — what the load balancer hands on once the attribute is on.
pub(super) fn dropped(headers: &mut Vec<(String, String)>) {
    headers.retain(|(name, _)| names_a_valid_field(name));
}

#[cfg(test)]
mod tests {
    //! The character set alone. What a client and a target SEE is pinned end to end in
    //! `tests/all/alb_drop_invalid_header_fields.rs`.

    use super::*;

    #[test]
    fn a_field_name_is_valid_only_while_it_is_alphanumerics_and_hyphens() {
        // The rows span the character classes the set is stated over: letters of either
        // case, digits, the hyphen, the three of them together, and one row per class of
        // character HTTP's own token grammar admits that the set does not — the underscore,
        // the dot, and the other token punctuation — beside the name with no characters.
        let cases: &[(&str, bool)] = &[
            ("x-request-id", true),
            ("X-Mixed-Case-42", true),
            ("accept", true),
            ("ABC", true),
            ("123", true),
            ("a-1", true),
            ("x_underscored", false),
            ("x.dotted", false),
            ("x+plus", false),
            ("x~tilde", false),
            ("x|pipe", false),
            ("x^caret", false),
            ("x`backtick", false),
            ("x!bang", false),
            ("x#hash", false),
            ("x$dollar", false),
            ("x%percent", false),
            ("x&amp", false),
            ("x'quote", false),
            ("x*star", false),
            ("x header", false),
            ("", false),
        ];
        for (name, expected) in cases {
            let valid = names_a_valid_field(name);
            assert_eq!(
                valid, *expected,
                "case {name:?}: expected valid {expected}, got {valid}"
            );
        }
    }

    #[test]
    fn only_the_invalid_named_headers_are_dropped_and_the_order_is_kept() {
        let mut headers: Vec<(String, String)> = [
            ("host", "my-alb-1.us-west-2.elb.amazonaws.com"),
            ("x_bad", "dropped"),
            ("x-good", "kept"),
            ("x.also-bad", "dropped"),
            ("content-type", "text/plain"),
        ]
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
        dropped(&mut headers);
        let kept: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            kept,
            vec!["host", "x-good", "content-type"],
            "expected only the valid-named headers, in order, got {kept:?}"
        );
    }
}
