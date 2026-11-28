// SPDX-License-Identifier: CC0-1.0

//! WHERE a synthesis task's output is ANNOUNCED: the address this emulator serves that
//! object at.
//!
//! # The rule this module implements
//!
//! `SynthesisTask$OutputUri` is the "Pathway for the output speech file" — an address the
//! CALLER follows to read the object a completed task left in the bucket the start call
//! named. So it has to lead back to the object storage of THIS emulator, which is reached
//! at the endpoint the request itself reached: a URI naming a host under AWS's own zones
//! sends the client out of the emulated environment, where the bucket does not exist, or
//! — with no egress — nowhere at all.
//!
//! Beneath that address the object is addressed PATH-STYLE, `/<bucket>/<key>`, which is
//! the form the one endpoint this emulator answers on serves: a bucket's name is a label
//! of a published AWS hostname rather than of this emulator's own host.
//!
//! The reading of WHICH address that is comes from
//! `service_common::data_plane_addressing`, in the order every service announcing an
//! address at the front door takes it, so this service cannot drift from the others:
//!
//! * the EMULATOR'S OWN endpoint the request addressed
//!   ([`reached_emulator_endpoint_of_parts`]) — a client configured with one local
//!   endpoint URL resolves nothing else, so it is handed back the very address it is
//!   already talking to, port included, and a request a front end re-addressed to this
//!   process carries the address the client dialled in what that front end states about it,
//!   which is the only statement of it that survives the hop;
//! * else, for a caller that resolves AWS's own hostnames, the address this process is
//!   SERVED at ([`served_endpoint_authority`]), the one address a client on this machine
//!   can reach it at — which is the address announced to the caller whose request names a
//!   host of AWS's own, in-process callers of this service included;
//! * else the base [`::smithy_server_runtime::OperationContext::endpoint`] reports for an
//!   emulator served on no port at all — a host nothing resolves, which is the truth
//!   about such an emulator rather than a name of AWS's.
//!
//! Nothing here resolves a name or opens a connection: it reads request parts and returns
//! a string.

use ::service_common::data_plane_addressing::{
    reached_emulator_endpoint_of_parts, served_endpoint_authority, url_authority,
};
use ::smithy_server_runtime::OperationContext;

/// The address the output object `key` of `bucket` is announced at, for the request `ctx`
/// is answering: the object addressed path-style under [`announced_base`].
///
/// The key is taken as the writer built it, so the location a task reports and the object
/// its completion writes cannot drift apart.
#[must_use]
pub fn announced_output_uri(ctx: &OperationContext<'_>, bucket: &str, key: &str) -> String {
    announced_object_uri(
        &announced_base(ctx.parts(), ctx.endpoint().as_str()),
        bucket,
        key,
    )
}

/// The object `key` of `bucket` addressed PATH-STYLE under the announced base `base`, which
/// is the one form the endpoint this emulator answers on serves.
fn announced_object_uri(base: &str, bucket: &str, key: &str) -> String {
    format!("{base}/{bucket}/{key}")
}

/// The base URL — `scheme://host[:port]`, no path — an announced location is rooted at:
/// the endpoint serving THIS request, in the order this module's header states.
///
/// Taken over the two things it reads — the request's own parts and the base this process
/// is served at — so the reading is exercised without a served socket.
fn announced_base(parts: &::http::request::Parts, served_base: &str) -> String {
    // Without the trailing slash, which would double the separator in front of the path.
    let base = served_base.trim_end_matches('/');
    // The base's own authority, read through the shared reading rather than a second split
    // of a URL here; the scheme is whatever precedes it. `EndpointUrl` admits only an
    // absolute `https` URL, so the default stands in for nothing the core reports.
    let served = url_authority(base);
    let scheme = match base.strip_suffix(served) {
        Some(scheme) if !scheme.is_empty() => scheme,
        _ => "https://",
    };
    let authority = reached_emulator_endpoint_of_parts(parts)
        .or_else(|| served_endpoint_authority(base))
        .unwrap_or_else(|| served.to_string());
    format!("{scheme}{authority}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the addressing table: what the row is, the base this process is served
    /// at, the `Host` the request carries (or none, which leaves this service's own
    /// published host in the request's absolute URI), what a front end states about the
    /// address the client dialled it at, and the base an announced location must then be
    /// rooted at.
    type Row = (
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static [(&'static str, &'static str)],
        &'static str,
    );

    /// A request addressed to one of this service's published hosts, carrying `host` where
    /// the row names one and whatever the row's front end states.
    fn request(host: Option<&str>, stated: &[(&str, &str)]) -> ::http::request::Parts {
        let mut builder = ::http::Request::builder()
            .uri("https://polly.us-east-1.amazonaws.com/v1/synthesisTasks");
        if let Some(host) = host {
            builder = builder.header(::http::header::HOST, host);
        }
        for (name, value) in stated {
            builder = builder.header(*name, *value);
        }
        builder
            .body(())
            .expect("the row is a well-formed request")
            .into_parts()
            .0
    }

    /// RULE: an announced output location is rooted at the EMULATOR'S OWN endpoint the
    /// answering request arrived on, at the address this process is SERVED at for a caller
    /// that resolves AWS's own hostnames, and at the unserved base for an emulator served
    /// on no port at all — never at a host AWS publishes — with the object addressed
    /// path-style beneath it.
    #[test]
    fn an_announced_location_is_rooted_at_an_address_this_emulator_is_reached_at() {
        /// The base a served row's process publishes for itself: the data port's own URL.
        const SERVED: &str = "https://127.0.0.1:46365";
        /// What a front end re-addressing a request to that socket writes as the `Host`.
        const UPSTREAM: &str = "127.0.0.1:46365";

        // The rows differ along what the reading generalises over: what the request states
        // about where it arrived (nothing, a loopback literal, a loopback name on another
        // port, a bracketed IPv6 literal, one of AWS's own endpoint hosts, and what a front
        // end states ahead of all of those) crossed with the base this process is served at
        // (a loopback literal with its port, a loopback name, a base written with a trailing
        // separator, and the base of a process served on no port at all).
        let rows: &[Row] = &[
            (
                "a caller that resolves this service's own published host",
                SERVED,
                None,
                &[],
                SERVED,
            ),
            (
                "a caller that addressed the object store's own published host",
                SERVED,
                Some("s3.us-east-1.amazonaws.com"),
                &[],
                SERVED,
            ),
            (
                "the emulator's own endpoint, as the client dialled it",
                SERVED,
                Some(UPSTREAM),
                &[],
                SERVED,
            ),
            (
                "another spelling of the same socket keeps the spelling dialled",
                SERVED,
                Some("localhost:46365"),
                &[],
                "https://localhost:46365",
            ),
            (
                "a bracketed IPv6 literal",
                "https://[::1]:8443",
                Some("[::1]:8443"),
                &[],
                "https://[::1]:8443",
            ),
            (
                "a port this process is not served on, which reached it all the same",
                SERVED,
                Some("127.0.0.1:37623"),
                &[],
                "https://127.0.0.1:37623",
            ),
            (
                "a request a front end re-addressed to this process",
                SERVED,
                Some(UPSTREAM),
                &[("x-forwarded-host", "127.0.0.1:37623")],
                "https://127.0.0.1:37623",
            ),
            (
                "a front end that states the port separately",
                SERVED,
                Some(UPSTREAM),
                &[
                    ("x-forwarded-host", "localhost"),
                    ("x-forwarded-port", "37623"),
                ],
                "https://localhost:37623",
            ),
            (
                "a front end pointed at one of AWS's own hosts",
                SERVED,
                Some(UPSTREAM),
                &[("x-forwarded-host", "s3.us-east-1.amazonaws.com")],
                SERVED,
            ),
            (
                "a served base naming the loopback zone",
                "https://localhost:9297",
                Some("s3.eu-west-1.amazonaws.com"),
                &[],
                "https://localhost:9297",
            ),
            (
                "a served base with a trailing separator",
                "https://127.0.0.1:9297/",
                None,
                &[],
                "https://127.0.0.1:9297",
            ),
            (
                "served on no port at all",
                ::smithy_server_runtime::UNSERVED_URL_BASE,
                Some("s3.us-east-1.amazonaws.com"),
                &[],
                ::smithy_server_runtime::UNSERVED_URL_BASE,
            ),
        ];
        for (case, served_base, host, stated, root) in rows {
            let announced = announced_base(&request(*host, stated), served_base);
            assert_eq!(
                announced.as_str(),
                *root,
                "case {case:?} (served at {served_base:?}, carrying {host:?} and {stated:?}): \
                 expected {root:?}, got {announced:?}"
            );
            // ... and the object sits path-style beneath it, with ONE separator in front of
            // the bucket whatever the base was written with. The expected URIs are written
            // out, not composed, so the row states the whole announced value.
            for (bucket, key, beneath) in [
                ("polly-output", "c0ffee.mp3", "/polly-output/c0ffee.mp3"),
                (
                    "polly-output-eu",
                    "out/2026/c0ffee.pcm",
                    "/polly-output-eu/out/2026/c0ffee.pcm",
                ),
            ] {
                let uri = announced_object_uri(&announced, bucket, key);
                let expected = format!("{root}{beneath}");
                assert_eq!(
                    uri, expected,
                    "case {case:?}: the object {key:?} of {bucket:?} must be addressed as \
                     {expected:?}, got {uri:?}"
                );
            }
        }
    }
}
