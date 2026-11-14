// SPDX-License-Identifier: CC0-1.0

//! Whether the certificate a custom domain is created with can serve the NAME that domain
//! is created under.
//!
//! A custom domain answers HTTPS on the name the create states, and the only thing that
//! handshake can be terminated with is the certificate the same create names — so the two
//! members are not independent: a certificate whose subject name and subject alternative
//! names do not include the requested name would present every client a certificate for
//! some other host. `CreateDomainName` therefore resolves the certificate and refuses the
//! PAIR, with the `BadRequestException` (HTTP 400) it models, before anything is stored.
//!
//! WHICH certificates the account holds, and which names each one covers, are Certificate
//! Manager's facts: they are read in that service's own store — the store its
//! `ImportCertificate` and `RequestCertificate` write and its `DescribeCertificate`
//! reports — through the SAME shared engine this service's handlers reach their own store
//! through. Whether one certificate covers one host is that service's own answer as well
//! ([`acm_store::Certificate::covers`]), so this service cannot decide it differently from
//! the service that holds the certificate.
//!
//! Nothing is refused on evidence that is not there: an engine handing out no Certificate
//! Manager store, and an ARN that store resolves nothing for (an Identity and Access
//! Management server certificate, which `CertificateArn` also admits, among them), leave
//! the pair to be created — whether a certificate Certificate Manager never issued is
//! trusted at all is a different question, about the certificate rather than about the
//! name it is being asked to serve.
//!
//! LOCKING: this takes the Certificate Manager store's guard, and the handler may ask it
//! while holding this service's own. No Certificate Manager operation reads this service's
//! store, so the two can never be taken in opposite orders, and the work done under both
//! is a single lookup.

use ::service_com_amazonaws_acm_certificatemanager::state as acm_store;

/// The sentence a `BadRequestException` states a certificate that cannot serve the
/// requested custom domain with.
///
/// Recorded behavior, and entirely FIXED text: AppSync names neither the domain nor the
/// certificate it refused, so nothing of the request is interpolated into it.
pub const CERTIFICATE_INVALID_MESSAGE: &str = "Certificate is invalid.";

/// `Some(message)` when the certificate `certificate_arn` names cannot serve
/// `domain_name`, and `None` when it can — or when nothing in the account resolves for
/// that ARN, which says nothing about the name.
pub fn certificate_coverage_refusal<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    domain_name: &str,
    certificate_arn: &str,
) -> Option<String> {
    let store = acm_store::service_state(engine)?;
    let certificates = store.lock();
    let certificate = certificates.certificates.get(certificate_arn.trim())?;
    if certificate.covers(domain_name) {
        return None;
    }
    Some(CERTIFICATE_INVALID_MESSAGE.to_string())
}
