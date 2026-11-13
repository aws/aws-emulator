// SPDX-License-Identifier: CC0-1.0

//! Whether the custom endpoint a domain's endpoint options name is a fully qualified
//! domain name — the one input constraint of those options no walk ahead of a handler
//! applies — and whether the custom endpoint those options ask the service to SERVE is one
//! it can serve at all ([`custom_endpoint_certificate_refusal`]).
//!
//! `DomainEndpointOptions$CustomEndpoint` targets `DomainNameFqdn`, whose `@pattern`
//! opens every label with the negative lookahead `(?!-)`
//! (`smithy_query(query="shape",shape="DomainNameFqdn")`). The engine the request
//! boundary compiles a `@pattern` with has no look-around, and that boundary treats a
//! pattern it cannot compile as matching (`pattern_matches` in
//! `vendor/smithy-schema-server/src/constraints.rs`), so the syntax the model declares for
//! this member would otherwise never be applied to a request at all. It is applied here
//! instead, by a look-around-capable engine over the MODEL's own expression, so the clause
//! — which quotes that expression — cannot state a requirement this module does not apply.
//! The member's `@length` is left to the boundary, which enforces it already.
//!
//! Being an INPUT constraint it is evaluated BEFORE the request's resources are resolved:
//! a value outside a member's declared syntax is refused by naming that member, never
//! reported as a domain that does not exist — which is a different claim, about a
//! different thing, and sends a caller to fix the wrong half of its request.
//!
//! The WORDS are the service's one home for them (`crate::service::validation`), so a
//! clause this handler-side check states reads like one the boundary states.

use std::sync::LazyLock;

use ::aws_smithy_schema_server::constraints::path;
use ::service_com_amazonaws_acm_certificatemanager::state as acm_store;

use crate::ops::types::{DomainEndpointOptions, ValidationException};

/// The structure the custom endpoint travels in, as the model names it.
const OPTIONS_MEMBER: &str = "DomainEndpointOptions";

/// The member inside it that carries the endpoint, as the model names it.
const ENDPOINT_MEMBER: &str = "CustomEndpoint";

/// The member inside it that carries the certificate the endpoint is served under, as the
/// model names it: "the ARN for your security certificate, managed in Amazon Web Services
/// Certificate Manager (ACM)"
/// (`smithy_query(query="doc",shape="DomainEndpointOptions",member="CustomEndpointCertificateArn")`).
const CERTIFICATE_ARN_MEMBER: &str = "CustomEndpointCertificateArn";

/// The syntax a custom endpoint has to satisfy, as the model declares it for
/// `DomainNameFqdn` (`smithy_query(query="shape",shape="DomainNameFqdn")`): a sequence of
/// dot-separated labels over letters, digits and hyphens, none of them opening with a
/// hyphen or closing with one, and at least one dot — a fully qualified name rather than a
/// single label.
///
/// The one text used BOTH ways: it is the expression the check runs and the expression the
/// clause quotes, so a caller that lifts the expression out of the refusal and
/// pre-validates with it gets the one this service applies.
const CUSTOM_ENDPOINT_PATTERN: &str =
    r"^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$";

/// The step budget one custom-endpoint match may spend.
///
/// The engine below BACKTRACKS, so a match has no linear-time guarantee in general and the
/// work one request may cost is bounded here rather than left to the shape of the
/// expression. Two facts put this budget out of reach in practice, and both are properties
/// of the input rather than of the engine: the expression is a MODEL CONSTANT, never built
/// from a request, and the value matched is `DomainNameFqdn`, whose `@length` maximum of
/// 255 characters the boundary validator enforces before this handler-side check runs. The
/// budget is the backstop for a model expression that is not so tame: a match that
/// exhausts it is answered, not retried, and never spins.
const BACKTRACK_LIMIT: usize = 1_000_000;

/// [`CUSTOM_ENDPOINT_PATTERN`] compiled once, under that budget.
///
/// Compilation is fallible and its input is a compile-time constant, never request text,
/// but an expression that failed to compile is still dropped rather than unwrapped:
/// nothing here panics, on any input.
static CUSTOM_ENDPOINT: LazyLock<Option<::fancy_regex::Regex>> = LazyLock::new(|| {
    ::fancy_regex::RegexBuilder::new(CUSTOM_ENDPOINT_PATTERN)
        .backtrack_limit(BACKTRACK_LIMIT)
        .build()
        .ok()
});

/// `Some(refusal)` when `options` carries a custom endpoint outside the syntax
/// [`CUSTOM_ENDPOINT_PATTERN`] states, and `None` for one that satisfies it — and for a
/// request that names no custom endpoint at all, which has nothing to refuse.
///
/// The member path is the whole dotted path a clause spells for a member inside a
/// structure, so a caller is told which member of which container to fix.
pub fn custom_endpoint_refusal(
    options: Option<&DomainEndpointOptions>,
) -> Option<ValidationException> {
    let endpoint = options?.custom_endpoint.as_deref()?;
    if satisfies_fqdn(endpoint) {
        return None;
    }
    Some(ValidationException {
        message: Some(crate::service::validation::published_pattern_report(
            &path::join(&[
                path::member(OPTIONS_MEMBER),
                path::member(ENDPOINT_MEMBER),
            ]),
            endpoint,
            CUSTOM_ENDPOINT_PATTERN,
        )),
    })
}

/// Whether `endpoint` satisfies the FQDN syntax of its member.
///
/// An expression this crate holds no compiled form for, and a match that exhausts its
/// backtrack budget, are both treated as satisfied — which is what the boundary validator
/// does with a pattern it cannot compile: a value is never refused because of an
/// expression nothing could evaluate.
fn satisfies_fqdn(endpoint: &str) -> bool {
    CUSTOM_ENDPOINT
        .as_ref()
        .is_none_or(|compiled| compiled.is_match(endpoint).unwrap_or(true))
}

/// The sentence a configuration that turns a custom endpoint ON without naming BOTH of the
/// two members it is served by is refused with.
///
/// Entirely fixed text, and the two members are named by the model's own spellings
/// ([`ENDPOINT_MEMBER`], [`CERTIFICATE_ARN_MEMBER`]) so the sentence cannot name a member
/// the request does not carry.
fn incomplete_custom_endpoint_message() -> String {
    format!(
        "Please provide both {ENDPOINT_MEMBER} and {CERTIFICATE_ARN_MEMBER} fields to create a \
         custom endpoint."
    )
}

/// The sentence a configuration naming a certificate the account holds none of is refused
/// with, naming the ARN the request asked for — the one thing about it the service knows,
/// since nothing resolved.
fn certificate_not_found_message(certificate_arn: &str) -> String {
    format!("ACM Certificate {certificate_arn} not found in your account.")
}

/// The sentence a configuration whose certificate does not cover the requested hostname is
/// refused with, naming the certificate the request pointed at and the host it asked that
/// certificate to serve.
fn endpoint_not_covered_message(certificate_arn: &str, custom_endpoint: &str) -> String {
    format!(
        "Certificate {certificate_arn} does not contain the custom domain name \
         {custom_endpoint}."
    )
}

/// A member's value as the request SUPPLIED it, and `None` for one the request left out —
/// a member carrying nothing but blanks naming no value either.
fn supplied(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// Why the custom endpoint `options` asks for cannot be served, or `None` for a
/// configuration the service can provision — and for one that turns no custom endpoint on,
/// which asks for no endpoint to serve.
///
/// A custom endpoint is an HTTPS endpoint the domain answers on under a name of the
/// caller's own, so it is served by exactly two things: the NAME clients reach it by and
/// the certificate that name's TLS handshake is terminated with. Hence the three ways a
/// configuration that enables one cannot be served, each refused with the operation's
/// `ValidationException` (HTTP 400) and nothing stored:
///
/// * it names neither or only one of the two members, so there is no endpoint to serve or
///   nothing to serve it with ([`incomplete_custom_endpoint_message`]);
/// * the certificate it names is one the account holds none of AT THE DOMAIN's Region, so
///   no handshake could be terminated at all ([`certificate_not_found_message`]) — a
///   well-formed ARN naming nothing, one of another Region or of another account among
///   them;
/// * the certificate it names does not COVER the requested host, so every client reaching
///   the endpoint under that name would be presented a certificate for another one
///   ([`endpoint_not_covered_message`]).
///
/// WHICH certificates the account holds, and which names each covers, are Certificate
/// Manager's facts: they are read in that service's own store — the store its
/// `ImportCertificate` and `RequestCertificate` write and its `DescribeCertificate`
/// reports — through the SAME shared engine this service's handlers reach their own store
/// through, the way [`super::encryption_at_rest`] resolves a KMS key and
/// [`super::log_destinations`] a log group. Whether one certificate covers one host is that
/// service's own answer too ([`acm_store::Certificate::covers`]), so this service cannot
/// decide it differently from the service that would present the certificate.
///
/// A certificate is a REGIONAL resource: it answers at one Region's endpoint in one
/// account, and the domain's custom endpoint is terminated with a certificate that service
/// serves at the DOMAIN's own Region — so `region`, the Region the request was signed for
/// and the domain is created in, is the Region the certificate is resolved at, and an ARN
/// of another Region names no certificate this endpoint could be served with.
///
/// Nothing is refused on evidence that is not there: an engine handing out no Certificate
/// Manager store leaves the configuration to be served.
///
/// LOCKING: this takes the Certificate Manager store's guard, and a handler may ask it
/// while holding this service's own. No Certificate Manager operation reads this service's
/// store, so the two can never be taken in opposite orders, and the work done under both is
/// a single lookup.
pub fn custom_endpoint_certificate_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    options: Option<&DomainEndpointOptions>,
) -> Option<ValidationException> {
    let options = options?;
    // "Whether to enable a custom endpoint for the domain": a configuration that enables
    // none asks the service to serve no name of the caller's own, so there is nothing here
    // to refuse.
    if !options.custom_endpoint_enabled.unwrap_or(false) {
        return None;
    }
    let refusal = |message: String| ValidationException {
        message: Some(message),
    };
    let (Some(endpoint), Some(certificate_arn)) = (
        supplied(options.custom_endpoint.as_deref()),
        supplied(options.custom_endpoint_certificate_arn.as_deref()),
    ) else {
        return Some(refusal(incomplete_custom_endpoint_message()));
    };
    let store = acm_store::service_state(engine)?;
    let certificates = store.lock();
    // Resolved at the domain's own Region, so a certificate of another Region is reported
    // as what it is to this endpoint: a certificate the account holds none of here.
    let held = Some(certificate_arn.trim())
        .filter(|arn| acm_store::acm_arn_region(arn) == Some(region))
        .and_then(|arn| certificates.certificates.get(arn));
    let Some(certificate) = held else {
        return Some(refusal(certificate_not_found_message(certificate_arn)));
    };
    if certificate.covers(endpoint) {
        return None;
    }
    Some(refusal(endpoint_not_covered_message(
        certificate_arn,
        endpoint,
    )))
}
