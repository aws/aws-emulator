// SPDX-License-Identifier: CC0-1.0

//! Whether the certificate a distribution's configuration NAMES can serve the alternate
//! domain names that configuration claims.
//!
//! The sibling rule beside this one reads the PAIR of members and nothing else: a
//! configuration that claims an alternate domain name has to name exactly one custom
//! certificate rather than CloudFront's own ([`super::alias_certificate_mismatch`]). That
//! leaves the other half — WHICH certificate it named. An alternate domain name is served
//! only under a certificate "that validates your authorization to use the domain name", so
//! the certificate is a resource the write RESOLVES rather than a string it stores:
//!
//! * `ACMCertificateArn` is "the Amazon Resource Name (ARN) of the ACM certificate", and
//!   "CloudFront only supports ACM certificates in the US East (N. Virginia) Region
//!   (`us-east-1`)" — so an ARN of Certificate Manager naming any other Region names a
//!   certificate CloudFront cannot present at all, which the value itself says; a
//!   certificate that service HOLDS has to be one it has ISSUED, since a `CertificateStatus`
//!   other than that is a certificate with no key material in force — one still being
//!   validated, one whose request failed or timed out, one revoked, expired or inactive —
//!   and nothing can terminate a viewer's TLS under it; and it has to cover the names the
//!   configuration claims, which is Certificate Manager's own answer about its own record
//!   ([`acm_store::Certificate::covers`]).
//! * `IAMCertificateId` is the id of a certificate "stored in Identity and Access
//!   Management". One the account uploaded that SIGNED ITSELF has no authority above it, so
//!   no viewer's trust store can verify it and it travels with no chain that could
//!   ([`iam_store::ServerCertificateRecord::is_self_signed`]) — and it too has to cover the
//!   names claimed ([`iam_store::ServerCertificateRecord::covers`], the same TLS name match
//!   Certificate Manager applies).
//!
//! WHICH certificates the account holds, how each was come by and which names each covers
//! are the facts of the SERVICES that hold them, so they are read in those services' own
//! stores — the one `UploadServerCertificate` writes and `GetServerCertificate` reports, and
//! the one `RequestCertificate`/`ImportCertificate` write and `DescribeCertificate` reports —
//! over the SAME shared engine this service's handlers reach their own store through. This
//! service therefore cannot decide any of it differently from the service that holds the
//! certificate, and a certificate that service no longer holds stops serving new
//! configurations at once.
//!
//! Nothing is refused on evidence that is not there: a value NEITHER certificate-holding
//! service resolves anything for says nothing about the certificate behind it, and leaves the
//! configuration to be stored.
//!
//! LOCKING: this takes the other services' guards one at a time and holds none of them while
//! taking another. Every path that calls it does so BEFORE it locks this service's own store,
//! and no Certificate Manager and no Identity and Access Management operation reads this
//! service's store, so the guards can never be taken in opposite orders.

use ::std::time::SystemTime;

use ::service_com_amazonaws_acm_certificatemanager::state as acm_store;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

use super::*;

/// The sentence CloudFront refuses a configuration whose named certificate cannot serve its
/// alternate domain names with: the whole text is fixed — it enumerates the conditions that
/// put a certificate outside what CloudFront will present rather than naming the one the
/// request hit — so the certificate the configuration named and the names it claimed reach
/// no part of it. Recorded behaviour.
///
/// It is the OTHER sentence of this one code: the pair rule's
/// [`INVALID_VIEWER_CERTIFICATE_MESSAGE`] is what a configuration that names no single custom
/// certificate at all is told, and this one what a configuration that named one is told about
/// the certificate it named.
pub const UNUSABLE_VIEWER_CERTIFICATE_MESSAGE: &str =
    "The specified SSL certificate doesn't exist, isn't in us-east-1 region, isn't valid, or \
     doesn't include a valid certificate chain.";

/// The one Region CloudFront presents a Certificate Manager certificate from:
/// `ViewerCertificate$ACMCertificateArn` — "CloudFront only supports ACM certificates in the
/// US East (N. Virginia) Region (`us-east-1`)". The SAME Region this service mints a managed
/// certificate's own ARN in ([`CERTIFICATE_REGION`]), so the two cannot drift apart.
pub const VIEWER_CERTIFICATE_REGION: &str = CERTIFICATE_REGION;

/// The Certificate Manager service namespace an `ACMCertificateArn` belongs to.
const ACM_VENDOR: &str = "acm";

/// `Some(message)` when the certificate `config` names cannot serve the alternate domain
/// names `config` claims, and `None` when it can — or when the configuration claims no
/// alternate domain name, or names a certificate neither certificate-holding service
/// resolves anything for.
///
/// The rule is stated in this module's own documentation; `engine` is the shared engine the
/// calling handler holds, which is how the holding services' records are read, and `now` is
/// the instant the write is judged at — what a certificate's own status is depends on it.
pub fn unusable_viewer_certificate<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    config: &DistributionConfig,
    now: SystemTime,
) -> Option<&'static str> {
    let aliases: &[String] = config
        .aliases
        .as_ref()
        .and_then(|aliases| aliases.items.as_deref())
        .unwrap_or_default();
    if aliases.is_empty() {
        return None;
    }
    let certificate = config.viewer_certificate.as_ref()?;
    if let Some(arn) = named_member(certificate.acm_certificate_arn.as_ref()) {
        if managed_certificate_unusable(engine, aliases, arn, now) {
            return Some(UNUSABLE_VIEWER_CERTIFICATE_MESSAGE);
        }
    }
    if let Some(id) = named_member(certificate.iam_certificate_id.as_ref()) {
        if server_certificate_unusable(engine, aliases, id) {
            return Some(UNUSABLE_VIEWER_CERTIFICATE_MESSAGE);
        }
    }
    None
}

/// The value a certificate member NAMES, or `None` for one left empty — the same reading
/// [`super::custom_certificate_members`] makes of the pair, so a member that counts as naming
/// no certificate there is not resolved here either.
fn named_member(value: Option<&String>) -> Option<&str> {
    value.map(|value| value.trim()).filter(|value| !value.is_empty())
}

/// Whether the Certificate Manager certificate `certificate_arn` names cannot serve every one
/// of `aliases`.
///
/// Three things about it are read, in the order of what the value alone says and then what the
/// holding service says: the Region the ARN names has to be the one CloudFront presents a
/// certificate from ([`VIEWER_CERTIFICATE_REGION`]), a certificate that service holds has to
/// be one it reports as ISSUED, and it has to cover every name claimed. An ARN of no
/// Certificate Manager resource at all, and one that service holds nothing under, are evidence
/// of nothing and report `false`.
///
/// WHETHER a certificate is in force is that service's `CertificateDetail$Status` and nothing
/// derived here: it is the one member that says whether key material exists for the
/// certificate, so a validation still running, a request that failed or timed out and a
/// revoked, expired or inactive certificate are all read off it rather than re-decided from
/// the material. The holder's own reads materialise what the clock owes its records before
/// reporting a status ([`acm_store::ComAmazonawsAcmCertificatemanagerState::materialize_due`]),
/// so that pass is made here too and this read sees the record `DescribeCertificate` would
/// report at `now` — a validation that has already fallen due has issued the certificate.
fn managed_certificate_unusable<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    aliases: &[String],
    certificate_arn: &str,
    now: SystemTime,
) -> bool {
    if let Some(region) = ::service_common::arn::region_of(certificate_arn, ACM_VENDOR) {
        if region != VIEWER_CERTIFICATE_REGION {
            return true;
        }
    }
    let Some(store) = acm_store::service_state(engine) else {
        return false;
    };
    let mut certificates = store.lock();
    certificates.materialize_due(now);
    let Some(certificate) = certificates.certificates.get(certificate_arn) else {
        return false;
    };
    certificate.status != acm_store::STATUS_ISSUED
        || !aliases.iter().all(|alias| certificate.covers(alias))
}

/// Whether the certificate the account holds in Identity and Access Management under the id
/// `certificate_id` cannot serve every one of `aliases`: it signed itself, so nothing stands
/// above it to verify it, or it does not cover a name claimed.
///
/// The record is found by the id that service minted for it
/// (`ServerCertificateMetadata$ServerCertificateId`), which is the value
/// `ViewerCertificate$IAMCertificateId` names a certificate by. An id that service holds
/// nothing under is evidence of nothing and reports `false`.
fn server_certificate_unusable<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    aliases: &[String],
    certificate_id: &str,
) -> bool {
    let Some(store) = iam_store::service_state(engine) else {
        return false;
    };
    let entities = store.lock();
    let Some(held) = entities
        .server_certificates
        .values()
        .find(|certificate| certificate.metadata.server_certificate_id == certificate_id)
    else {
        return false;
    };
    held.is_self_signed() || !aliases.iter().all(|alias| held.covers(alias))
}
