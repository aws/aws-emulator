// SPDX-License-Identifier: CC0-1.0

//! What this service's NON-RPC surfaces share: how a request's path and query are
//! read, how a message echoes a value the caller chose, and how a modelled error of
//! this service is rendered outside the generated operations.
//!
//! HealthLake serves surfaces that are not awsJson1_0 RPC calls — a data store's
//! FHIR REST data plane ([`super::fhir_rest`]) and, on the data-transformation
//! endpoint, the `/validate-source` check ([`super::validate_source`]) and the REST
//! tagging surface ([`super::rest_tagging`]) — and each is addressed by HTTP method
//! and path rather than by `X-Amz-Target`. The readers below are what
//! [`super::EmulatedService::override_operation`] routes them with, and what their
//! handlers then read their own inputs with, so the routing decision and the handler
//! cannot read a request differently.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

/// The most characters of a caller-supplied value a message from these surfaces
/// echoes, so a request cannot amplify itself into an arbitrarily long response.
///
/// It is the ceiling the framework's own constraint messages use when they echo a
/// refused value (`MAX_ECHOED_VALUE` in
/// `vendor/smithy-schema-server/src/constraints.rs`), so an elided message here reads
/// like an elided message anywhere else in the emulator.
const MAX_ECHOED: usize = 256;

/// Whether `path` addresses the data-transformation endpoint's `/validate-source`
/// check.
///
/// The path is matched on its segments rather than as a string, so the leading and
/// trailing `/` a client may or may not send do not change the answer. The
/// `datatransformation.` host prefix the client injects addresses the ENDPOINT and
/// leaves the path alone, so nothing here reads the host.
pub(crate) fn addresses_validate_source(path: &str) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    segments.next() == Some(super::validate_source::VALIDATE_SOURCE_PATH_SEGMENT)
        && segments.next().is_none()
}

/// The path component of a request URI, with any scheme, authority and query removed.
///
/// Several service crates carry this same private reader (`grep -rln 'fn request_path'
/// services/*/src`); nothing shared exports one yet, so this is a copy of that rule
/// rather than a new shared surface, and like the others it is crate-private.
pub(crate) fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan
    // over the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The value of query parameter `name` in `uri`, percent-decoded, or `None` when the
/// URI carries no such parameter. The decoding itself is the framework's
/// ([`smithy_server_runtime::percent_decode`]), never hand-rolled here.
pub(crate) fn query_value(uri: &str, name: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_path, query)| query)?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| smithy_server_runtime::percent_decode(value))
    })
}

/// Every query parameter of `uri` in the order it was sent, each name and value
/// percent-decoded through the framework's decoder, as [`query_value`] decodes the one it
/// is asked for.
///
/// A surface addressed by its whole query rather than by one named parameter needs them
/// all: a FHIR search is a CONJUNCTION of the parameters it carries, and a parameter it
/// does not recognise changes what the answer means, so it cannot be left unread. A pair
/// with no `=` carries no value and is reported with an empty one, which is what a
/// reader of that parameter then judges.
pub(crate) fn query_pairs(uri: &str) -> Vec<(String, String)> {
    let Some((_path, query)) = uri.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (
                smithy_server_runtime::percent_decode(name),
                smithy_server_runtime::percent_decode(value),
            ),
            None => (smithy_server_runtime::percent_decode(pair), String::new()),
        })
        .collect()
}

/// `value` as a message echoes it: a bounded, single-line prefix, so neither a long
/// request value nor a parser's multi-line complaint can run away with the response.
pub(crate) fn bounded(value: &str) -> String {
    let flattened: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flattened.trim();
    match trimmed.char_indices().nth(MAX_ECHOED) {
        Some((end, _)) => format!("{}…", &trimmed[..end]),
        None => trimmed.to_string(),
    }
}

/// The media type this service's awsJson1_0 envelope travels as, which the modelled
/// errors below answer in.
const AWS_JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.0";

/// One modelled error of this service, as its awsJson1_0 envelope renders one: the
/// shape NAME in `__type` — the spelling every other error of this crate carries,
/// from the framework's own renderer — and the shape's own `Message` member.
///
/// The non-RPC surfaces have no generated module to carry their modelled error
/// shapes, so this is where they are written; it is `pub(crate)` and shared so a
/// refusal from one of them is indistinguishable, in shape, from a refusal of any
/// other HealthLake operation.
pub(crate) fn modelled_error(status: u16, shape: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({
        "__type": shape,
        "Message": message,
    });
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of a caller of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", AWS_JSON_CONTENT_TYPE);
    response
}

#[cfg(test)]
mod tests {
    //! The two path rules the non-RPC surfaces are routed on, over the shapes a client
    //! sends them in, and the bound every echoed value keeps.

    use super::*;

    /// A data store FHIR endpoint path is recognised whether or not the client's join
    /// doubled the separator, and a path outside that space is not.
    #[test]
    fn a_datastore_fhir_endpoint_path_is_recognised_however_it_was_joined() {
        // (case, request path, the DatastoreId it names)
        let rows: [(&str, &str, Option<&str>); 9] = [
            (
                "a type-level interaction",
                "/datastore/abc/r4/Patient",
                Some("abc"),
            ),
            (
                "an endpoint already ending in a separator",
                "/datastore/abc/r4//Patient",
                Some("abc"),
            ),
            (
                "an instance-level interaction",
                "/datastore/abc/r4/Observation/1-2.3",
                Some("abc"),
            ),
            ("the endpoint itself", "/datastore/abc/r4/", Some("abc")),
            ("an absolute-form request URI", "https://healthlake.us-east-1.amazonaws.com/datastore/abc/r4/Patient", None),
            ("the control plane's root path", "/", None),
            ("another release version", "/datastore/abc/r5/Patient", None),
            ("no release-version segment", "/datastore/abc", None),
            ("another path space", "/validate-source", None),
        ];

        for (case, path, expected) in rows {
            // An absolute-form URI is reduced to its path first, exactly as the routing
            // seam does, so that row proves the two readers compose.
            let actual = crate::state::parse_datastore_fhir_path(path);
            let named = actual.as_ref().map(|route| route.datastore_id.as_str());
            assert_eq!(
                named, expected,
                "case {case:?} ({path}): expected the data store {expected:?}, got {actual:?}"
            );
            if expected.is_none() && path.contains("://") {
                let reduced = crate::state::parse_datastore_fhir_path(request_path(path));
                assert_eq!(
                    reduced.as_ref().map(|route| route.datastore_id.as_str()),
                    Some("abc"),
                    "case {case:?}: reduced to its path it names the data store, got {reduced:?}"
                );
            }
        }
    }

    /// `/validate-source` is addressed by that one segment, and nothing deeper is.
    #[test]
    fn only_the_validate_source_segment_addresses_validate_source() {
        // (case, request path, addresses /validate-source)
        let rows: [(&str, &str, bool); 6] = [
            ("the path as sent", "/validate-source", true),
            ("with a trailing separator", "/validate-source/", true),
            ("with no leading separator", "validate-source", true),
            ("a deeper path", "/validate-source/CCDA", false),
            ("another path", "/data-transformation-job", false),
            ("the root path", "/", false),
        ];

        for (case, path, expected) in rows {
            let actual = addresses_validate_source(path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({path}): expected addresses_validate_source == {expected}, \
                 got {actual}"
            );
        }
    }

    /// An echoed value is one line, bounded, and otherwise itself.
    #[test]
    fn an_echoed_value_is_bounded_and_single_line() {
        // (case, value, what a message carries)
        let rows: Vec<(&str, String, String)> = vec![
            ("a short value", "Patient".to_string(), "Patient".to_string()),
            (
                "a value carrying newlines and tabs",
                "line one\n\tline two".to_string(),
                "line one  line two".to_string(),
            ),
            (
                "a value at the bound",
                "x".repeat(MAX_ECHOED),
                "x".repeat(MAX_ECHOED),
            ),
            (
                "a value one past the bound",
                "x".repeat(MAX_ECHOED + 1),
                format!("{}…", "x".repeat(MAX_ECHOED)),
            ),
            (
                "a value far past the bound",
                "x".repeat(MAX_ECHOED * 4),
                format!("{}…", "x".repeat(MAX_ECHOED)),
            ),
            (
                // Elision lands on a character boundary, never mid-UTF-8.
                "a multi-byte value past the bound",
                "é".repeat(MAX_ECHOED + 4),
                format!("{}…", "é".repeat(MAX_ECHOED)),
            ),
        ];

        for (case, value, expected) in rows {
            let actual = bounded(&value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the message to carry {expected:?}, got {actual:?}"
            );
        }
    }
}
