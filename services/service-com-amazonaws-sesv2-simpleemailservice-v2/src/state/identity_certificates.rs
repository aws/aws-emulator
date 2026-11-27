// SPDX-License-Identifier: CC0-1.0

//! The S/MIME certificate an email identity's association NAMES, resolved against the
//! service that HOLDS it.
//!
//! `AssociateEmailIdentityCertificate` takes the ARN of a Certificate Manager
//! certificate, and SES then "prepares the certificate for use": whether that
//! preparation can succeed is a fact about the CERTIFICATE, which lives in Certificate
//! Manager's own store rather than in this one. So it is read from that store, in this
//! one shared engine, rather than from a copy here — the engine keys state by TYPE, so a
//! copy of its own here would be invisible to the holder and a certificate the holder has
//! would address nothing.
//!
//! LOCK ORDER: take [`super::service_state`] first and this one second, which is the
//! order every other reader of another service's store takes.
//!
//! Split out of the state module root so this one concern lives in its own file; every
//! item is re-exported from [`crate::state`], so a caller still reaches it as
//! `crate::state::…`.

use super::*;

/// Certificate Manager's OWN record of this account's certificates, in the shared
/// engine: call it with a handler's `&*self.engine`.
///
/// `None` means the engine is not the shared in-memory engine, so there is no holder to
/// ask; a handler answers that the way it answers a missing store of its own.
pub fn acm_certificates<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState,
    >,
> {
    ::service_com_amazonaws_acm_certificatemanager::state::service_state(engine)
}

/// Whether the certificate `certificate_arn` names can be used to SIGN a message at
/// `now`, as the service that holds that certificate says.
///
/// Four things have to hold of the held record, and each is read off it rather than
/// assumed:
///
/// * the holder has a certificate under that ARN at all — an ARN it has none under names
///   nothing to sign with;
/// * that certificate has been ISSUED and its validity has not run out — a request still
///   being validated, one that failed and one that has expired all have no key material
///   in force;
/// * its X.509 Extended Key Usage extension PERMITS email protection — RFC 5280 section
///   4.2.1.12's `emailProtection` purpose is the one "protection of email" is asserted
///   by, and a certificate whose extension names other purposes may not be used for it,
///   while one that carries no such extension is restricted to no purpose and so may
///   ([`certificate_permits_email_protection`]);
/// * a certificate AUTHORITY issued it, which the holder answers off the certificate's
///   own issuer and subject fields
///   ([`::service_com_amazonaws_acm_certificatemanager::state::Certificate::is_self_issued`]).
///   A self-issued certificate is signed by the very key it certifies, so the signature
///   it would put on a message is vouched for by nothing a recipient can check: there is
///   no authority at the end of its chain, and no signing certificate to prepare out of
///   it.
///
/// Together they are exactly the condition the modelled `FAILED` association status
/// covers: "the certificate couldn't be prepared for use, or the certificate has
/// expired".
///
/// `false` for an engine that has no Certificate Manager store at all, since nothing
/// then vouches for the certificate either.
pub fn certificate_can_sign<Eng: Send + Sync + 'static>(
    engine: &Eng,
    certificate_arn: &str,
    now: SystemTime,
) -> bool {
    let Some(certificates) = acm_certificates(engine) else {
        return false;
    };
    let mut held = certificates.lock();
    // The holder's own reads materialise what the clock owes its records first — a
    // validation that has already fallen due has issued the certificate — so this read
    // sees the same record `DescribeCertificate` would report.
    held.materialize_due(now);
    held.certificates
        .get(certificate_arn)
        .is_some_and(|certificate| {
            certificate.status
                == ::service_com_amazonaws_acm_certificatemanager::state::STATUS_ISSUED
                && certificate.not_after.is_none_or(|not_after| now < not_after)
                && certificate_permits_email_protection(certificate)
                && !certificate.is_self_issued()
        })
}

/// Whether `certificate`'s key may be used to protect email, read off the key purposes
/// the holder parsed out of its Extended Key Usage extension.
///
/// A certificate asserts the purposes its extension lists: `emailProtection` is the one
/// that covers signing a message, and the `anyExtendedKeyUsage` purpose covers every
/// one. A certificate carrying NO such extension restricts its key to no purpose at all,
/// so every purpose is open to it — the holder reports that absence as its own value
/// (`ExtendedKeyUsageName$NONE`). A certificate asserting only the TLS purposes a public
/// certificate authority issues for — server and client authentication — may not be used
/// to sign email, so an association naming one cannot be prepared.
fn certificate_permits_email_protection(
    certificate: &::service_com_amazonaws_acm_certificatemanager::state::Certificate,
) -> bool {
    use ::service_com_amazonaws_acm_certificatemanager::state as acm;
    certificate.extended_key_usages.iter().any(|purpose| {
        matches!(
            purpose.name.as_str(),
            EMAIL_PROTECTION_KEY_PURPOSE | ANY_KEY_PURPOSE
        ) || purpose.name == acm::EXTENDED_KEY_USAGE_NONE
    })
}

/// `ExtendedKeyUsageName`'s value for RFC 5280 section 4.2.1.12's `emailProtection` key
/// purpose (`1.3.6.1.5.5.7.3.4`), the purpose an S/MIME signing certificate asserts.
const EMAIL_PROTECTION_KEY_PURPOSE: &str = "EMAIL_PROTECTION";

/// `ExtendedKeyUsageName`'s value for `anyExtendedKeyUsage` (`2.5.29.37.0`), the purpose
/// that stands for every other one.
const ANY_KEY_PURPOSE: &str = "ANY";
