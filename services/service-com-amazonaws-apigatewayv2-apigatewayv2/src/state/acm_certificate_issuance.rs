// SPDX-License-Identifier: CC0-1.0

//! How the certificate a custom domain name PRESENTS came to be in Certificate Manager,
//! for the one precondition of this service that turns on it: mutual TLS.
//!
//! `DomainNameConfiguration$OwnershipVerificationCertificateArn` is "the ARN of the public
//! certificate issued by ACM to validate ownership of your custom domain. Only required
//! when configuring mutual TLS and using an ACM imported or private CA certificate ARN as
//! the regional certificate". So whether a domain name may be configured for mutual TLS
//! without one depends on HOW the certificate it presents was issued — and that is not a
//! fact this service holds: a certificate is Certificate Manager's resource, and
//! `acm:DescribeCertificate` reports its `Type`.
//!
//! It is therefore ASKED there, the way any emulated service reaches another — through the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), over the
//! client in [`super::network`] — so this crate names no other service's crate, no socket
//! is opened and nothing leaves the process. Nothing is cached: a certificate imported a
//! moment ago is weighed at once.
//!
//! A hop that could not be made at all establishes nothing, so silence refuses nothing and
//! says so at `warn` — the reading [`super::ec2_subnets`] gives the same case — and the
//! refusal that IS served is the one the owner's answer grounds.

use super::network::acm;

/// The sentence a request that would configure mutual TLS on an endpoint presenting a
/// client-supplied certificate, without naming the certificate that validates ownership of
/// the domain, is refused with.
///
/// Fixed text: it names the missing member in its wire spelling and the two kinds of
/// certificate it is required for, and carries no value from the request.
pub const MISSING_OWNERSHIP_VERIFICATION_CERTIFICATE: &str =
    "Missing ownershipVerificationCertificate. To use an imported or private certificate \
     for the domain name, ownershipVerificationCertificate is required.";

/// How many DISTINCT certificates one request's endpoints are resolved for.
///
/// One hop per certificate, so the work a single call costs is capped by a constant rather
/// than by how many configurations the caller sends. It is the shared ceiling on how many
/// certificates ONE resource can reference at all
/// ([`::service_common::acm_certificate_associations::MAX_CERTIFICATES_PER_RESOURCE`]), so
/// no configuration a domain name can actually hold goes unresolved.
const MAX_CERTIFICATES_RESOLVED: usize =
    ::service_common::acm_certificate_associations::MAX_CERTIFICATES_PER_RESOURCE;

/// Refuse a request that would leave a custom domain name configured for mutual TLS on an
/// endpoint whose certificate needs an ownership-verification certificate it does not name.
///
/// `configurations` and `mutual_tls` are the state the request would LEAVE BEHIND — for a
/// patch, the members it carries over the ones it leaves stored — so the precondition is
/// weighed against the resource as it would stand rather than against one call's members.
/// `Ok(())` for a request that configures no mutual TLS at all, for one whose every
/// endpoint either names an ownership-verification certificate or presents a certificate
/// Certificate Manager itself issued, and for one the owner could not be asked about (see
/// this module's documentation). `Err` carries the message for the operation's
/// `BadRequestException`.
pub async fn resolve_mutual_tls_ownership(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configurations: &[super::DomainNameEndpoint],
    mutual_tls: Option<&crate::ops::types::MutualTlsAuthenticationInput>,
) -> Result<(), String> {
    if !configures_mutual_tls(mutual_tls) {
        return Ok(());
    }
    let mut asked: Vec<&str> = Vec::new();
    for endpoint in configurations {
        // An endpoint that names the certificate validating ownership of the domain has
        // met the precondition, whatever its own certificate is.
        if present(endpoint.ownership_verification_certificate_arn.as_deref()).is_some() {
            continue;
        }
        let Some(certificate_arn) = present(endpoint.certificate_arn.as_deref()) else {
            continue;
        };
        if asked.contains(&certificate_arn) {
            continue;
        }
        if asked.len() >= MAX_CERTIFICATES_RESOLVED {
            // At `warn`, for the same reason silence is logged below: past the ceiling this
            // call weighs nothing more, so a request carrying more distinct certificates
            // than one resource can reference is accepted on an unresolved tail.
            ::tracing::warn!(
                asked = asked.len(),
                "more distinct certificates were named than one resource can reference, so \
                 the rest were not weighed"
            );
            break;
        }
        asked.push(certificate_arn);
        if ownership_verification_is_required(ctx, region, certificate_arn).await {
            return Err(MISSING_OWNERSHIP_VERIFICATION_CERTIFICATE.to_string());
        }
    }
    Ok(())
}

/// Whether this request configures mutual TLS: `MutualTlsAuthentication` names the
/// truststore "clients must present a trusted certificate from" (`$TruststoreUri`), so a
/// structure carrying no truststore configures no mutual TLS.
fn configures_mutual_tls(
    mutual_tls: Option<&crate::ops::types::MutualTlsAuthenticationInput>,
) -> bool {
    mutual_tls.is_some_and(|authentication| present(authentication.truststore_uri.as_deref()).is_some())
}

/// A member's value when it carries one at all: `None` for an absent or blank value, which
/// names no certificate and no truststore.
fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Whether a domain name presenting the certificate under `certificate_arn` needs an
/// ownership-verification certificate to be configured for mutual TLS: whether Certificate
/// Manager reports it as one the client supplied rather than one ACM issued.
///
/// `false` for a certificate the owner holds none of, and for a hop that could not be made:
/// neither establishes that the certificate is client-supplied.
async fn ownership_verification_is_required(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    certificate_arn: &str,
) -> bool {
    let Some(client) = acm(ctx, region) else {
        ::tracing::warn!(
            "the request names no Region label, so the certificate manager cannot be asked"
        );
        return false;
    };
    match client
        .describe_certificate()
        .certificate_arn(certificate_arn)
        .send()
        .await
    {
        Ok(answer) => answer
            .certificate()
            .and_then(|certificate| certificate.r#type())
            .is_some_and(is_client_supplied),
        Err(error) => {
            // At `warn`: this precondition refuses nothing on silence, so a deployment
            // where the hop fails accepts a mutual-TLS configuration AWS would refuse, and
            // that is worth seeing.
            ::tracing::warn!(
                certificate = %certificate_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the certificate manager did not answer how the certificate was issued"
            );
            false
        }
    }
}

/// Whether a `CertificateType` is one of the two the ownership-verification certificate is
/// required for: a certificate the client brought into ACM itself (`IMPORTED`) and one its
/// own private CA issued (`PRIVATE`) — the pair
/// `DomainNameConfiguration$OwnershipVerificationCertificateArn` names. A certificate ACM
/// issued and renews for a domain it validated (`AMAZON_ISSUED`) needs none, and so does a
/// value outside the catalog, which establishes nothing.
fn is_client_supplied(certificate_type: &::aws_sdk_acm::types::CertificateType) -> bool {
    use ::aws_sdk_acm::types::CertificateType;

    matches!(
        certificate_type,
        CertificateType::Imported | CertificateType::Private
    )
}

#[cfg(test)]
mod tests {
    use super::{configures_mutual_tls, is_client_supplied, present};
    use ::aws_sdk_acm::types::CertificateType;

    /// A truststore is what makes a `MutualTlsAuthentication` a configuration of mutual
    /// TLS, so only a structure carrying one configures it.
    #[test]
    fn only_a_truststore_configures_mutual_tls() {
        // (case, the truststore uri the request carried, configures mutual TLS?)
        let rows: Vec<(&str, Option<Option<&str>>, bool)> = vec![
            ("no mutualTlsAuthentication at all", None, false),
            ("a structure carrying no truststore", Some(None), false),
            ("a blank truststore uri", Some(Some("   ")), false),
            (
                "a truststore object",
                Some(Some("s3://truststore/bundle.pem")),
                true,
            ),
        ];
        for (case, truststore, expected) in rows {
            let authentication =
                truststore.map(|uri| crate::ops::types::MutualTlsAuthenticationInput {
                    truststore_uri: uri.map(str::to_string),
                    truststore_version: None,
                });
            let configured = configures_mutual_tls(authentication.as_ref());
            assert_eq!(
                configured, expected,
                "case {case:?}: expected configures_mutual_tls={expected}, got {configured}"
            );
        }
    }

    /// Only the two kinds of certificate the member documents need one.
    #[test]
    fn an_ownership_verification_certificate_is_required_for_client_supplied_material() {
        // (the type the certificate manager reported, requires the ownership certificate?)
        let rows: Vec<(CertificateType, bool)> = vec![
            (CertificateType::Imported, true),
            (CertificateType::Private, true),
            (CertificateType::AmazonIssued, false),
            (CertificateType::from("SOMETHING_ELSE"), false),
        ];
        for (certificate_type, expected) in rows {
            let required = is_client_supplied(&certificate_type);
            assert_eq!(
                required, expected,
                "case {certificate_type:?}: expected required={expected}, got {required}"
            );
        }
    }

    /// A blank member names nothing, so it is read as absent.
    #[test]
    fn a_blank_member_names_nothing() {
        // (the member's value, what it names)
        let rows: Vec<(Option<&str>, Option<&str>)> = vec![
            (None, None),
            (Some(""), None),
            (Some("  \t "), None),
            (Some(" arn:aws:acm:us-west-2:000000000000:certificate/one "), Some("arn:aws:acm:us-west-2:000000000000:certificate/one")),
        ];
        for (value, expected) in rows {
            let named = present(value);
            assert_eq!(
                named, expected,
                "case {value:?}: expected {expected:?}, got {named:?}"
            );
        }
    }
}
