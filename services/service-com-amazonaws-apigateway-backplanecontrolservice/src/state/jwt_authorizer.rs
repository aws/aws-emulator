// SPDX-License-Identifier: CC0-1.0

//! The JWT configuration a `JWT` authorizer carries: the record it is stored as, what a
//! request may declare in it, and the configuration a read of the authorizer reports.
//!
//! `JWT` is the `AuthorizerType` "for using a native JWT authorizer with the trusted
//! issuers, audiences, and signing algorithms that you declare" (`Authorizer$type`), and
//! `jwtConfiguration` is what declares them — "the configuration returned for a JWT
//! authorizer. This field is present only when `type` is `JWT`"
//! (`Authorizer$jwtConfiguration`). The authorizer verifies a presented token against that
//! declaration, so what the members say about a value they admit is a property of the
//! RECORD: every clause below is one of those sentences, and an authorizer the clause
//! refuses could verify no token at all.
//!
//! * `issuers` — "the issuers that the authorizer trusts. Issuer URLs must be unique"
//!   (`JwtConfigurationInput$issuers`), and each one "must be an absolute `https` URL that
//!   includes a host and has no user information, query, or fragment"
//!   (`JwtIssuerInput$issuer`).
//! * `audiences` — "the audiences that the issuer can issue tokens for. None can be empty"
//!   (`JwtIssuerInput$audiences`).
//! * `defaultAlgorithms` — "the signing algorithms that the authorizer accepts, applied to
//!   every issuer" (`JwtConfigurationInput$defaultAlgorithms`); WHICH values are signing
//!   algorithms is the modelled `JwtSigningAlgorithm` enum, so the constraint layer in
//!   front of the operation refuses a value outside it before a handler runs.
//! * `additionalClaims` — "the additional claim rules for this issuer. Claim names must be
//!   unique within the issuer" (`JwtIssuerInput$additionalClaims`), each naming a claim that
//!   "cannot be empty and cannot be `iss`, `aud`, `exp`, `nbf`, `iat`, `scope`, or `scp`"
//!   ([`RESERVED_CLAIM_NAMES`]) and carrying accepted values of which "the value cannot be
//!   empty" (`JwtAdditionalClaim$expectedValues`).
//!
//! `jwksUri` is the one member of the reported configuration no request supplies: "the JSON
//! Web Key Set (JWKS) URI that API Gateway resolved from the issuer's OpenID Connect
//! discovery document. This value is read-only and cannot be set" (`JwtIssuer$jwksUri`), so
//! it is derived from the issuer when the record is REPORTED rather than stored — the
//! tree's one locator of a key set under an issuer
//! ([`::service_common::openid_discovery::jwks_uri`]) answers where that document points.
//!
//! Every refusal here is the one sentence this service answers a configuration it does not
//! admit with ([`INVALID_REQUEST_INPUT`]), which is how the type-coherence rule beside it
//! words every clause of its own; the emulator's resource guards keep their own wording,
//! which names the member and the bound it broke.

use std::collections::BTreeSet;

use super::{
    bound_chars, bound_entries, AuthorizerEntry, MAX_COLLECTION_ENTRIES, MAX_STRING_CHARS,
};
use crate::ops::create_authorizer::{AUTHORIZER_TYPE_JWT, INVALID_REQUEST_INPUT};
use crate::ops::types::{
    JwtAdditionalClaim, JwtConfiguration, JwtConfigurationInput, JwtIssuer, JwtIssuerInput,
};

/// The claim names an additional claim rule may not name: "the value cannot be empty and
/// cannot be `iss`, `aud`, `exp`, `nbf`, `iat`, `scope`, or `scp`"
/// (`JwtAdditionalClaim$claimName`). Each is a claim the authorizer reads itself — the
/// issuer, the audience and the window it checks a token's own
/// [`::service_common::jwt`] reading of against, and the scopes a method's
/// `authorizationScopes` are matched against — so a rule over one of them would restate a
/// check the authorizer already makes.
pub const RESERVED_CLAIM_NAMES: &[&str] = &["iss", "aud", "exp", "nbf", "iat", "scope", "scp"];

/// The scheme an issuer URL is written in: "the value must be an absolute `https` URL"
/// (`JwtIssuerInput$issuer`).
const ISSUER_SCHEME: &str = "https";

/// The stored `jwtConfiguration` of a `JWT` authorizer: the issuers it trusts and the
/// signing algorithms it accepts, kept exactly as the request declared them so a reader
/// reports what was stored rather than a value invented per read.
#[derive(Debug, Clone, Default)]
pub struct JwtConfigurationEntry {
    /// The trusted issuers, in the order the request declared them; never empty, and no two
    /// entries carry the same issuer URL.
    pub issuers: Vec<JwtIssuerEntry>,
    /// The signing algorithms the authorizer accepts, "applied to every issuer"; each is a
    /// value of the modelled `JwtSigningAlgorithm` enum.
    pub default_algorithms: Vec<String>,
}

/// One trusted issuer of a stored configuration.
///
/// `additional_claims` is a list rather than an option because a request declaring none and
/// a request declaring an empty one make the same claim — that the issuer carries no
/// additional claim rule — and the reported configuration says so by carrying no member.
#[derive(Debug, Clone, Default)]
pub struct JwtIssuerEntry {
    pub issuer: String,
    pub audiences: Vec<String>,
    pub additional_claims: Vec<JwtAdditionalClaim>,
}

/// Whether `value` is an issuer URL as `JwtIssuerInput$issuer` documents one: "an absolute
/// `https` URL that includes a host and has no user information, query, or fragment".
///
/// The scheme is compared case-insensitively, as RFC 3986 §3.1 makes a scheme, while the
/// value is stored as written: the issuer "must exactly match the `iss` claim of the token",
/// so nothing about it is normalised. The host is read by the tree's one reading of a host's
/// form ([`::service_common::data_plane_addressing::is_host_shaped`]), which is also what
/// bounds its length; a value carrying whitespace anywhere names no reachable issuer.
#[must_use]
pub fn names_an_issuer(value: &str) -> bool {
    if value.contains(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case(ISSUER_SCHEME) {
        return false;
    }
    if rest.contains(['?', '#']) {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    // "no user information": the `userinfo@` an authority may carry (RFC 3986 §3.2.1).
    if authority.contains('@') {
        return false;
    }
    ::service_common::data_plane_addressing::is_host_shaped(authority)
}

/// The configuration a `JWT` authorizer is stored with, or the reason the requested one is
/// refused.
///
/// Every clause is a member's own documentation, as this module's own docs record; the
/// sizes are the emulator's resource guards, which an over-sized request is refused by
/// before anything is stored.
pub fn stored_configuration(
    requested: &JwtConfigurationInput,
) -> Result<JwtConfigurationEntry, String> {
    bound_entries(
        "jwtConfiguration.issuers",
        requested.issuers.len(),
        MAX_COLLECTION_ENTRIES,
    )?;
    bound_entries(
        "jwtConfiguration.defaultAlgorithms",
        requested.default_algorithms.len(),
        MAX_COLLECTION_ENTRIES,
    )?;
    // An authorizer that trusts no issuer, or accepts no signing algorithm, accepts no
    // token at all: the members are what the authorizer verifies a token AGAINST, so
    // neither list can be empty.
    if requested.issuers.is_empty() || requested.default_algorithms.is_empty() {
        return Err(INVALID_REQUEST_INPUT.to_string());
    }
    for algorithm in &requested.default_algorithms {
        bound_chars(
            "jwtConfiguration.defaultAlgorithms",
            algorithm,
            MAX_STRING_CHARS,
        )?;
    }
    let mut issuers: Vec<JwtIssuerEntry> = Vec::with_capacity(requested.issuers.len());
    for requested_issuer in &requested.issuers {
        issuers.push(stored_issuer(requested_issuer)?);
    }
    // "Issuer URLs must be unique": two entries for one issuer would be two readings of
    // the same `iss`, and the authorizer could not say which of them a token is held to.
    let distinct: BTreeSet<&str> = issuers.iter().map(|issuer| issuer.issuer.as_str()).collect();
    if distinct.len() != issuers.len() {
        return Err(INVALID_REQUEST_INPUT.to_string());
    }
    Ok(JwtConfigurationEntry {
        issuers,
        default_algorithms: requested.default_algorithms.clone(),
    })
}

/// One trusted issuer as it is stored, or the reason the requested one is refused.
fn stored_issuer(requested: &JwtIssuerInput) -> Result<JwtIssuerEntry, String> {
    bound_chars(
        "jwtConfiguration.issuers.issuer",
        &requested.issuer,
        MAX_STRING_CHARS,
    )?;
    bound_entries(
        "jwtConfiguration.issuers.audiences",
        requested.audiences.len(),
        MAX_COLLECTION_ENTRIES,
    )?;
    if !names_an_issuer(&requested.issuer) {
        return Err(INVALID_REQUEST_INPUT.to_string());
    }
    // "The token's `aud` claim must contain at least one of these values", so an issuer
    // listing none admits no token of that issuer, and "none can be empty".
    if requested.audiences.is_empty() {
        return Err(INVALID_REQUEST_INPUT.to_string());
    }
    for audience in &requested.audiences {
        bound_chars(
            "jwtConfiguration.issuers.audiences",
            audience,
            MAX_STRING_CHARS,
        )?;
        if audience.is_empty() {
            return Err(INVALID_REQUEST_INPUT.to_string());
        }
    }
    let additional_claims = requested.additional_claims.clone().unwrap_or_default();
    bound_entries(
        "jwtConfiguration.issuers.additionalClaims",
        additional_claims.len(),
        MAX_COLLECTION_ENTRIES,
    )?;
    {
        let mut named: BTreeSet<&str> = BTreeSet::new();
        for claim in &additional_claims {
            require_claim_rule(claim)?;
            // "Claim names must be unique within the issuer": two rules over one claim
            // name are two answers to the same question about a token.
            if !named.insert(claim.claim_name.as_str()) {
                return Err(INVALID_REQUEST_INPUT.to_string());
            }
        }
    }
    Ok(JwtIssuerEntry {
        issuer: requested.issuer.clone(),
        audiences: requested.audiences.clone(),
        additional_claims,
    })
}

/// Refuse an additional claim rule that names no claim the authorizer may check, or that
/// accepts an empty value for one.
fn require_claim_rule(claim: &JwtAdditionalClaim) -> Result<(), String> {
    bound_chars(
        "jwtConfiguration.issuers.additionalClaims.claimName",
        &claim.claim_name,
        MAX_STRING_CHARS,
    )?;
    if claim.claim_name.is_empty() || RESERVED_CLAIM_NAMES.contains(&claim.claim_name.as_str()) {
        return Err(INVALID_REQUEST_INPUT.to_string());
    }
    let Some(expected_values) = &claim.expected_values else {
        return Ok(());
    };
    bound_entries(
        "jwtConfiguration.issuers.additionalClaims.expectedValues",
        expected_values.len(),
        MAX_COLLECTION_ENTRIES,
    )?;
    for value in expected_values {
        bound_chars(
            "jwtConfiguration.issuers.additionalClaims.expectedValues",
            value,
            MAX_STRING_CHARS,
        )?;
        // "The value cannot be empty. When a value is specified, the claim must match it
        // exactly": a rule accepting the empty string accepts no claim value at all.
        if value.is_empty() {
            return Err(INVALID_REQUEST_INPUT.to_string());
        }
    }
    Ok(())
}

/// The `JwtConfiguration` body a stored configuration is reported as — the one place this
/// crate renders that shape, so every reader of an authorizer reports one record the same
/// way.
///
/// `jwksUri` is filled in here because it is read-only and derived: it is where the issuer
/// publishes the key set a verifier of its tokens fetches, which the tree's one locator of
/// that document answers. A list the record does not carry is reported absent rather than
/// empty, which is what an issuer declaring no additional claim rule says on the wire.
#[must_use]
pub fn configuration_body(entry: &JwtConfigurationEntry) -> JwtConfiguration {
    JwtConfiguration {
        issuers: Some(
            entry
                .issuers
                .iter()
                .map(|issuer| JwtIssuer {
                    issuer: Some(issuer.issuer.clone()),
                    audiences: Some(issuer.audiences.clone()),
                    additional_claims: (!issuer.additional_claims.is_empty())
                        .then(|| issuer.additional_claims.clone()),
                    jwks_uri: Some(::service_common::openid_discovery::jwks_uri(&issuer.issuer)),
                })
                .collect(),
        ),
        default_algorithms: Some(entry.default_algorithms.clone()),
    }
}

/// Refuse an authorizer whose `jwtConfiguration` its own `type` does not admit.
///
/// The member "is present only when `type` is `JWT`" (`Authorizer$jwtConfiguration`) and a
/// `JWT` authorizer declares "the trusted issuers, audiences, and signing algorithms"
/// through it (`AuthorizerType`), so the two go together: a `JWT` authorizer without a
/// configuration verifies nothing, and an authorizer of any other type with one carries a
/// declaration nothing reads.
///
/// The check reads the STORED record, for the reason
/// [`crate::ops::create_authorizer::require_members_of_type`] records: the rule holds of the
/// record however it came to look that way, so the create judges the record it would store
/// and `UpdateAuthorizer` the record its patches produced.
pub fn require_configuration_of_type(entry: &AuthorizerEntry) -> Result<(), String> {
    let declared = entry.jwt_configuration.is_some();
    if declared != (entry.r#type == AUTHORIZER_TYPE_JWT) {
        return Err(INVALID_REQUEST_INPUT.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claim rule naming `claim_name`, accepting `expected_values`.
    fn claim(claim_name: &str, expected_values: Option<&[&str]>) -> JwtAdditionalClaim {
        JwtAdditionalClaim {
            claim_name: claim_name.to_string(),
            // The one value `JwtClaimFormat` accepts, which the constraint layer in front
            // of the operation is what checks.
            format: "SINGLE_STRING".to_string(),
            required: false,
            expected_values: expected_values
                .map(|values| values.iter().map(|value| (*value).to_string()).collect()),
        }
    }

    /// An issuer declaration over `issuer` and `audiences`, with `claims`.
    fn issuer(issuer: &str, audiences: &[&str], claims: Vec<JwtAdditionalClaim>) -> JwtIssuerInput {
        JwtIssuerInput {
            issuer: issuer.to_string(),
            audiences: audiences
                .iter()
                .map(|audience| (*audience).to_string())
                .collect(),
            additional_claims: (!claims.is_empty()).then_some(claims),
        }
    }

    /// A configuration over `issuers`, accepting `algorithms`.
    fn configuration(issuers: Vec<JwtIssuerInput>, algorithms: &[&str]) -> JwtConfigurationInput {
        JwtConfigurationInput {
            issuers,
            default_algorithms: algorithms
                .iter()
                .map(|algorithm| (*algorithm).to_string())
                .collect(),
        }
    }

    /// RULE: an issuer URL is an absolute `https` URL that includes a host and carries no
    /// user information, query or fragment.
    #[test]
    fn an_issuer_url_is_an_absolute_https_url_naming_a_host() {
        // Rows span the ways a value can satisfy or miss each clause of the member:
        // the scheme (present, other, absent, written in another case), the host (named,
        // empty, carrying user information), and the parts the member excludes.
        let rows: &[(&str, &str, bool)] = &[
            ("an issuer with a path", "https://issuer.example.com/tenant", true),
            ("an issuer with no path", "https://issuer.example.com", true),
            ("one with a port", "https://issuer.example.com:8443/tenant", true),
            ("one with a trailing slash", "https://issuer.example.com/", true),
            ("a scheme in another case", "HTTPS://issuer.example.com", true),
            ("a plaintext scheme", "http://issuer.example.com", false),
            ("no scheme at all", "issuer.example.com", false),
            ("a relative reference", "//issuer.example.com/tenant", false),
            ("user information", "https://user@issuer.example.com", false),
            ("a query", "https://issuer.example.com/t?tenant=1", false),
            ("a fragment", "https://issuer.example.com/t#part", false),
            ("no host", "https:///tenant", false),
            ("whitespace inside", "https://issuer.example.com/a b", false),
            ("the empty string", "", false),
        ];
        for (case, value, expected) in rows {
            let actual = names_an_issuer(value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected} for {value:?}, got {actual}"
            );
        }
    }

    /// RULE: a stored configuration is one whose issuers are unique and whose every issuer
    /// names an issuer URL, lists at least one non-empty audience, and carries at most one
    /// rule per claim name — over names no rule may take.
    #[test]
    fn a_configuration_is_stored_only_when_every_member_admits_it() {
        // Rows differ along which clause the configuration breaks, and how many issuers,
        // audiences and claim rules it carries.
        let rows: &[(&str, JwtConfigurationInput, bool)] = &[
            (
                "one issuer over one audience",
                configuration(
                    vec![issuer("https://one.example.com", &["api"], vec![])],
                    &["RS256"],
                ),
                true,
            ),
            (
                "two issuers, two audiences each, two algorithms",
                configuration(
                    vec![
                        issuer("https://one.example.com", &["api", "web"], vec![]),
                        issuer("https://two.example.com", &["api", "cli"], vec![]),
                    ],
                    &["RS256", "ES512"],
                ),
                true,
            ),
            (
                "an issuer carrying two claim rules",
                configuration(
                    vec![issuer(
                        "https://one.example.com",
                        &["api"],
                        vec![
                            claim("tenant", Some(&["acme"])),
                            claim("department", None),
                        ],
                    )],
                    &["RS384"],
                ),
                true,
            ),
            (
                "no issuer at all",
                configuration(vec![], &["RS256"]),
                false,
            ),
            (
                "no signing algorithm at all",
                configuration(
                    vec![issuer("https://one.example.com", &["api"], vec![])],
                    &[],
                ),
                false,
            ),
            (
                "two entries for one issuer URL",
                configuration(
                    vec![
                        issuer("https://one.example.com", &["api"], vec![]),
                        issuer("https://one.example.com", &["web"], vec![]),
                    ],
                    &["RS256"],
                ),
                false,
            ),
            (
                "an issuer that is no https URL",
                configuration(
                    vec![issuer("http://one.example.com", &["api"], vec![])],
                    &["RS256"],
                ),
                false,
            ),
            (
                "an issuer listing no audience",
                configuration(
                    vec![issuer("https://one.example.com", &[], vec![])],
                    &["RS256"],
                ),
                false,
            ),
            (
                "an empty audience",
                configuration(
                    vec![issuer("https://one.example.com", &["api", ""], vec![])],
                    &["RS256"],
                ),
                false,
            ),
            (
                "two rules for one claim name",
                configuration(
                    vec![issuer(
                        "https://one.example.com",
                        &["api"],
                        vec![claim("tenant", None), claim("tenant", Some(&["acme"]))],
                    )],
                    &["RS256"],
                ),
                false,
            ),
            (
                "a rule over a claim the authorizer reads itself",
                configuration(
                    vec![issuer(
                        "https://one.example.com",
                        &["api"],
                        vec![claim("scp", None)],
                    )],
                    &["RS256"],
                ),
                false,
            ),
            (
                "a rule naming no claim",
                configuration(
                    vec![issuer(
                        "https://one.example.com",
                        &["api"],
                        vec![claim("", None)],
                    )],
                    &["RS256"],
                ),
                false,
            ),
            (
                "a rule accepting an empty value",
                configuration(
                    vec![issuer(
                        "https://one.example.com",
                        &["api"],
                        vec![claim("tenant", Some(&["acme", ""]))],
                    )],
                    &["RS256"],
                ),
                false,
            ),
        ];
        for (case, requested, admitted) in rows {
            let stored = stored_configuration(requested);
            assert_eq!(
                stored.is_ok(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {stored:?}"
            );
            if !admitted {
                assert_eq!(
                    stored.err().as_deref(),
                    Some(INVALID_REQUEST_INPUT),
                    "case {case:?}: a configuration this service does not admit is refused \
                     with its one sentence"
                );
            }
        }
    }

    /// RULE: every reserved claim name is refused as the name of a rule, whichever one it
    /// is.
    #[test]
    fn no_rule_names_a_claim_the_authorizer_reads_itself() {
        for reserved in RESERVED_CLAIM_NAMES {
            let requested = configuration(
                vec![issuer(
                    "https://one.example.com",
                    &["api"],
                    vec![claim(reserved, None)],
                )],
                &["RS256"],
            );
            let stored = stored_configuration(&requested);
            assert_eq!(
                stored.as_ref().err().map(String::as_str),
                Some(INVALID_REQUEST_INPUT),
                "case {reserved:?}: a rule over a reserved claim is refused, got {stored:?}"
            );
        }
    }

    /// RULE: the reported configuration carries the stored declaration, each issuer beside
    /// the key set published under it, and no `additionalClaims` for an issuer declaring
    /// none.
    #[test]
    fn the_reported_configuration_carries_the_key_set_published_under_each_issuer() {
        let stored = stored_configuration(&configuration(
            vec![
                issuer("https://one.example.com/tenant", &["api"], vec![]),
                issuer(
                    "https://two.example.com",
                    &["web", "cli"],
                    vec![claim("tenant", Some(&["acme"]))],
                ),
            ],
            &["RS256", "ES256"],
        ))
        .expect("the declaration above is one every member admits");
        let body = configuration_body(&stored);
        assert_eq!(
            body.default_algorithms.as_deref(),
            Some(&["RS256".to_string(), "ES256".to_string()][..]),
            "the accepted algorithms are reported as declared, got {:?}",
            body.default_algorithms
        );
        let issuers = body.issuers.unwrap_or_default();
        assert_eq!(
            issuers.len(),
            2,
            "both declared issuers are reported, got {issuers:?}"
        );
        assert_eq!(
            issuers[0].jwks_uri.as_deref(),
            Some("https://one.example.com/tenant/.well-known/jwks.json"),
            "the key set sits under the issuer, got {:?}",
            issuers[0].jwks_uri
        );
        assert!(
            issuers[0].additional_claims.is_none(),
            "an issuer declaring no claim rule reports none, got {:?}",
            issuers[0].additional_claims
        );
        assert_eq!(
            issuers[1]
                .additional_claims
                .as_ref()
                .map(|claims| claims.len()),
            Some(1),
            "the declared claim rule is reported, got {:?}",
            issuers[1].additional_claims
        );
    }

    /// RULE: a record carries a `jwtConfiguration` exactly when its own type is `JWT`.
    #[test]
    fn a_configuration_belongs_to_a_jwt_authorizer_and_to_no_other() {
        let declared = stored_configuration(&configuration(
            vec![issuer("https://one.example.com", &["api"], vec![])],
            &["RS256"],
        ))
        .expect("the declaration above is one every member admits");
        // Rows span the catalog `smithy_query(query="shape",
        // shape="com.amazonaws.apigateway#AuthorizerType")` declares, each with and without
        // a configuration.
        for authorizer_type in crate::ops::types::AUTHORIZER_TYPE_VALUES {
            for carried in [None, Some(declared.clone())] {
                let entry = AuthorizerEntry {
                    r#type: (*authorizer_type).to_string(),
                    jwt_configuration: carried.clone(),
                    ..AuthorizerEntry::default()
                };
                let admitted = carried.is_some() == (*authorizer_type == AUTHORIZER_TYPE_JWT);
                let judged = require_configuration_of_type(&entry);
                assert_eq!(
                    judged.is_ok(),
                    admitted,
                    "case {authorizer_type:?} with a configuration={}: expected \
                     admitted={admitted}, got {judged:?}",
                    carried.is_some()
                );
            }
        }
    }
}
