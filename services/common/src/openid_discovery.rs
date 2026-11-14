// SPDX-License-Identifier: CC0-1.0

//! The two documents an OIDC issuer this emulator SERVES publishes about itself, where they
//! are published, and how a request for one is recognised.
//!
//! Several emulated services are such an issuer — a Cognito user pool, an account's
//! outbound web identity federation — and each publishes its JSON Web Key Set and its
//! OpenID Connect discovery document under its own issuer URL, fetched ANONYMOUSLY by
//! whatever relying party holds a token that issuer signed or bootstraps from its
//! discovery document. What is the same for all of them lives here once: the two
//! well-known paths, the locator of the key set under an issuer, the reading of a request
//! path that addresses one of the documents, and the JSON response it is answered with.
//!
//! What a DISCOVERY DOCUMENT says does not: it names the endpoints the provider itself
//! serves, and those differ per provider — a user pool announces its authorization, token,
//! revocation and userinfo endpoints because it serves them, while an STS web identity
//! issuer announces none of them because it serves none. So each issuer renders its own
//! document, out of the pieces here.

/// The path under an issuer its JSON Web Key Set is published at: the location the `iss`
/// claim of a token leads a verifier to, so it can find the key the token's `kid` names.
pub const JWKS_DOCUMENT_PATH: &str = ".well-known/jwks.json";

/// The path under an issuer its OpenID Connect discovery document is published at: the
/// well-known location RFC 8414 §3 defines, which every OIDC client and relying party reads
/// a provider's endpoints from.
pub const OPENID_CONFIGURATION_DOCUMENT_PATH: &str = ".well-known/openid-configuration";

/// Which well-known document of an issuer a request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WellKnownDocument {
    /// The issuer's JSON Web Key Set, which every verifier of a token it minted fetches to
    /// check that token's signature.
    KeySet,
    /// Its OpenID Connect discovery document, which every relying party reads the issuer
    /// and the location of that key set from.
    OpenIdConfiguration,
}

impl WellKnownDocument {
    /// The document published at `tail` — the path under an issuer — or `None` when an
    /// issuer publishes none there.
    #[must_use]
    pub fn at_path(tail: &str) -> Option<Self> {
        match tail {
            JWKS_DOCUMENT_PATH => Some(Self::KeySet),
            OPENID_CONFIGURATION_DOCUMENT_PATH => Some(Self::OpenIdConfiguration),
            _ => None,
        }
    }
}

/// Where `issuer` publishes its key set: the issuer URL followed by [`JWKS_DOCUMENT_PATH`]
/// — the address its discovery document announces and the one a verifier of a token it
/// signed fetches.
#[must_use]
pub fn jwks_uri(issuer: &str) -> String {
    format!("{}/{JWKS_DOCUMENT_PATH}", issuer.trim_end_matches('/'))
}

/// The issuer segment and the document `uri` addresses, and `None` for every other path.
///
/// An issuer this emulator serves is the endpoint it is reached at followed by ONE opaque
/// segment that names the issuer (a user pool's id, an account's web identity issuer
/// segment), so its documents are reached at `/<segment>/.well-known/<document>`. Only the
/// FORM is read here: `addresses_an_issuer` is the service's own answer for whether a
/// segment has the shape IT mints an issuer in, so a path of the same shape addressed to
/// another mounted service is not claimed, and WHICH issuer holds that segment is decided
/// by the store when the document is served.
///
/// The locator may carry a query or a fragment, and a request whose target is absolute
/// carries a scheme and authority before the path; both are stripped before the segments
/// are read.
#[must_use]
pub fn well_known_route<'u>(
    uri: &'u str,
    addresses_an_issuer: &dyn Fn(&str) -> bool,
) -> Option<(&'u str, WellKnownDocument)> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let path = match path.split_once("://") {
        Some((_, authority_and_path)) => {
            let slash = authority_and_path.find('/')?;
            authority_and_path.get(slash..)?
        }
        None => path,
    };
    let (segment, tail) = path.strip_prefix('/')?.split_once('/')?;
    if !addresses_an_issuer(segment) {
        return None;
    }
    Some((segment, WellKnownDocument::at_path(tail)?))
}

/// A JSON document answered on one of these non-modelled routes: `status` with `body` and
/// the media type a JOSE key set and an OIDC discovery document are served in.
///
/// `None` only for a status that is not a valid HTTP status code.
#[must_use]
pub fn json_response(
    status: u16,
    body: &str,
) -> Option<::aws_smithy_runtime_api::http::Response> {
    let mut response = ::aws_smithy_runtime_api::http::Response::new(
        ::aws_smithy_runtime_api::http::StatusCode::try_from(status).ok()?,
        ::aws_smithy_types::body::SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", "application/json");
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below: the case it is about, the request path, and the route
    /// that path names.
    type Row = (&'static str, &'static str, Option<(&'static str, WellKnownDocument)>);

    /// RULE: only `/<segment>/.well-known/<a document an issuer publishes>` names an
    /// issuer's document, and only for a segment the service says is one of its own.
    #[test]
    fn only_an_issuers_well_known_paths_name_a_document() {
        let addresses = |segment: &str| segment == "ISSUER01";
        // One row: the case, the path, and the route it names.
        let rows: &[Row] = &[
            (
                "the key set",
                "/ISSUER01/.well-known/jwks.json",
                Some(("ISSUER01", WellKnownDocument::KeySet)),
            ),
            (
                "the discovery document",
                "/ISSUER01/.well-known/openid-configuration",
                Some(("ISSUER01", WellKnownDocument::OpenIdConfiguration)),
            ),
            (
                "the key set with a cache-buster",
                "/ISSUER01/.well-known/jwks.json?v=1",
                Some(("ISSUER01", WellKnownDocument::KeySet)),
            ),
            (
                "an absolute request target",
                "https://localhost:9297/ISSUER01/.well-known/jwks.json",
                Some(("ISSUER01", WellKnownDocument::KeySet)),
            ),
            (
                "a segment of another service",
                "/my-bucket/.well-known/jwks.json",
                None,
            ),
            (
                "a document no issuer publishes",
                "/ISSUER01/.well-known/oauth-authorization-server",
                None,
            ),
            ("the issuer itself", "/ISSUER01", None),
            ("no segment at all", "//.well-known/jwks.json", None),
            ("a relative name", "jwks.json", None),
            ("the root", "/", None),
        ];
        for (case, path, expected) in rows {
            let actual = well_known_route(path, &addresses);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?} for {path}, got {actual:?}"
            );
        }
    }

    /// RULE: the key set's locator is the issuer followed by its well-known path, whether
    /// or not the issuer URL was given with a trailing slash.
    #[test]
    fn the_key_set_sits_under_the_issuer() {
        for (case, issuer) in [
            ("an issuer URL", "https://localhost:9297/ISSUER01"),
            ("one with a trailing slash", "https://localhost:9297/ISSUER01/"),
        ] {
            let uri = jwks_uri(issuer);
            assert_eq!(
                uri, "https://localhost:9297/ISSUER01/.well-known/jwks.json",
                "case {case:?}: unexpected locator {uri}"
            );
        }
    }
}
