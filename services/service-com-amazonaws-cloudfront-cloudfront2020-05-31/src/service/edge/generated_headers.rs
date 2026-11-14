// SPDX-License-Identifier: CC0-1.0

//! The request headers the EDGE itself generates, and what a viewer that states one of them
//! gets instead.
//!
//! CloudFront adds headers of its own to an origin request — the `CloudFront-*` family —
//! and they describe the VIEWER'S CONNECTION rather than anything the viewer asked for:
//! where the viewer is (`CloudFront-Viewer-Country`), which scheme it dialled
//! (`CloudFront-Forwarded-Proto`) and the address it came from
//! (`CloudFront-Viewer-Address`). Two consequences, and this module owns both:
//!
//! * the edge GENERATES them for every viewer request, including one that sent none of them,
//!   so an origin that reads them reads a value for every request it serves;
//! * a value the VIEWER states for one of them is DROPPED, never relayed: the viewer does
//!   not decide what the connection it opened looks like, so a spoofed `CloudFront-*` header
//!   cannot reach the origin ([`is_generated`] is what the origin-request builder skips on).
//!
//! A generated header reaches the origin only where the behaviour's origin request policy
//! NAMES it ([`super::forwarding::ForwardedParameters::generated_headers`]): a behaviour with
//! no origin request policy, or one whose policy does not list the header, sees none of
//! them.
//!
//! ## Where the values come from
//!
//! The COUNTRY is the viewer's location. CloudFront resolves it from the viewer's address,
//! through the MaxMind GeoIP databases `GeoRestriction` names; nothing of that is reachable
//! here, because every viewer of this emulator dials a loopback address, which is in no
//! country any database places, and no database is consulted off this machine. So the
//! country is read from the one header CloudFront itself states it in when the viewer states
//! a country code there — the addressing choice this plane makes for everything a viewer of
//! a loopback emulator cannot otherwise express — and is the edge's own
//! [`EDGE_COUNTRY`] otherwise. A value that is not a code ISO 3166-1 ASSIGNS to a country is
//! not a country, so it is not read as one either ([`is_country_code`]).
//!
//! The SCHEME is the one the viewer dialled: the request's own URI carries it when the
//! address the viewer reached this emulator at stated it (a distribution's edge listener
//! answers both `http://` and `https://` on one address and states which it served —
//! `crate::state::edge_endpoint`), and `https` is what every other address of this emulator
//! is reached over.
//!
//! The ADDRESS is the one the request came from, by the ONE reading of it this emulator has
//! ([`viewer_address`]): a request served in process crossed no socket from a remote peer, so
//! the address is the one its forwarded-for header states and the loopback address it really
//! arrived from otherwise. It is the same reading the real-time log record reports as `c-ip`
//! and the one a web ACL's address rules are evaluated against, so one viewer has one address
//! whichever part of the edge asks — which is what makes a signed URL's source-address
//! condition ([`super::signed_access`]) and the header an origin reads the same value.

use aws_smithy_runtime_api::http::Request as RtRequest;

/// The prefix of every header name CloudFront generates itself.
///
/// The whole `CloudFront-*` family: whatever the edge does or does not generate, a header
/// under this prefix describes the viewer's connection, so a value a VIEWER states for one is
/// not the edge's and never travels to an origin.
const GENERATED_PREFIX: &str = "cloudfront-";

/// `CloudFront-Viewer-Country`: the viewer's country, as "a two-letter, uppercase country
/// code" — the same `ISO 3166-1 alpha-2` spelling a `GeoRestriction` `Location` carries.
pub const VIEWER_COUNTRY: &str = "cloudfront-viewer-country";

/// `CloudFront-Forwarded-Proto`: the scheme the viewer dialled the edge over.
pub const FORWARDED_PROTO: &str = "cloudfront-forwarded-proto";

/// `CloudFront-Viewer-Address`: the address the viewer's request came from.
pub const VIEWER_ADDRESS: &str = "cloudfront-viewer-address";

/// The country the edge reports for a viewer whose location it is told nothing about.
///
/// A real `ISO 3166-1 alpha-2` code, as every value of this header is: the code of the
/// country the Region CloudFront's own endpoint is served from
/// (`service_common::management_events::PARTITION_LEADER_REGION`, `us-east-1`) is in — the
/// United States. Every viewer of this emulator dials a loopback address, so this is the one
/// location there is to report, and it is the same for every request, which is what an
/// origin comparing two requests' countries reads.
const EDGE_COUNTRY: &str = "US";

/// The scheme a viewer that reached an address of this emulator dialled, unless its request
/// states otherwise: every endpoint this emulator publishes is served over TLS.
pub const DEFAULT_SCHEME: &str = "https";

/// The scheme a viewer that dialled the plaintext half of an address used.
pub const PLAINTEXT_SCHEME: &str = "http";

/// The header a viewer that traversed a proxy states its own address in, and the address a
/// request that states none came from.
///
/// WAFv2's own reading of the same question for the same requests
/// (`service_com_amazonaws_wafv2_awswaf_20190729::service::edge`), so a rule that inspects the
/// viewer's address and the header an origin reads it from cannot disagree about one request.
const FORWARDED_FOR_HEADER: &str =
    ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge::FORWARDED_FOR_HEADER;
const IN_PROCESS_CLIENT_IP: &str =
    ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge::IN_PROCESS_CLIENT_IP;

/// Whether `name` is a header name CloudFront generates itself, rather than one a viewer
/// states.
///
/// A header name is case-insensitive (RFC 9110), so the prefix is matched without regard to
/// case: the drop this module documents holds for `CloudFront-Viewer-Country` exactly as it
/// does for `cloudfront-viewer-country`.
pub fn is_generated(name: &str) -> bool {
    name.get(..GENERATED_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(GENERATED_PREFIX))
}

/// The `CloudFront-*` headers the edge generates for one viewer request.
#[derive(Debug, PartialEq, Eq)]
pub struct Generated {
    /// `CloudFront-Viewer-Country`.
    pub country: String,
    /// `CloudFront-Forwarded-Proto`.
    pub scheme: &'static str,
    /// `CloudFront-Viewer-Address`.
    pub address: String,
}

impl Generated {
    /// What the edge generates for `req`.
    pub fn of_request(req: &RtRequest) -> Self {
        Self {
            country: viewer_country(req),
            scheme: viewer_scheme(req.uri()),
            address: viewer_address(req).to_string(),
        }
    }

    /// The generated headers as `(name, value)` pairs, lowercased as an origin request
    /// carries them.
    pub fn headers(&self) -> [(String, String); 3] {
        [
            (VIEWER_COUNTRY.to_string(), self.country.clone()),
            (FORWARDED_PROTO.to_string(), self.scheme.to_string()),
            (VIEWER_ADDRESS.to_string(), self.address.clone()),
        ]
    }
}

/// The address one viewer request came from — the ONE value this request's address has, read
/// by the signed-URL source-address condition, by the web ACL's address rules and by the
/// header an origin receives alike.
///
/// The first address the forwarded-for header states, when it states one that parses as an
/// address, and the loopback address the request really arrived from otherwise: a request this
/// plane serves is dispatched in process, so no remote peer address exists to report and none
/// is invented.
pub fn viewer_address(req: &RtRequest) -> &str {
    req.headers()
        .get(FORWARDED_FOR_HEADER)
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|address| address.parse::<::std::net::IpAddr>().is_ok())
        .unwrap_or(IN_PROCESS_CLIENT_IP)
}

/// The country the edge resolves for one viewer request — the ONE value this request's
/// country has, read by the distribution's geographic restriction and by the header an origin
/// receives alike.
pub fn viewer_country(req: &RtRequest) -> String {
    match req.headers().get(VIEWER_COUNTRY) {
        Some(stated) if is_country_code(stated) => stated.to_ascii_uppercase(),
        // A viewer that stated no country, or stated something that is not a country code.
        _ => EDGE_COUNTRY.to_string(),
    }
}

/// The scheme a viewer dialled, read off the request's own URI.
///
/// The one reading of it: the header the edge generates for the origin
/// ([`Generated::of_request`]) and the behaviour's own `ViewerProtocolPolicy` decision
/// (`super::viewer_protocol_plan`) both ask this, so a request is one scheme's whichever
/// asks.
pub fn viewer_scheme(uri: &str) -> &'static str {
    match uri.split_once("://") {
        Some((scheme, _rest)) if scheme.eq_ignore_ascii_case(PLAINTEXT_SCHEME) => PLAINTEXT_SCHEME,
        // An absolute URI naming any other scheme, and an origin-form URI, which the
        // emulator's own TLS endpoints are reached with.
        _ => DEFAULT_SCHEME,
    }
}

/// Whether `code` is a code ISO 3166-1 ASSIGNS to a country.
///
/// Two ASCII letters, and not one of the codes the standard RESERVES for user assignment —
/// `AA`, `QM` through `QZ`, `XA` through `XZ` and `ZZ` — which name no country and so cannot
/// be a country a viewer is in.
fn is_country_code(code: &str) -> bool {
    let mut letters = code.chars();
    let (Some(first), Some(second), None) = (letters.next(), letters.next(), letters.next()) else {
        return false;
    };
    if !first.is_ascii_alphabetic() || !second.is_ascii_alphabetic() {
        return false;
    }
    let (first, second) = (
        first.to_ascii_uppercase(),
        second.to_ascii_uppercase(),
    );
    let user_assigned = match first {
        'A' => second == 'A',
        'Q' => ('M'..='Z').contains(&second),
        'X' => second.is_ascii_uppercase(),
        'Z' => second == 'Z',
        _ => false,
    };
    !user_assigned
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// A viewer request carrying `headers`, addressed at `uri`.
    fn request(uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri.to_string()).expect("a valid URI");
        for (name, value) in headers {
            req.headers_mut()
                .insert((*name).to_string(), (*value).to_string());
        }
        req
    }

    #[test]
    fn the_edge_generates_a_country_and_the_viewers_scheme() {
        // Rows differ along what the viewer stated for the country — nothing, a code, a
        // lower-case code, a code ISO reserves for user assignment, something that is not a
        // code at all — and along the scheme its request was addressed with.
        // (case, the URI it was addressed at, the headers it carried, the country, the
        // scheme)
        type Row = (
            &'static str,
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static str,
            &'static str,
        );
        let cases: [Row; 7] = [
            ("a viewer that stated nothing", "/obj", &[], EDGE_COUNTRY, "https"),
            (
                "a viewer in a country it stated",
                "/obj",
                &[(VIEWER_COUNTRY, "DE")],
                "DE",
                "https",
            ),
            (
                "a country stated in lower case",
                "/obj",
                &[(VIEWER_COUNTRY, "de")],
                "DE",
                "https",
            ),
            (
                "a code ISO reserves for user assignment",
                "/obj",
                &[(VIEWER_COUNTRY, "ZZ")],
                EDGE_COUNTRY,
                "https",
            ),
            (
                "another reserved code",
                "/obj",
                &[(VIEWER_COUNTRY, "XK")],
                EDGE_COUNTRY,
                "https",
            ),
            (
                "a value that is not a country code",
                "/obj",
                &[(VIEWER_COUNTRY, "Elbonia")],
                EDGE_COUNTRY,
                "https",
            ),
            (
                "a viewer that dialled the plaintext half of the address",
                "http://d111.cloudfront.net/obj",
                &[],
                EDGE_COUNTRY,
                "http",
            ),
        ];
        for (case, uri, headers, country, scheme) in cases {
            let generated = Generated::of_request(&request(uri, headers));
            assert_eq!(
                (generated.country.as_str(), generated.scheme),
                (country, scheme),
                "case {case:?}: expected the country {country:?} and the scheme {scheme:?}, \
                 got {generated:?}"
            );
            // Whatever it resolved to, the country is a two-letter uppercase code, which is
            // what an origin reading this header parses.
            assert!(
                generated.country.len() == 2
                    && generated
                        .country
                        .chars()
                        .all(|letter| letter.is_ascii_uppercase()),
                "case {case:?}: expected a two-letter uppercase country code, got {:?}",
                generated.country
            );
        }
    }

    #[test]
    fn the_edge_reads_one_address_for_one_viewer() {
        // Rows differ along what the request says about where it came from: nothing, one
        // address, a proxy chain (the viewer is the FIRST entry), an entry that is not an
        // address at all, an IPv6 address, and a header whose single entry is blank.
        // (case, the headers it carried, the address the edge reads for it)
        type Row = (&'static str, &'static [(&'static str, &'static str)], &'static str);
        let cases: [Row; 7] = [
            ("a request that stated nothing", &[], IN_PROCESS_CLIENT_IP),
            (
                "a viewer that stated its address",
                &[(FORWARDED_FOR_HEADER, "203.0.113.7")],
                "203.0.113.7",
            ),
            (
                "a proxy chain, whose first entry is the viewer",
                &[(FORWARDED_FOR_HEADER, "203.0.113.7, 198.51.100.2")],
                "203.0.113.7",
            ),
            (
                "a chain whose entries carry spaces",
                &[(FORWARDED_FOR_HEADER, "  198.51.100.2 , 203.0.113.7")],
                "198.51.100.2",
            ),
            (
                "a value that is not an address",
                &[(FORWARDED_FOR_HEADER, "unknown")],
                IN_PROCESS_CLIENT_IP,
            ),
            (
                "an IPv6 viewer",
                &[(FORWARDED_FOR_HEADER, "2001:db8::1")],
                "2001:db8::1",
            ),
            ("an empty value", &[(FORWARDED_FOR_HEADER, "")], IN_PROCESS_CLIENT_IP),
        ];
        for (case, headers, expected) in cases {
            let req = request("/obj", headers);
            let actual = viewer_address(&req);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the viewer address {expected:?}, got {actual:?}"
            );
            // Whatever it resolved to, it is an address: the source-address condition of a
            // signed URL is evaluated against it.
            assert!(
                actual.parse::<::std::net::IpAddr>().is_ok(),
                "case {case:?}: expected an IP address, got {actual:?}"
            );
            // And it is the value the generated header carries, so one viewer has one
            // address wherever the edge reports it.
            let generated = Generated::of_request(&req);
            assert_eq!(
                generated.address, expected,
                "case {case:?}: expected the generated header to carry {expected:?}, got {:?}",
                generated.address
            );
        }
    }

    #[test]
    fn every_cloudfront_header_is_one_the_edge_generates() {
        // Rows differ along the header name: the two the edge generates, other members of
        // the family it does not, a member spelled in another case, and headers outside it.
        let cases: [(&str, &str, bool); 8] = [
            ("the viewer's country", VIEWER_COUNTRY, true),
            ("the forwarded scheme", FORWARDED_PROTO, true),
            ("another member of the family", "cloudfront-viewer-address", true),
            ("a member as a viewer would spell it", "CloudFront-Viewer-Country", true),
            ("a member in upper case", "CLOUDFRONT-FORWARDED-PROTO", true),
            ("a viewer header", "accept-language", false),
            ("a header that merely mentions it", "x-cloudfront-hint", false),
            ("the empty name", "", false),
        ];
        for (case, name, expected) in cases {
            let actual = is_generated(name);
            assert_eq!(
                actual, expected,
                "case {case:?} ({name:?}): expected is_generated == {expected}, got {actual}"
            );
        }
    }
}
