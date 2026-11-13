// SPDX-License-Identifier: CC0-1.0

//! The CORS configuration an api carries: which apis may carry one at all, and what the
//! values of the lists it holds have to be.
//!
//! A module of its own under [`crate::state`], because the question is one every writer of
//! an api's CORS document asks — `CreateApi` and `UpdateApi` from the modelled `Cors`, and
//! `ImportApi`/`ReimportApi` from the definition's own CORS extension — so the rules and
//! their wording live in one place and cannot differ between them. [`crate::state`]
//! re-exports the entry points, so a handler reaches them the way it reaches the rest of
//! that module.
//!
//! What a configuration is FOR is the answer an api's data plane decorates a cross-origin
//! request with (`crate::service::invoke`): each list it holds becomes an
//! `Access-Control-Allow-…` response header. So a value that could never go into the header
//! it belongs to describes a policy the api could not apply, and the service refuses the
//! write rather than storing it:
//!
//! * `AllowOrigins` carries ORIGINS, whose grammar is the reading beside this one
//!   ([`super::cors_origin`]);
//! * `AllowMethods` carries HTTP METHODS, which is what `Access-Control-Allow-Methods`
//!   lists ([`validate_cors_methods`]), read from the single catalog of the methods this
//!   control plane serves a route for (`crate::ops::import_api::route_methods`);
//! * either list may carry the wildcard [`CORS_WILDCARD`] instead, "the special value `*`
//!   which allows all" of them;
//! * and a configuration that allows CREDENTIALED requests may carry the wildcard in
//!   neither, nor in `AllowHeaders` (see [`validate_cors_configuration`]).
//!
//! `AllowHeaders` and `ExposeHeaders` carry header NAMES, which the HTTP field-name grammar
//! admits almost any token for, and `MaxAge` is a number the member's own schema bounds —
//! so neither has a value rule of its own here.

use super::*;

/// The entry a CORS list carries to stand for "every value": the wildcard.
pub const CORS_WILDCARD: &str = "*";

/// `Ok(())`, or why an api of this `ProtocolType` cannot carry a CORS configuration at
/// all — the message for the operation's `BadRequestException`.
///
/// `CorsConfiguration` is documented "Supported only for HTTP APIs", so a request that
/// configures one on an api of another protocol asks for a setting that protocol has no
/// behaviour for. API Gateway words THIS member's refusal in the member's own terms,
/// naming the CORS feature and the protocol type that does not support it, rather than
/// with either of the sentences [`validate_protocol_restricted_members`] builds for a
/// restricted member: the refusal a WEBSOCKET create carrying a CORS configuration is
/// answered with is `Cors is not supported for WEBSOCKET protocolType ` — the fixed text
/// `Cors is not supported for `, the protocol type the request asked for, then the fixed
/// text ` protocolType ` down to its trailing space, which API Gateway emits and a client
/// comparing the sentence byte for byte requires. The protocol type is the only variable
/// part, so an api of any future non-HTTP protocol is refused in its own name.
///
/// Shared by every writer of an api's CORS document, so the create and the patch cannot
/// word or gate it differently. `configured` is whether THIS request carries one; a
/// request that carries none asks for nothing.
pub fn validate_cors_protocol(protocol_type: &str, configured: bool) -> Result<(), String> {
    if configured && protocol_type != HTTP_PROTOCOL_TYPE {
        return Err(format!(
            "Cors is not supported for {protocol_type} protocolType "
        ));
    }
    Ok(())
}

/// `Ok(())`, or why this CORS configuration cannot hold together — the message for the
/// operation's `BadRequestException`.
///
/// Three rules, each about a document the api could never apply:
///
/// * every `AllowOrigins` entry is an ORIGIN — the wildcard, or the `scheme://host[:port]`
///   serialization RFC 6454 gives one ([`super::cors_origin`]). A value of another form
///   matches no request's `Origin` and goes into no `Access-Control-Allow-Origin`;
/// * every `AllowMethods` entry is an HTTP method the gateway serves, or the wildcard
///   ([`validate_cors_methods`]): a preflight's `Access-Control-Allow-Methods` lists the
///   methods a route may be called with, and a value that is no method names none of them;
/// * a configuration that allows CREDENTIALED cross-origin requests cannot answer them with
///   the wildcard. A browser rejects a credentialed response whose
///   `Access-Control-Allow-Origin` (or `-Methods`, or `-Headers`) is `*`, so the pair
///   describes a policy no client could ever use and the service refuses it instead of
///   storing a document that would never work.
///
/// What is NOT a rule here is an upper bound on `MaxAge`: see the note on the schema that
/// shape's `maxAge` member is validated against (`crate::ops::types::Cors`).
///
/// Shared by every writer of an api's CORS document, so the requirement cannot differ
/// between the create and a later update; an update checks the MERGED configuration,
/// which is the one the api would carry.
pub fn validate_cors_configuration(cors: &wire::Cors) -> Result<(), String> {
    // The VALUES of each list first: a value that names no origin and one that names no
    // method are malformed whatever the rest of the document says.
    if let Some(origins) = cors.allow_origins.as_ref() {
        cors_origin::validate_cors_origins(origins.iter().map(String::as_str))?;
    }
    if let Some(methods) = cors.allow_methods.as_ref() {
        validate_cors_methods(methods.iter().map(String::as_str))?;
    }
    if cors.allow_credentials != Some(true) {
        return Ok(());
    }
    // The message names the response header the wildcard would go into, which is what
    // the browser rule it states is about: `access-control-allow-origin: *` cannot
    // answer a credentialed request, and neither can the method or header forms.
    for (header, values) in [
        ("allow-origin", cors.allow_origins.as_ref()),
        ("allow-methods", cors.allow_methods.as_ref()),
        ("allow-headers", cors.allow_headers.as_ref()),
    ] {
        if values.is_some_and(|values| values.iter().any(|value| value == CORS_WILDCARD)) {
            return Err(format!(
                "allow-credentials is not supported if '{header}' is {CORS_WILDCARD}"
            ));
        }
    }
    Ok(())
}

/// `Ok(())`, or the sentence a configuration naming a value that is not a method is refused
/// with — the message for the operation's `BadRequestException`.
///
/// The methods a configuration may allow are the ones this control plane serves a route
/// for, read from their single source — the OpenAPI operation keys this service's own import
/// maps to a verb (`crate::ops::import_api::route_methods`) — so the two cannot drift. The
/// catch-all a route key may name instead of a method is not one of them: it is no HTTP
/// method, so no `Access-Control-Allow-Methods` lists it.
///
/// As with the origins, the FIRST offending entry in the order the request lists them is the
/// one reported.
pub fn validate_cors_methods<'a, I>(methods: I) -> Result<(), String>
where
    I: IntoIterator<Item = &'a str>,
{
    for method in methods {
        if method != CORS_WILDCARD
            && !crate::ops::import_api::route_methods().any(|served| served == method)
        {
            return Err(unsupported_method_message(method));
        }
    }
    Ok(())
}

/// How API Gateway words the refusal of one value that is not a method it serves: the fixed
/// text `Method `, the value the REQUEST sent, and ` is not supported`.
pub fn unsupported_method_message(method: &str) -> String {
    format!("Method {method} is not supported")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_allowed_method_is_the_wildcard_or_a_method_this_gateway_serves() {
        // Every method of the catalog, the wildcard, and the ways a value stops being one:
        // another verb, the wrong case, the catch-all a route key names (which is no HTTP
        // method), a list written as one value, and the empty value.
        let mut rows: Vec<(&str, bool)> = crate::ops::import_api::route_methods()
            .map(|method| (method, true))
            .collect();
        rows.extend([
            ("*", true),
            ("BOGUS", false),
            ("get", false),
            (crate::ops::import_api::ANY_METHOD, false),
            ("TRACE", false),
            ("CONNECT", false),
            ("GET,POST", false),
            ("", false),
        ]);
        for (method, expected) in rows {
            let validated = validate_cors_methods([method]);
            assert_eq!(
                validated.is_ok(),
                expected,
                "case {method:?}: expected accepted={expected}, got {validated:?}"
            );
            if !expected {
                assert_eq!(
                    validated,
                    Err(format!("Method {method} is not supported")),
                    "case {method:?}: expected the unsupported-method sentence"
                );
            }
        }
    }

    #[test]
    fn the_first_unsupported_method_is_the_one_the_refusal_names() {
        let validated = validate_cors_methods(["GET", "BOGUS", "ALSO-BOGUS"]);
        assert_eq!(
            validated,
            Err("Method BOGUS is not supported".to_string()),
            "expected the first unsupported entry to be named, got {validated:?}"
        );
    }
}
