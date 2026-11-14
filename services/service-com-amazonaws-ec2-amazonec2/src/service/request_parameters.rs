// SPDX-License-Identifier: CC0-1.0

//! One reading of an ec2Query request's own parameter list, shared by every gate of this
//! front door that needs it.
//!
//! An ec2Query request carries its parameters as an `application/x-www-form-urlencoded` body
//! (and, for a request that sends them in the URI, a query string), so reading them is one
//! walk over both through `form_urlencoded` — the crate that decodes that media type, rather
//! than a split of the bytes. Three readings of this front door need it and each needs a
//! different thing out of it: the resources a request ADDRESSES ([`super::addressed_resources`]),
//! whether it sets the `DryRun` flag ([`super::dry_run`]), and the parameters the audit leg
//! records ([`super::management_events`]). They share the walk so the body/query handling and
//! the bound on it cannot drift between them, and each passes the bound its own concern takes.

use ::std::borrow::Cow;

use aws_smithy_runtime_api::http::Request as RtRequest;

/// The most parameters a GATE of this front door walks in one request: a bound on the work a
/// request can ask of it — the parse itself and whatever each gate does per parameter — well
/// above the longest list an EC2 request carries (a filter or resource list of a few hundred
/// entries), so no request is read past it.
///
/// The audit leg passes its own, larger bound
/// (`super::management_events::MAX_RECORDED_PARAMETERS`): what it reads bounds a RECORD rather
/// than a decision, so the two are stated where each is taken.
pub const MAX_PARAMETERS_READ: usize = 4_096;

/// The `(name, value)` parameters `request` carries, body first and then its query string, at
/// most `most` of them.
///
/// Borrowed from the request throughout: nothing here copies the list, so a gate that reads one
/// parameter pays for one parameter.
pub fn request_parameters(
    request: &RtRequest,
    most: usize,
) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
    let body = request.body().bytes().unwrap_or_default();
    let query = request
        .uri()
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    ::form_urlencoded::parse(body)
        .chain(::form_urlencoded::parse(query.as_bytes()))
        .take(most)
}
