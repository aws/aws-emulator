// SPDX-License-Identifier: CC0-1.0

//! Amazon N2 certificates: the ROOT HIERARCHY ACM issues one from, and the Extended
//! Key Usage profile it carries.
//!
//! "If you omit this parameter, ACM issues a standard public certificate"
//! (`RequestCertificateRequest$RootHierarchyType` doc), so the root hierarchy a
//! request named is what makes the certificate it creates an Amazon N2 one — and the
//! two members such a certificate reports "exist only for Amazon N2 certificates"
//! (`CertificateDetail$RootHierarchyType` / `$CertificateUsage` docs), which is why
//! each is stored as an option on the record and reported exactly where the record
//! carries it.
//!
//! Everything a root hierarchy decides about the certificate lives here: the
//! parameters a request may combine with it, the `CertificateUsage` the certificate
//! ends up with, the validity window its issuance opens, and the key purposes its
//! Extended Key Usage extension asserts.

use super::*;

/// `RootHierarchyType` of "an Amazon N2 certificate that chains to an Amazon N2 root"
/// (`RootHierarchyType` doc).
pub const ROOT_HIERARCHY_TYPE_N2_AMAZON_MANAGED: &str = "N2_AMAZON_MANAGED";

/// `RootHierarchyType` of "an Amazon N2 certificate that chains to an Amazon N2 root
/// cross-signed by Starfield G2" (`RootHierarchyType` doc).
pub const ROOT_HIERARCHY_TYPE_N2_SFG2_AMAZON_MANAGED: &str = "N2_SFG2_AMAZON_MANAGED";

/// `CertificateUsage` of an Amazon N2 certificate whose request named none: "Default:
/// CLIENT_AND_SERVER_AUTH" (`RequestCertificateRequest$CertificateUsage` doc), which
/// "authenticates both the client and the server" and is "valid for both N2 root
/// hierarchy types".
pub const DEFAULT_CERTIFICATE_USAGE: &str = "CLIENT_AND_SERVER_AUTH";

/// `CertificateUsage` that "authenticates only the client" and is "valid only for
/// N2_AMAZON_MANAGED" (`RequestCertificateRequest$CertificateUsage` doc).
pub const CERTIFICATE_USAGE_CLIENT_AUTH_ONLY: &str = "CLIENT_AUTH_ONLY";

/// The `KeyAlgorithm` values an Amazon N2 certificate request may name: "algorithms
/// supported for an N2 certificate request include: RSA_2048, EC_prime256v1,
/// EC_secp384r1" (`RequestCertificateRequest$RootHierarchyType` doc). Read the sentence
/// verbatim with:
///
/// ```text
/// smithy_query(query="doc", shape="com.amazonaws.acm#RequestCertificateRequest",
///     member="RootHierarchyType",
///     model_path="services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json")
/// ```
pub const N2_KEY_ALGORITHMS: &[&str] = &["RSA_2048", "EC_prime256v1", "EC_secp384r1"];

/// The one `ValidationMethod` an Amazon N2 certificate request may name: "an Amazon N2
/// certificate request must specify a ValidationMethod of DNS"
/// (`RequestCertificateRequest$RootHierarchyType` doc).
pub const N2_VALIDATION_METHOD: &str = VALIDATION_METHOD_DNS;

/// How long an Amazon N2 certificate that chains to an Amazon N2 root cross-signed by
/// Starfield G2 is valid for.
///
/// A DOCUMENTED span, not a chosen one: for `N2_SFG2_AMAZON_MANAGED` "the maximum
/// validity period, which is also the default, is 98 days"
/// (`RequestCertificateRequest$RootHierarchyType` doc). The other root hierarchy's
/// documented maximum and default is 395 days, which is the span every other
/// certificate ACM issues here carries ([`CERTIFICATE_VALIDITY`]).
pub const N2_SFG2_CERTIFICATE_VALIDITY: Duration = Duration::from_secs(98 * 24 * 60 * 60);

/// The key purposes the Extended Key Usage extension of a certificate whose
/// `CertificateUsage` is [`CERTIFICATE_USAGE_CLIENT_AUTH_ONLY`] asserts: client
/// authentication alone, since that profile "authenticates only the client"
/// (`RequestCertificateRequest$CertificateUsage` doc).
pub const CLIENT_AUTH_ONLY_EXTENDED_KEY_USAGE_OIDS: &[&str] =
    &[EXTENDED_KEY_USAGE_OID_CLIENT_AUTH];

impl Certificate {
    /// Whether this record is an Amazon N2 certificate: one ACM issued from an N2 root
    /// hierarchy, which is what a request naming a `RootHierarchyType` asks for — and
    /// so the condition under which the members that "exist only for Amazon N2
    /// certificates" are reported at all.
    pub fn is_amazon_n2(&self) -> bool {
        self.root_hierarchy_type.is_some()
    }

    /// The span between the `NotBefore` and the `NotAfter` of this certificate once ACM
    /// issues it, which the root hierarchy it is issued from decides: 98 days for a
    /// certificate chaining to the cross-signed Amazon N2 root
    /// ([`N2_SFG2_CERTIFICATE_VALIDITY`]) and the 395 days every other certificate ACM
    /// issues carries ([`CERTIFICATE_VALIDITY`]).
    pub(super) fn issued_validity(&self) -> Duration {
        if self.root_hierarchy_type.as_deref() == Some(ROOT_HIERARCHY_TYPE_N2_SFG2_AMAZON_MANAGED) {
            N2_SFG2_CERTIFICATE_VALIDITY
        } else {
            CERTIFICATE_VALIDITY
        }
    }

    /// The key purposes this certificate's Extended Key Usage extension asserts once
    /// ACM has issued it.
    ///
    /// `CertificateUsage` IS "the Extended Key Usage profile of the certificate"
    /// (`CertificateDetail$CertificateUsage` doc), so a certificate whose profile
    /// authenticates only the client asserts client authentication alone; every other
    /// certificate this service issues is a TLS server certificate and asserts both
    /// purposes ([`ISSUED_EXTENDED_KEY_USAGE_OIDS`]).
    pub(super) fn issued_extended_key_usage_oids(&self) -> &'static [&'static str] {
        if self.certificate_usage.as_deref() == Some(CERTIFICATE_USAGE_CLIENT_AUTH_ONLY) {
            CLIENT_AUTH_ONLY_EXTENDED_KEY_USAGE_OIDS
        } else {
            ISSUED_EXTENDED_KEY_USAGE_OIDS
        }
    }
}

/// The `CertificateUsage` the certificate a request creates carries: the profile the
/// request named, the documented default for one that named none, and NOTHING for a
/// request that asked for no Amazon N2 root hierarchy — "this value exists only for
/// Amazon N2 certificates" (`CertificateDetail$CertificateUsage` doc).
pub fn requested_certificate_usage(
    root_hierarchy_type: Option<&str>,
    certificate_usage: Option<&str>,
) -> Option<String> {
    root_hierarchy_type.map(|_| {
        certificate_usage
            .unwrap_or(DEFAULT_CERTIFICATE_USAGE)
            .to_string()
    })
}

/// The sentence that refuses a certificate request whose Amazon N2 parameters are
/// outside the rules those parameters are documented under, or `None` when the request
/// satisfies every one of them.
///
/// Four rules, each the member's own sentence, in the order a reader of the request
/// meets them:
///
/// * "You cannot specify RootHierarchyType together with CertificateAuthorityArn"
///   (`RequestCertificateRequest$RootHierarchyType` doc) — a private authority's
///   certificate chains to that authority, not to an Amazon N2 root.
/// * "You can specify [CertificateUsage] only when RootHierarchyType is
///   N2_AMAZON_MANAGED or N2_SFG2_AMAZON_MANAGED. If you specify CertificateUsage
///   with any other root hierarchy type, or without RootHierarchyType, the request
///   fails" (`$CertificateUsage` doc): the profile belongs to an Amazon N2 certificate,
///   so there is nothing for it to be the profile of otherwise.
/// * `CLIENT_AUTH_ONLY` is "valid only for N2_AMAZON_MANAGED" (`$CertificateUsage`
///   doc).
/// * "An Amazon N2 certificate request must specify a ValidationMethod of DNS", and the
///   algorithms "supported for an N2 certificate request" are
///   [`N2_KEY_ALGORITHMS`] (`$RootHierarchyType` doc). Both are read against the
///   values the request RESOLVED to — the method and algorithm the certificate would be
///   created with, defaults included — so a request that reaches a value by omission is
///   held to the same rule as one that named it.
///
/// Every sentence is about the rule rather than about one request: it names the
/// members and the values the model declares, and interpolates nothing the caller sent.
pub fn amazon_n2_request_refusal(
    root_hierarchy_type: Option<&str>,
    certificate_usage: Option<&str>,
    certificate_authority_arn: Option<&str>,
    validation_method: &str,
    key_algorithm: &str,
) -> Option<String> {
    if root_hierarchy_type.is_some() && certificate_authority_arn.is_some() {
        return Some(
            "You cannot specify RootHierarchyType together with CertificateAuthorityArn."
                .to_string(),
        );
    }
    let Some(root_hierarchy_type) = root_hierarchy_type else {
        return certificate_usage.map(|_| {
            format!(
                "You can specify CertificateUsage only when RootHierarchyType is \
                 {ROOT_HIERARCHY_TYPE_N2_AMAZON_MANAGED} or \
                 {ROOT_HIERARCHY_TYPE_N2_SFG2_AMAZON_MANAGED}."
            )
        });
    };
    if certificate_usage == Some(CERTIFICATE_USAGE_CLIENT_AUTH_ONLY)
        && root_hierarchy_type != ROOT_HIERARCHY_TYPE_N2_AMAZON_MANAGED
    {
        return Some(format!(
            "A CertificateUsage of {CERTIFICATE_USAGE_CLIENT_AUTH_ONLY} is valid only for a \
             RootHierarchyType of {ROOT_HIERARCHY_TYPE_N2_AMAZON_MANAGED}."
        ));
    }
    if validation_method != N2_VALIDATION_METHOD {
        return Some(format!(
            "An Amazon N2 certificate request must specify a ValidationMethod of \
             {N2_VALIDATION_METHOD}."
        ));
    }
    if !N2_KEY_ALGORITHMS.contains(&key_algorithm) {
        return Some(format!(
            "An Amazon N2 certificate request must specify a KeyAlgorithm of {}.",
            N2_KEY_ALGORITHMS.join(", ")
        ));
    }
    None
}
