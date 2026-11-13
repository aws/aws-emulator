// SPDX-License-Identifier: CC0-1.0

//! The ISSUER a JWT authorizer records: what `JWTConfiguration$Issuer` has to be written as,
//! and the sentences API Gateway refuses a configuration that cannot name an identity
//! provider with.
//!
//! A module of its own under [`crate::state`], because both writers of an authorizer
//! (`CreateAuthorizer` and `UpdateAuthorizer`, through
//! [`crate::state::validate_authorizer_config`]) hold the member to it, so the rule and its
//! wording cannot differ between them.
//!
//! `Issuer` is "the base domain of the identity provider that issues JSON Web Tokens. For
//! example, an Amazon Cognito user pool has the following format:
//! https://cognito-idp.{region}.amazonaws.com/{userPoolId}". What API Gateway does with it
//! when the authorizer is CREATED is fetch that provider's OpenID discovery document, at the
//! issuer followed by [`DISCOVERY_PATH`] — the location OpenID Connect Discovery 1.0 §4
//! defines — and the refusal it answers when that cannot be done names the URL it tried and
//! states what an issuer has to be ([`invalid_issuer_message`]).
//!
//! WHAT IS DECIDED HERE. Whether a value can CARRY that discovery endpoint at all is
//! decided from the value: the specification requires the issuer identifier to be "a URL
//! using the https scheme with no query or fragment components" (OpenID Connect Discovery
//! 1.0 §2), so a value that is not one has no discovery endpoint to end with
//! [`DISCOVERY_PATH`] and the write is refused ([`validate_jwt_issuer`]). Whether the
//! provider it names PUBLISHES the document is then resolved by fetching it over the
//! in-process transport ([`unfetchable_discovery_document`]): a provider this emulator
//! serves answers, and one outside it is reached by nothing and refused for nothing — this
//! emulator makes no outbound connection, the same reading [`crate::service::jwt`] states
//! for the signing key it cannot fetch either.

/// The path a provider publishes its OpenID discovery document at, appended to the issuer:
/// OpenID Connect Discovery 1.0 §4 defines `/.well-known/openid-configuration`, and API
/// Gateway's refusal quotes it.
pub const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// The scheme an issuer identifier is written with: OpenID Connect Discovery 1.0 §2 requires
/// "a URL using the `https` scheme".
const HTTPS_SCHEME: &str = "https";

/// How API Gateway words the refusal of a `JwtConfiguration` a JWT authorizer carries none
/// of: the member, and the type that requires it ("Required for the JWT authorizer type",
/// `CreateAuthorizer$JwtConfiguration`). Every part of it is fixed text.
pub const JWT_CONFIGURATION_REQUIRED_MESSAGE: &str =
    "JwtConfiguration must not be null for JWT Authorizer";

/// How API Gateway words the refusal of a `JwtConfiguration` that carries no issuer, or
/// carries it empty: a JWT authorizer validates a token against the provider the issuer
/// names, so there is nothing to validate against. Every part of it is fixed text, including
/// the `Invalid issuer: ` the service prefixes every issuer refusal with.
pub const EMPTY_ISSUER_MESSAGE: &str =
    "Invalid issuer: Issuer can not be empty for JWT Authorizer.";

/// How API Gateway words the refusal of an issuer it could not fetch a discovery document
/// from: the URL it tried, the issuer the attempt was for, and the requirement an issuer has
/// to satisfy. The ISSUER is the only variable part, taken from the request, and it appears
/// three times — once in the URL that was tried, once naming the issuer the attempt was for,
/// and once in the inner error.
pub fn invalid_issuer_message(issuer: &str) -> String {
    format!(
        "Caught exception when connecting to {issuer}{DISCOVERY_PATH} for issuer {issuer}. \
         Please try again later. Error: Invalid issuer: {issuer}. Issuer must have a valid \
         discovery endpoint ended with '{DISCOVERY_PATH}'"
    )
}

/// `Ok(())`, or why this JWT authorizer's issuer cannot name an identity provider — the
/// message for the operation's `BadRequestException`.
///
/// `issuer` is the value the configuration carries, and `None` is a configuration that names
/// none; a value written empty says as little as one left out, so both take
/// [`EMPTY_ISSUER_MESSAGE`].
///
/// This is the half decidable from the VALUE. Whether the provider the value names publishes
/// the document is [`unfetchable_discovery_document`].
pub fn validate_jwt_issuer(issuer: Option<&str>) -> Result<(), String> {
    match issuer {
        None | Some("") => Err(EMPTY_ISSUER_MESSAGE.to_string()),
        Some(issuer) if carries_a_discovery_endpoint(issuer) => Ok(()),
        Some(issuer) => Err(invalid_issuer_message(issuer)),
    }
}

/// The message a JWT authorizer whose issuer publishes no discovery document is refused
/// with, or `None` when there is no evidence that it does not.
///
/// API Gateway FETCHES `<issuer>{DISCOVERY_PATH}` when the authorizer is written, so this
/// fetches it too — and the fetch is made over the emulator's own IN-PROCESS transport
/// (`OperationContext::callback`), the same transport every other cross-service question of
/// this crate is asked over: no socket is opened, no name is resolved and nothing leaves
/// this process, so the only destinations the call can reach are the services this emulator
/// itself hosts, whatever host the issuer names. The framework stamps the call's
/// re-entrancy depth, so a chain of them is bounded by the router rather than by the value a
/// caller sent; one write makes at most one such call.
///
/// What the answer says:
///
/// * a 2xx is the discovery document the provider publishes, so the issuer resolves;
/// * any other answer is the provider's own, and it is not a discovery document: the issuer
///   has no discovery endpoint at [`DISCOVERY_PATH`] and the write is refused. An api of
///   THIS control plane answering its own endpoint is the case the recorded create ran into:
///   an api serves its routes, not an identity provider's metadata;
/// * a call that reached nothing — a host no service of this emulator claims
///   (`service_common::in_process_refusal`), or a transport that could not carry it — is
///   evidence of NOTHING: the issuer names a provider outside this emulator, which this
///   emulator makes no outbound connection to, so nothing is refused for it and nothing
///   about the provider is invented. That is the same reading
///   [`crate::state::ec2_subnets`] takes of silence from the owner of subnets.
///
/// Call it with no state guard held: it awaits.
pub async fn unfetchable_discovery_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    issuer: &str,
) -> Option<String> {
    use ::aws_smithy_runtime_api::client::http::HttpConnector;

    // The request is made the way a client on this emulator's own network makes one: the
    // HOST the issuer names, and the discovery document's path under whatever path the
    // issuer carries (a Cognito user pool's issuer is the pool under the service's host).
    let parsed = ::url::Url::parse(issuer).ok()?;
    let authority = match parsed.port() {
        Some(port) => format!("{}:{port}", parsed.host_str()?),
        None => parsed.host_str()?.to_string(),
    };
    let document = format!("{}{DISCOVERY_PATH}", parsed.path().trim_end_matches('/'));
    let Ok(request) = ::http::Request::get(document.as_str())
        .header(::http::header::HOST, authority.as_str())
        .body(::aws_smithy_types::body::SdkBody::empty())
    else {
        // A URI or host the transport cannot even be handed is one nothing was asked
        // about.
        return None;
    };
    let Ok(request) = ::aws_smithy_runtime_api::http::Request::try_from(request) else {
        return None;
    };
    let Ok(answer) = ctx.callback().http_connector().call(request).await else {
        ::tracing::warn!(
            %issuer,
            "the OpenID discovery document a JWT authorizer's issuer names could not be \
             fetched in process"
        );
        return None;
    };
    if answer.status().is_success() {
        return None;
    }
    // The front door's own refusal of a call that reached no service is not the provider's
    // answer, so it says nothing about the issuer.
    let reached_nothing = answer
        .body()
        .bytes()
        .is_some_and(::service_common::in_process_refusal::is_transport_refusal);
    (!reached_nothing).then(|| invalid_issuer_message(issuer))
}

/// Whether `issuer` is a value that CAN carry the discovery endpoint API Gateway fetches: an
/// issuer identifier as OpenID Connect Discovery 1.0 §2 defines one — a URL using the https
/// scheme, with a host, and with no query and no fragment component, since the discovery
/// path is appended to it.
///
/// The value is a URL a caller sent, so it is READ by the maintained `url` parser rather than
/// split here; what this adds is the ISSUER rule on top of the parse.
fn carries_a_discovery_endpoint(issuer: &str) -> bool {
    let Ok(parsed) = ::url::Url::parse(issuer) else {
        return false;
    };
    parsed.scheme() == HTTPS_SCHEME
        && parsed.host_str().is_some_and(|host| !host.is_empty())
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issuer_is_an_https_url_a_discovery_path_can_be_appended_to() {
        // The rows span what the rule generalises over: the documented Cognito form, an
        // issuer with and without a path, with and without a trailing slash, a port, and
        // the ways a value stops being an issuer identifier (no scheme, the wrong scheme,
        // no host, a query, a fragment, whitespace).
        let rows: &[(&str, bool)] = &[
            (
                "https://cognito-idp.us-west-2.amazonaws.com/us-west-2_abc123def",
                true,
            ),
            ("https://issuer.example.com", true),
            ("https://issuer.example.com/", true),
            ("https://issuer.example.com/oauth2", true),
            ("https://issuer.example.com:8443/oauth2", true),
            ("http://issuer.example.com", false),
            ("issuer.example.com", false),
            ("https://", false),
            ("https://issuer.example.com?v=1", false),
            ("https://issuer.example.com#fragment", false),
            ("https://issuer example.com", false),
            ("https://user@issuer.example.com", false),
        ];
        for (issuer, expected) in rows {
            let validated = validate_jwt_issuer(Some(issuer));
            assert_eq!(
                validated.is_ok(),
                *expected,
                "case {issuer:?}: expected accepted={expected}, got {validated:?}"
            );
            if !*expected {
                assert_eq!(
                    validated,
                    Err(invalid_issuer_message(issuer)),
                    "case {issuer:?}: expected the invalid-issuer sentence"
                );
            }
        }
    }

    #[test]
    fn a_configuration_naming_no_issuer_is_refused_for_the_empty_member() {
        for issuer in [None, Some("")] {
            let validated = validate_jwt_issuer(issuer);
            assert_eq!(
                validated,
                Err(EMPTY_ISSUER_MESSAGE.to_string()),
                "case {issuer:?}: expected the empty-issuer sentence"
            );
        }
    }

    #[test]
    fn the_invalid_issuer_sentence_names_the_issuer_three_times() {
        let message = invalid_issuer_message("https://no-document.example.com");
        assert_eq!(
            message.matches("https://no-document.example.com").count(),
            3,
            "expected the issuer in the URL, the subject and the inner error, got {message}"
        );
        assert!(
            message.starts_with("Caught exception when connecting to ")
                && message.ends_with(
                    "Issuer must have a valid discovery endpoint ended with \
                     '/.well-known/openid-configuration'"
                ),
            "expected the service's own fixed text around it, got {message}"
        );
    }
}
