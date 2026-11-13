// SPDX-License-Identifier: CC0-1.0

//! The X.509 material a cluster's certificate authorities are made of.
//!
//! `Certificate$data` is "the Base64-encoded certificate data required to
//! communicate with your cluster. Add this to the `certificate-authority-data`
//! section of the kubeconfig file for your cluster"
//! (`smithy_query(query="shape", shape="Certificate")`), and a
//! `CertificateAuthority$data` is the same thing for one certificate authority of
//! the cluster — so what the member carries has to be a certificate a client can
//! decode, parse and install as a trust anchor, not text in the shape of one.
//! Every client built from a cluster's own kubeconfig reads it that way.
//!
//! A child module of [`super`] rather than more of `state.rs`: issuing material
//! is one concern, and it is the only part of this service that holds a private
//! key at all — the key is generated here, used to sign the certificate, and
//! dropped. Nothing stores or reports it: no shape of this service has a member
//! for a private key.

use super::*;

/// The subject common name a cluster's certificate authority is issued under.
///
/// Kubernetes names a cluster's own root of trust `kubernetes`, which is the
/// subject every client that inspects the CA of a cluster sees — so a certificate
/// authority of this service is issued under the same name rather than under a
/// name this emulator invents.
pub const CERTIFICATE_AUTHORITY_COMMON_NAME: &str = "kubernetes";

/// A certificate authority's `data`: the Base64 of the PEM-encoded X.509
/// certificate it signs with, over the validity period the record reports.
///
/// A REAL self-signed certificate authority certificate, issued through the
/// maintained generator this tree already uses for certificate material: it is
/// marked as a CA and carries the key usages a CA signs certificates and
/// revocation lists with, so a client that Base64-decodes the member gets a PEM
/// block it can parse and install as a trust anchor — which is what a kubeconfig
/// built from `DescribeCluster` does with it.
///
/// The private half is generated here and dropped: the cluster's control plane is
/// what would sign with it, and no shape of this service declares a member for a
/// private key, so nothing this service stores or reports can present it. The one
/// certificate this service DOES have to present — the Kubernetes API server's,
/// on the address the cluster publishes as its `endpoint` — is issued in the same
/// breath as the authority that signs it ([`provision_cluster_authority`]), while
/// that key is still in hand.
///
/// `not_before`/`not_after` are the record's own validity period
/// ([`CertificateAuthorityRecord::not_before`]), so the window the certificate
/// bears and the one `DescribeCertificateAuthority` reports are one value. The
/// material is issued ONCE, when the certificate authority is created, and stored
/// on the record; no read re-issues it, so every read reports the same
/// certificate.
///
/// `None` when the material does not issue, which the operation that asked for it
/// reports rather than storing a certificate authority with nothing in it.
pub fn mint_certificate_authority_data(
    not_before: SystemTime,
    not_after: SystemTime,
) -> Option<String> {
    match issue_certificate_authority(not_before, not_after) {
        Some((_issuer, pem)) => Some(::aws_smithy_types::base64::encode(pem)),
        None => {
            // The caller answers its modelled server error for this, which says
            // nothing about WHY: the generator's own failure is worth a line so
            // the path is diagnosable if it ever fires.
            ::tracing::warn!("the certificate authority's certificate could not be issued");
            None
        }
    }
}

/// The certificate authority a cluster is provisioned with, together with the TLS
/// identity its Kubernetes API server serves under.
///
/// The pair is what makes the cluster USABLE as the API documents it: a client
/// "adds [`certificateAuthority.data`] to the certificate-authority-data section
/// of the kubeconfig file for your cluster" (`Certificate$data`) and then reaches
/// "the endpoint for your Kubernetes API server" (`Cluster$endpoint`) over TLS —
/// so the certificate presented at that endpoint has to be one the published
/// authority signed, or the documented thing a client does with the two members
/// cannot work.
#[derive(Debug)]
pub struct ProvisionedAuthority {
    /// The authority's own `data`, as the member publishes it: Base64 of its
    /// PEM-encoded certificate.
    pub data: String,
    /// The identity the cluster's API server answers a handshake with.
    pub api_server: ApiServerIdentity,
}

/// The TLS identity a cluster's Kubernetes API server presents: a leaf
/// certificate the cluster's certificate authority issued for the names that
/// endpoint is dialled by, the authority's own certificate behind it, and the
/// leaf's private key.
///
/// The key is held in memory for as long as the cluster's listener is open and
/// goes nowhere else: it is written to no file, carried in no response, and —
/// since the fields are private and the rendering below prints none of them —
/// reaches no log.
pub struct ApiServerIdentity {
    certificate_der: Vec<u8>,
    authority_der: Vec<u8>,
    private_key_der: Vec<u8>,
}

impl ApiServerIdentity {
    /// The certificate chain a handshake sends, leaf first: the API server's own
    /// certificate and then the cluster authority that issued it.
    #[must_use]
    pub fn certificate_chain_der(&self) -> Vec<Vec<u8>> {
        vec![self.certificate_der.clone(), self.authority_der.clone()]
    }

    /// The leaf's private key, in the PKCS#8 DER a TLS server loads it from.
    #[must_use]
    pub fn private_key_der(&self) -> Vec<u8> {
        self.private_key_der.clone()
    }
}

/// Written by hand rather than derived: the struct holds a private key, and a
/// derived rendering would put it in every log line that prints the value it is
/// held on. What is diagnosable is that an identity is there at all.
impl ::std::fmt::Debug for ApiServerIdentity {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ApiServerIdentity")
            .finish_non_exhaustive()
    }
}

/// Provision a cluster's certificate authority and the TLS identity its
/// Kubernetes API server serves under, both valid over `not_before..not_after`.
///
/// `server_names` are the names and addresses the endpoint is dialled by, which
/// the API server's certificate carries as its subject alternative names: a
/// client that verifies the host it connected to finds it there.
///
/// `None` when either half does not issue — the caller then reports that rather
/// than publishing an endpoint whose certificate nothing signed.
pub fn provision_cluster_authority(
    not_before: SystemTime,
    not_after: SystemTime,
    server_names: &[String],
) -> Option<ProvisionedAuthority> {
    let (issuer, authority_pem) = issue_certificate_authority(not_before, not_after)?;
    let (authority_certificate, authority_key) = issuer;
    let key = ::rcgen::KeyPair::generate().ok()?;
    let mut params = ::rcgen::CertificateParams::new(server_names.to_vec()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, API_SERVER_COMMON_NAME);
    params.distinguished_name = subject;
    params.not_before = offset_date_time(not_before)?;
    params.not_after = offset_date_time(not_after)?;
    params.is_ca = ::rcgen::IsCa::NoCa;
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![
        ::rcgen::KeyUsagePurpose::DigitalSignature,
        ::rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    // A server certificate, which is the purpose a verifying client checks it was
    // issued for.
    params.extended_key_usages = vec![::rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let certificate = params
        .signed_by(&key, &authority_certificate, &authority_key)
        .ok()?;
    Some(ProvisionedAuthority {
        data: ::aws_smithy_types::base64::encode(authority_pem),
        api_server: ApiServerIdentity {
            certificate_der: certificate.der().to_vec(),
            authority_der: authority_certificate.der().to_vec(),
            private_key_der: key.serialize_der(),
        },
    })
}

/// The subject common name a cluster's API server certificate is issued under.
///
/// Kubernetes issues its API server's serving certificate to `kube-apiserver`,
/// which is the subject a client that inspects the certificate of a cluster's
/// endpoint sees.
pub const API_SERVER_COMMON_NAME: &str = "kube-apiserver";

/// The certificate authority itself — the certificate and the key that signs with
/// it, plus its PEM — which [`mint_certificate_authority_data`] publishes the
/// Base64 of and [`provision_cluster_authority`] issues the API server's
/// certificate from.
///
/// The key is returned rather than dropped inside so that the ONE certificate
/// this service has to present can be issued from it before it goes out of scope;
/// neither the key nor the certificate is stored anywhere a shape of this service
/// reports.
fn issue_certificate_authority(
    not_before: SystemTime,
    not_after: SystemTime,
) -> Option<((::rcgen::Certificate, ::rcgen::KeyPair), String)> {
    let key = ::rcgen::KeyPair::generate().ok()?;
    // No subject alternative name: a certificate authority certifies the names its
    // issued certificates carry, it does not serve one itself.
    let mut params = ::rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(
        ::rcgen::DnType::CommonName,
        CERTIFICATE_AUTHORITY_COMMON_NAME,
    );
    params.distinguished_name = subject;
    params.not_before = offset_date_time(not_before)?;
    params.not_after = offset_date_time(not_after)?;
    params.is_ca = ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        ::rcgen::KeyUsagePurpose::KeyCertSign,
        ::rcgen::KeyUsagePurpose::CrlSign,
        ::rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let certificate = params.self_signed(&key).ok()?;
    let pem = certificate.pem();
    Some(((certificate, key), pem))
}

/// `at` as the date-time type the certificate generator takes a validity bound
/// in, at the one-second resolution an X.509 validity period is stated at.
///
/// `None` for an instant outside the range that type represents, which is the
/// same answer the caller gives for material that does not issue.
fn offset_date_time(at: SystemTime) -> Option<::time::OffsetDateTime> {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_secs()).ok())?;
    ::time::OffsetDateTime::from_unix_timestamp(seconds).ok()
}
