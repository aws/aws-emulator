// SPDX-License-Identifier: CC0-1.0

//! What every route that serves an ADDRESS this service published has in common: how it
//! reads that address's path, and how it answers one fetch of it.
//!
//! Two planes are addressed this way — the authenticated session a presigned URL opens
//! ([`super::presigned_session`]) and the Studio landing a domain and its spaces publish
//! ([`super::studio_landing`]) — and both are reached the same way: a browser follows a URL
//! this service minted, so the request carries no SigV4 credential and names no modelled
//! operation. What it is answered with is therefore not an API error envelope but a status
//! and a line of text, or the document the address serves.
//!
//! One copy of those two readings, so a change to either (a header, the way a path segment
//! is decoded) reaches every address this service publishes rather than one of them.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

/// The media type a plain-text answer carries: an address this service published is opened
/// by a browser, so a refusal is a line of text rather than an API error document.
pub const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// The media type of a document one of these addresses serves.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// The method such an address is opened on: a browser following the URL.
pub const FETCH_METHOD: &str = "GET";

/// One answer of such a route: the bytes, their media type and their length.
///
/// The headers are set through the NON-panicking setter: `Headers::insert` aborts on a
/// value it cannot represent, and nothing a request can reach may panic. Every value a
/// caller of this passes is this crate's own, so neither can fail; a header that somehow
/// did is dropped rather than taken the process down with.
pub fn answer(status: u16, media_type: &str, body: &str) -> RtResponse {
    let bytes = body.as_bytes().to_vec();
    let length = bytes.len();
    let mut response = RtResponse::new(answered_status(status), SdkBody::from(bytes));
    let _ = response
        .headers_mut()
        .try_insert("content-type", media_type.to_string());
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// A status one of these routes answers with. Every call site passes a constant, so the
/// conversion cannot fail.
fn answered_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("a published address answers constant statuses")
}

/// The status a hand-off to another address of this emulator's answers with: "the requested
/// resource resides temporarily under a different URI" (RFC 9110's `302 Found`), which is
/// what a browser following a sign-in URL is redirected on.
pub const HANDED_OFF: u16 = 302;

/// One hand-off of such a route: the status above and the `Location` the client follows next,
/// with the address also in the body so a client that reports the answer names where it was
/// sent.
///
/// `None` when the address cannot be carried in a header at all — the setter is the
/// NON-panicking one, for the reason [`answer`] gives — because a `302` with no `Location` is
/// a redirect no client can follow: the caller answers what it would have answered without
/// the hand-off instead.
pub fn hand_off(location: &str) -> Option<RtResponse> {
    let mut response = answer(
        HANDED_OFF,
        PLAIN_MEDIA_TYPE,
        &format!("this URL signs you in at {location}\n"),
    );
    response
        .headers_mut()
        .try_insert("location", location.to_string())
        .ok()?;
    Some(response)
}

/// The path segments of `uri`, percent-decoded, empty segments dropped — the same view of a
/// path the mint built the address from, so an identifier that had to be encoded names the
/// same resource here as it does in the record.
pub fn decoded_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(smithy_server_runtime::percent_decode)
                .collect()
        })
        .unwrap_or_default()
}

/// The decoded value of one query field of `uri`, or `None` when it carries none.
pub fn query_field(uri: &str, field: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        name.eq_ignore_ascii_case(field)
            .then(|| smithy_server_runtime::percent_decode(value))
    })
}
