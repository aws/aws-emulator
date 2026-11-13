// SPDX-License-Identifier: CC0-1.0

//! The X.509 identity a native session request presents, and whether it proves itself.
//!
//! IAM Roles Anywhere's data plane is addressed by a workload that holds no AWS credential
//! at all: it holds an X.509 certificate and the private key that certificate certifies.
//! The request therefore carries two things instead of a SigV4 credential — the DER
//! certificate, base64-encoded in [`CERTIFICATE_HEADER`], and an `Authorization` header in
//! the SigV4-derived scheme named by [`RSA_ALGORITHM`] / [`ECDSA_ALGORITHM`], whose
//! signature is made with the certificate's private key over the standard SigV4
//! string-to-sign rather than with an HMAC of a secret access key.
//!
//! Two questions are answered here, and nothing else:
//!
//! 1. **Who is this?** [`PresentedCertificate`] is the certificate the request presented,
//!    read with `x509-cert` — the DER/ASN.1 and PEM reading this workspace does with that
//!    crate rather than by hand — and [`chains_to_authority_bundle`] says whether it was
//!    issued by one of the certificate authorities a trust anchor's bundle holds, by
//!    verifying the issuer's signature over the certificate itself. A certificate outside
//!    its own validity window, or one no authority in the bundle signed, establishes
//!    nothing.
//! 2. **Does the holder hold the private key?** [`signature_verifies`] rebuilds the SigV4
//!    canonical request and string-to-sign from the request as it arrived and checks the
//!    hex signature the `Authorization` header carries against the certificate's public
//!    key. A certificate is public, so without this step anyone holding a copy of someone
//!    else's certificate could ask for that identity's credentials.
//!
//! The emulator's own authenticating layer cannot answer either: it verifies SigV4 with
//! this process's keys (`emulator_server::SigV4Authenticator`), and neither the scheme name
//! nor the key here is one it knows — the credential-scope reader it offers
//! (`emulator_server::sigv4_credential_scope`) is keyed to `AWS4-HMAC-SHA256`. So the
//! scheme's own header is read by [`parse_authorization`] below, which is a total parse of
//! three comma-separated fields and no more.
//!
//! Every bound a caller could grow is declared as a constant here: the certificate header,
//! the authority bundle, the signature and the signed-header list each have a ceiling, and
//! nothing recurses on anything the caller chose.
//!
//! The cryptography is `ring`'s, the one library this workspace verifies signatures with.
//! The algorithm is never taken from the caller alone: the scheme name has to agree with
//! the key the certificate carries, and a chain signature is verified under the algorithm
//! the certificate itself declares, so a request cannot name a weaker algorithm than the
//! key it presents.

use std::time::SystemTime;

use ::ring::signature::VerificationAlgorithm;
use ::sha2::{Digest as _, Sha256};
use ::x509_cert::der::{Decode as _, Encode as _, EncodePem as _};
use ::x509_cert::Certificate;

/// The header the DER end-entity certificate travels in, base64-encoded.
pub const CERTIFICATE_HEADER: &str = "x-amz-x509";

/// The scheme name a request signed with an RSA certificate key carries: SigV4's
/// structure with RSASSA-PKCS1-v1_5 over SHA-256 in place of the HMAC.
pub const RSA_ALGORITHM: &str = "AWS4-X509-RSA-SHA256";

/// The scheme name a request signed with an elliptic-curve certificate key carries.
pub const ECDSA_ALGORITHM: &str = "AWS4-X509-ECDSA-SHA256";

/// The fourth element of the credential scope, which is also this service's signing name.
pub const SIGNING_NAME: &str = "rolesanywhere";

/// The terminator every SigV4 credential scope ends with.
const SCOPE_TERMINATOR: &str = "aws4_request";

/// The header carrying the instant the request was signed at, which is the second line of
/// the string-to-sign.
const DATE_HEADER: &str = "x-amz-date";

/// The header a caller may state the payload hash in, which SigV4 then takes verbatim as
/// the canonical request's last line instead of hashing the body again.
const CONTENT_HASH_HEADER: &str = "x-amz-content-sha256";

/// The value a caller states in that header INSTEAD of a digest, to sign a request without
/// covering its body.
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// The prefix of the values a caller states in that header for a chunked payload, whose
/// chunks carry their own digests.
const STREAMING_PAYLOAD_PREFIX: &str = "STREAMING-";

/// The largest base64 certificate this plane decodes.
///
/// An X.509 certificate with an RSA-4096 key and a full set of extensions is a few
/// kilobytes; this ceiling sits far above that and bounds the work one request can ask for
/// before anything is parsed.
const MAX_CERTIFICATE_HEADER_BYTES: usize = 16 * 1024;

/// The largest authority bundle this plane reads a trust anchor's certificates out of.
///
/// `SourceData$x509CertificateData` is bounded by the model at 8000 characters
/// (`smithy_query(query="shape", shape="SourceData")`); this ceiling is that bound with
/// room for the PEM framing, so a stored bundle is always readable and nothing larger is
/// walked.
const MAX_AUTHORITY_BUNDLE_BYTES: usize = 64 * 1024;

/// The most certificates one bundle is searched for the issuer.
const MAX_AUTHORITY_CERTIFICATES: usize = 32;

/// The most header names one signature may cover.
const MAX_SIGNED_HEADERS: usize = 64;

/// The largest signature this plane decodes: an RSA-8192 signature is 1024 bytes, so this
/// covers every key size `ring` verifies with room to spare.
const MAX_SIGNATURE_BYTES: usize = 2 * 1024;

/// `rsaEncryption`, the public-key algorithm of an RSA certificate (RFC 8017).
const RSA_ENCRYPTION_OID: &str = "1.2.840.113549.1.1.1";
/// `id-ecPublicKey`, the public-key algorithm of an elliptic-curve certificate (RFC 5480).
const EC_PUBLIC_KEY_OID: &str = "1.2.840.10045.2.1";
/// `sha256WithRSAEncryption` (RFC 8017).
const SHA256_WITH_RSA_OID: &str = "1.2.840.113549.1.1.11";
/// `sha384WithRSAEncryption` (RFC 8017).
const SHA384_WITH_RSA_OID: &str = "1.2.840.113549.1.1.12";
/// `sha512WithRSAEncryption` (RFC 8017).
const SHA512_WITH_RSA_OID: &str = "1.2.840.113549.1.1.13";
/// `ecdsa-with-SHA256` (RFC 5758).
const ECDSA_WITH_SHA256_OID: &str = "1.2.840.10045.4.3.2";
/// `ecdsa-with-SHA384` (RFC 5758).
const ECDSA_WITH_SHA384_OID: &str = "1.2.840.10045.4.3.3";
/// `prime256v1` / NIST P-256 (RFC 5480).
const PRIME256V1_OID: &str = "1.2.840.10045.3.1.7";
/// `secp384r1` / NIST P-384 (RFC 5480).
const SECP384R1_OID: &str = "1.3.132.0.34";

/// `basicConstraints`, the extension that says whether a certificate may issue others
/// (RFC 5280 section 4.2.1.9).
const BASIC_CONSTRAINTS_OID: &str = "2.5.29.19";

/// The end-entity certificate one request presented, parsed.
#[derive(Debug, Clone)]
pub struct PresentedCertificate {
    parsed: Certificate,
}

impl PresentedCertificate {
    /// The certificate a [`CERTIFICATE_HEADER`] value carries, or `None` when the value is
    /// not one base64 DER certificate within [`MAX_CERTIFICATE_HEADER_BYTES`].
    pub fn from_header_value(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_CERTIFICATE_HEADER_BYTES {
            return None;
        }
        let der = ::aws_smithy_types::base64::decode(value).ok()?;
        Self::from_der(&der)
    }

    /// The certificate `der` encodes, or `None` when those bytes are not one.
    pub fn from_der(der: &[u8]) -> Option<Self> {
        Certificate::from_der(der)
            .ok()
            .map(|parsed| Self { parsed })
    }

    /// `SubjectDetail$x509Subject`: "the x509 principal identifier of the authenticating
    /// certificate", which is the certificate's own subject distinguished name.
    pub fn subject(&self) -> String {
        self.parsed.tbs_certificate.subject.to_string()
    }

    /// `CredentialSummary$issuer`: the name of the authority that issued this certificate,
    /// taken from the certificate's own issuer field.
    pub fn issuer(&self) -> String {
        self.parsed.tbs_certificate.issuer.to_string()
    }

    /// The certificate's serial number, hex-encoded with no leading zero — the form the
    /// default role session name of a native session carries, and the form
    /// `CredentialSummary$serialNumber` reports.
    ///
    /// The serial is a DER INTEGER, so its encoding carries a leading zero byte whenever
    /// its high bit would otherwise make it negative; the zeros are dropped here so the
    /// value is the number itself rather than one encoding of it. A serial of zero renders
    /// as `0`.
    pub fn serial_hex(&self) -> String {
        let encoded = ::hex::encode(self.parsed.tbs_certificate.serial_number.as_bytes());
        let trimmed = encoded.trim_start_matches('0');
        if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// `CredentialSummary$x509CertificateData`: "the PEM-encoded data of the certificate",
    /// re-encoded from what was presented.
    pub fn pem(&self) -> Option<String> {
        self.parsed.to_pem(::x509_cert::der::pem::LineEnding::LF).ok()
    }

    /// Whether `now` is inside the certificate's own validity window.
    pub fn is_current(&self, now: SystemTime) -> bool {
        is_current(&self.parsed, now)
    }
}

/// Whether `now` is inside `certificate`'s validity window.
fn is_current(certificate: &Certificate, now: SystemTime) -> bool {
    let validity = &certificate.tbs_certificate.validity;
    validity.not_before.to_system_time() <= now && now <= validity.not_after.to_system_time()
}

/// Whether `certificate` was issued by one of the certificate authorities `bundle` holds —
/// the `x509CertificateData` of a `CERTIFICATE_BUNDLE` trust anchor — and both it and that
/// authority are current at `now`.
///
/// "Issued by" is the authority's SIGNATURE over the certificate, not a name match: the
/// authority's subject has to equal the certificate's issuer AND the authority's public key
/// has to verify the signature the certificate carries, under the algorithm the certificate
/// declares. A bundle that is not readable as PEM certificates issues nothing.
pub fn chains_to_authority_bundle(
    certificate: &PresentedCertificate,
    bundle: &str,
    now: SystemTime,
) -> bool {
    if bundle.len() > MAX_AUTHORITY_BUNDLE_BYTES || !certificate.is_current(now) {
        return false;
    }
    let Ok(authorities) = Certificate::load_pem_chain(bundle.as_bytes()) else {
        return false;
    };
    authorities
        .iter()
        .take(MAX_AUTHORITY_CERTIFICATES)
        .any(|authority| issued(authority, &certificate.parsed, now))
}

/// Whether `authority` issued `certificate`: the names line up, the authority is current
/// and is one its own extensions let issue certificates, and its key verifies the
/// certificate's signature over the certificate's own `TBSCertificate` — which is the only
/// thing a signature on a certificate covers.
fn issued(authority: &Certificate, certificate: &Certificate, now: SystemTime) -> bool {
    if authority.tbs_certificate.subject != certificate.tbs_certificate.issuer
        || !is_current(authority, now)
        || !may_issue(authority)
    {
        return false;
    }
    let Ok(signed) = certificate.tbs_certificate.to_der() else {
        return false;
    };
    let Some(signature) = certificate.signature.as_bytes() else {
        return false;
    };
    let declared = certificate.signature_algorithm.oid.to_string();
    let Some((key, algorithm)) = key_and_algorithm(authority, &declared) else {
        return false;
    };
    ::ring::signature::UnparsedPublicKey::new(algorithm, key)
        .verify(&signed, signature)
        .is_ok()
}

/// The public key `certificate` carries and the `ring` algorithm that verifies a
/// `signature_oid` signature made by it, or `None` when the two do not go together — an
/// RSA signature named over an elliptic-curve key, a curve or a digest width this does not
/// verify.
fn key_and_algorithm<'c>(
    certificate: &'c Certificate,
    signature_oid: &str,
) -> Option<(&'c [u8], &'static dyn VerificationAlgorithm)> {
    let key_info = &certificate.tbs_certificate.subject_public_key_info;
    let key = key_info.subject_public_key.as_bytes()?;
    let key_oid = key_info.algorithm.oid.to_string();
    let algorithm: &'static dyn VerificationAlgorithm = match (signature_oid, key_oid.as_str()) {
        (SHA256_WITH_RSA_OID, RSA_ENCRYPTION_OID) => {
            &::ring::signature::RSA_PKCS1_2048_8192_SHA256
        }
        (SHA384_WITH_RSA_OID, RSA_ENCRYPTION_OID) => {
            &::ring::signature::RSA_PKCS1_2048_8192_SHA384
        }
        (SHA512_WITH_RSA_OID, RSA_ENCRYPTION_OID) => {
            &::ring::signature::RSA_PKCS1_2048_8192_SHA512
        }
        // An elliptic-curve signature is verified under the named curve the key itself
        // carries, so the digest width and the curve are both the certificate's own.
        (ECDSA_WITH_SHA256_OID | ECDSA_WITH_SHA384_OID, EC_PUBLIC_KEY_OID) => {
            let curve = key_info
                .algorithm
                .parameters
                .as_ref()
                .and_then(|parameters| {
                    parameters
                        .decode_as::<::x509_cert::der::asn1::ObjectIdentifier>()
                        .ok()
                })?
                .to_string();
            match (signature_oid, curve.as_str()) {
                (ECDSA_WITH_SHA256_OID, PRIME256V1_OID) => {
                    &::ring::signature::ECDSA_P256_SHA256_ASN1
                }
                (ECDSA_WITH_SHA384_OID, SECP384R1_OID) => {
                    &::ring::signature::ECDSA_P384_SHA384_ASN1
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    Some((key, algorithm))
}

/// Whether `certificate` is one its own extensions let issue certificates: a
/// `basicConstraints` extension that does not assert `cA` names a certificate that cannot
/// (RFC 5280 section 4.2.1.9), so an end-entity certificate sitting in a bundle issues
/// nothing even when it signed something. A certificate carrying no such extension is left
/// to the chain signature alone — the bundle is the account's own statement of which
/// authorities it trusts.
fn may_issue(certificate: &Certificate) -> bool {
    let Some(extensions) = certificate.tbs_certificate.extensions.as_ref() else {
        return true;
    };
    let stated = extensions
        .iter()
        .find(|extension| extension.extn_id.to_string() == BASIC_CONSTRAINTS_OID);
    match stated {
        Some(extension) => ::x509_cert::ext::pkix::BasicConstraints::from_der(
            extension.extn_value.as_bytes(),
        )
        .is_ok_and(|constraints| constraints.ca),
        None => true,
    }
}

/// The `Authorization` header of a native session request, read.
#[derive(Debug, Clone)]
pub struct X509Authorization {
    /// The scheme name, which also names the signature algorithm.
    pub algorithm: String,
    /// The first element of the credential scope. The signing helper puts the signing
    /// certificate's serial number there, in place of an access key id.
    pub credential: String,
    /// The Region the credential scope names, which is the Region the request was signed
    /// for.
    pub region: String,
    /// The header names, lowercased, the signature covers, in the order the header lists
    /// them — which is the order they are canonicalized in.
    pub signed_headers: Vec<String>,
    /// The signature itself, hex-decoded.
    pub signature: Vec<u8>,
    /// `<date stamp>/<region>/<service>/aws4_request`, the third line of the
    /// string-to-sign.
    pub scope: String,
}

/// Whether `value` is an `Authorization` header in this scheme — the one test of that
/// which the claim and the parse below share.
pub fn names_the_scheme(value: &str) -> bool {
    let trimmed = value.trim_start();
    trimmed.starts_with(RSA_ALGORITHM) || trimmed.starts_with(ECDSA_ALGORITHM)
}

/// The `Authorization` header `value` carries, or `None` when it is not this scheme's.
///
/// Total: every field is optional in the parse and absent fields answer `None`, nothing is
/// indexed and nothing recurses. The scope has to name this service and end in
/// `aws4_request`, so a header signed for another service is not read as one of ours.
pub fn parse_authorization(value: &str) -> Option<X509Authorization> {
    let (algorithm, fields) = value.trim().split_once(' ')?;
    if algorithm != RSA_ALGORITHM && algorithm != ECDSA_ALGORITHM {
        return None;
    }
    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for field in fields.split(',') {
        let field = field.trim();
        if let Some(value) = field.strip_prefix("Credential=") {
            credential = Some(value.trim());
        } else if let Some(value) = field.strip_prefix("SignedHeaders=") {
            signed_headers = Some(value.trim());
        } else if let Some(value) = field.strip_prefix("Signature=") {
            signature = Some(value.trim());
        }
    }
    let mut scope_fields = credential?.split('/');
    let (Some(credential), Some(date), Some(region), Some(service), Some(SCOPE_TERMINATOR), None) = (
        scope_fields.next(),
        scope_fields.next(),
        scope_fields.next(),
        scope_fields.next(),
        scope_fields.next(),
        scope_fields.next(),
    ) else {
        return None;
    };
    if credential.is_empty() || date.is_empty() || region.is_empty() || service != SIGNING_NAME {
        return None;
    }
    let names: Vec<String> = signed_headers?
        .split(';')
        .filter(|name| !name.is_empty())
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    if names.is_empty() || names.len() > MAX_SIGNED_HEADERS {
        return None;
    }
    let signature = ::hex::decode(signature?).ok()?;
    if signature.is_empty() || signature.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    Some(X509Authorization {
        algorithm: algorithm.to_string(),
        credential: credential.to_string(),
        region: region.to_string(),
        signed_headers: names,
        signature,
        scope: format!("{date}/{region}/{service}/{SCOPE_TERMINATOR}"),
    })
}

/// The request a signature covers, as it arrived.
pub struct SignedRequest<'r> {
    pub method: &'r str,
    pub uri: &'r str,
    pub headers: &'r ::aws_smithy_runtime_api::http::Headers,
    pub body: &'r [u8],
}

/// Whether `authorization`'s signature is `request` signed by the private key
/// `certificate` certifies.
///
/// The string-to-sign is rebuilt from the request as it arrived, by SigV4's own
/// construction (AWS General Reference, "Create a signed AWS API request"): the canonical
/// request, hashed, under the scheme name and the credential scope the header carries. So a
/// request whose method, path, query, signed headers or body differ from what was signed
/// does not verify, and neither does one signed by any other key.
pub fn signature_verifies(
    authorization: &X509Authorization,
    certificate: &PresentedCertificate,
    request: &SignedRequest<'_>,
) -> bool {
    let Some(signed_at) = request.headers.get(DATE_HEADER) else {
        return false;
    };
    let Some(canonical) = canonical_request(authorization, request) else {
        return false;
    };
    let string_to_sign = format!(
        "{}\n{}\n{}\n{}",
        authorization.algorithm,
        signed_at,
        authorization.scope,
        hex_sha256(canonical.as_bytes()),
    );
    let signature_oid = match authorization.algorithm.as_str() {
        // The scheme name states the digest width and the key family, so it is mapped to
        // the signature algorithm the certificate's key would have signed under.
        RSA_ALGORITHM => SHA256_WITH_RSA_OID,
        ECDSA_ALGORITHM => ECDSA_WITH_SHA256_OID,
        _ => return false,
    };
    let Some((key, algorithm)) = key_and_algorithm(&certificate.parsed, signature_oid) else {
        return false;
    };
    ::ring::signature::UnparsedPublicKey::new(algorithm, key)
        .verify(string_to_sign.as_bytes(), &authorization.signature)
        .is_ok()
}

/// The SigV4 canonical request for `request`, covering exactly the headers
/// `authorization` says the signature covers, or `None` when one of those headers is not
/// on the request at all.
fn canonical_request(
    authorization: &X509Authorization,
    request: &SignedRequest<'_>,
) -> Option<String> {
    // The path and the query are read with the `http` crate's own URI parse rather than by
    // splitting the string here.
    let uri: ::http::Uri = request.uri.parse().ok()?;
    let path = match uri.path() {
        "" => "/",
        path => path,
    };
    let mut canonical = String::new();
    canonical.push_str(request.method);
    canonical.push('\n');
    canonical.push_str(path);
    canonical.push('\n');
    canonical.push_str(&canonical_query(uri.query().unwrap_or_default()));
    canonical.push('\n');
    for name in &authorization.signed_headers {
        let value = header_value(request, name)?;
        canonical.push_str(name);
        canonical.push(':');
        canonical.push_str(&value);
        canonical.push('\n');
    }
    canonical.push('\n');
    canonical.push_str(&authorization.signed_headers.join(";"));
    canonical.push('\n');
    canonical.push_str(&payload_hash(authorization, request)?);
    Some(canonical)
}

/// The value the canonical request carries for one signed header: the header's own value
/// with surrounding space removed and internal runs of space collapsed, as SigV4
/// canonicalizes a header value.
///
/// `host` falls back to the authority the URI carries, because an HTTP/2 request names the
/// host there rather than in a header (the `http` crate parses `:authority` onto the URI),
/// while the client signed the host either way.
fn header_value(request: &SignedRequest<'_>, name: &str) -> Option<String> {
    let carried = request.headers.get(name).map(str::to_string).or_else(|| {
        (name == "host")
            .then(|| request.uri.parse::<::http::Uri>().ok())
            .flatten()
            .and_then(|uri| uri.authority().map(|authority| authority.to_string()))
    })?;
    Some(
        carried
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ")
            .trim()
            .to_string(),
    )
}

/// The canonical query string: every parameter as it was sent, in sorted order, which is
/// SigV4's canonical form of the query component.
fn canonical_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut parameters: Vec<(&str, &str)> = query
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| match parameter.split_once('=') {
            Some((name, value)) => (name, value),
            None => (parameter, ""),
        })
        .collect();
    parameters.sort_unstable();
    parameters
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<String>>()
        .join("&")
}

/// The canonical request's last line: the payload hash the caller stated in a header the
/// signature COVERS — SigV4 takes that value verbatim, which is what lets a caller sign an
/// unsigned payload — else the hash of the body as it arrived.
///
/// A stated hash is read only when it is signed: an unsigned header is not part of what the
/// caller signed, so letting one decide the payload line would let anything that can add a
/// header change what the signature is checked against. And a stated DIGEST has to be the
/// digest of the body that arrived — `None` when it is not, so the request does not verify:
/// a signature over a digest the body does not match covers nothing about the body. The
/// sentinels a caller states INSTEAD of a digest ([`UNSIGNED_PAYLOAD`],
/// [`STREAMING_PAYLOAD_PREFIX`]) are taken as they stand, which is what they are for.
fn payload_hash(authorization: &X509Authorization, request: &SignedRequest<'_>) -> Option<String> {
    let stated = authorization
        .signed_headers
        .iter()
        .any(|name| name == CONTENT_HASH_HEADER)
        .then(|| request.headers.get(CONTENT_HASH_HEADER))
        .flatten();
    let Some(stated) = stated else {
        return Some(hex_sha256(request.body));
    };
    if stated == UNSIGNED_PAYLOAD || stated.starts_with(STREAMING_PAYLOAD_PREFIX) {
        return Some(stated.to_string());
    }
    (stated == hex_sha256(request.body)).then(|| stated.to_string())
}

/// The lowercase hex SHA-256 of `bytes`, the digest SigV4 is defined over.
fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    ::hex::encode(hasher.finalize())
}
