// SPDX-License-Identifier: CC0-1.0

//! The ORIGINS a CORS configuration may allow: the form a `Cors$AllowOrigins` entry has to
//! be written in, and the sentence API Gateway refuses one that is not with.
//!
//! A module of its own under [`crate::state`], beside the document's own rules
//! ([`crate::state::cors`], which applies this one): an entry of that list is an ORIGIN —
//! the value the browser sends in `Origin` and the value the gateway answers in
//! `Access-Control-Allow-Origin`, which RFC 6454 §6.1 serializes as
//! `scheme "://" host [ ":" port ]` — so the grammar is a reading of that serialization and
//! nothing about any one operation.
//!
//! The one other value the member admits is the wildcard [`crate::state::CORS_WILDCARD`],
//! "the special value `*` which allows all origins". A value of any other form names no
//! origin the data plane could ever match a request's `Origin` against, so the configuration
//! is refused instead of being stored on an api that then reports a CORS policy it cannot
//! apply.
//!
//! The host is NOT held to the DNS name grammar: a caller may write a host carrying the
//! wildcard character (`https://*.example.com`), and this service accepts it — which is what
//! the api-update suite's CORS table pins. What is refused is a value that is not an origin
//! SERIALIZATION at all: one with no scheme, or one carrying a path, a query or a fragment.

/// `Ok(())`, or the sentence a configuration naming a value that is not an origin is refused
/// with — the message for the operation's `BadRequestException`.
///
/// The FIRST offending entry is the one reported, in the order the request lists them, so
/// the answer does not depend on how many of them are malformed; a request that fixes it is
/// checked again.
pub fn validate_cors_origins<'a, I>(origins: I) -> Result<(), String>
where
    I: IntoIterator<Item = &'a str>,
{
    for origin in origins {
        if !is_origin(origin) {
            return Err(invalid_origin_message(origin));
        }
    }
    Ok(())
}

/// How API Gateway words the refusal of one malformed origin: the fixed text `Invalid format
/// for origin ` and the value the REQUEST sent for it, which is the only thing the caller can
/// act on.
pub fn invalid_origin_message(origin: &str) -> String {
    format!("Invalid format for origin {origin}")
}

/// Whether `value` is a value `Cors$AllowOrigins` may carry: the wildcard, or an origin
/// serialized as RFC 6454 §6.1 writes one (`scheme://host[:port]`, with nothing after the
/// authority).
///
/// The value is a URL a caller sent, so it is READ by the maintained `url` parser rather
/// than split here: what this adds is the ORIGIN rule on top of the parse — a host to answer
/// the header with, and nothing beyond the authority, since an origin has no userinfo, no
/// path, no query and no fragment. A parse that normalises an empty path to the root leaves
/// the two spellings of the same origin indistinguishable, so both are accepted.
fn is_origin(value: &str) -> bool {
    if value == crate::state::CORS_WILDCARD {
        return true;
    }
    let Ok(parsed) = ::url::Url::parse(value) else {
        return false;
    };
    parsed
        .host_str()
        .is_some_and(|host| !host.is_empty())
        && (parsed.path().is_empty() || parsed.path() == "/")
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_is_the_wildcard_or_a_scheme_and_authority() {
        // The rows span what the grammar generalises over: the wildcard, each part of an
        // origin serialization (scheme, host, port), a host carrying the wildcard
        // character, an IPv6 literal, and the ways a value stops being an origin (no
        // scheme at all, no host, a path, a query, a fragment, a non-numeric port).
        let rows: &[(&str, bool)] = &[
            ("*", true),
            ("https://example.com", true),
            ("http://example.com", true),
            ("http://localhost:3000", true),
            ("https://*.example.com", true),
            ("https://example.com:443", true),
            ("http://[::1]", true),
            ("http://[::1]:8080", true),
            ("ws://example.com", true),
            // The parse normalises an empty path to the root, so an origin written with the
            // trailing separator and one written without it are the same value.
            ("https://example.com/", true),
            ("not-a-url", false),
            ("example.com", false),
            ("https://", false),
            ("https://example.com/path", false),
            ("https://example.com?query", false),
            ("https://example.com#fragment", false),
            ("https://example.com:port", false),
            ("https://user@example.com", false),
            ("https://exa mple.com", false),
            ("://example.com", false),
            ("1https://example.com", false),
            ("", false),
            ("**", false),
        ];
        for (value, expected) in rows {
            assert_eq!(
                is_origin(value),
                *expected,
                "case {value:?}: expected an origin={expected}, got {}",
                is_origin(value)
            );
        }
    }

    #[test]
    fn the_first_malformed_origin_is_the_one_the_refusal_names() {
        let validated = validate_cors_origins(["https://ok.example.com", "nope", "also-nope"]);
        assert_eq!(
            validated,
            Err("Invalid format for origin nope".to_string()),
            "expected the first malformed entry to be named, got {validated:?}"
        );
        assert!(
            validate_cors_origins(["*", "https://ok.example.com"]).is_ok(),
            "a list of origins must be accepted"
        );
    }
}
