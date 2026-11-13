// SPDX-License-Identifier: CC0-1.0

//! The asymmetric key pair a key of an asymmetric algorithm IS, and the PKCS#10
//! certificate signing request a caller asks this service to make from it.
//!
//! One concern: the two halves of an asymmetric key and what each half is for. Both
//! are real — the pair comes from `aws-lc-rs` when the key is minted and is stored
//! with the key — and they stay on opposite sides of the service boundary: the private
//! half never leaves, and the only computation defined in terms of it here is the
//! signature over the certificate signing request this module builds; the public half
//! is what that request carries, so a CSR a caller takes to a certificate authority
//! names the very key pair this store holds for the key.

use x509_cert::der::asn1::{Any, BitString};
use x509_cert::der::{Decode, Encode, EncodePem, Tag as DerTag};
use x509_cert::name::{Name, RdnSequence};
use x509_cert::request::{CertReq, CertReqInfo, Version as CertReqVersion};
use x509_cert::spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};

use crate::ops::types::{CertificateSubjectType, SIGNING_ALGORITHM_TYPE_VALUES};

use super::{
    oid, relative_distinguished_name, KeyMaterial, RsaKeyPairDer, StoredKey,
    KEY_CLASS_ASYMMETRIC_PAIR,
};

/// One asymmetric key pair, as the two DER encodings every answer about it needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AsymmetricKeyPair {
    /// The private half, PKCS#8 DER. Carried in the same redacted wrapper as a
    /// symmetric key's secret, so no formatter anywhere can print it.
    private_key_pkcs8: KeyMaterial,
    /// The public half, a DER X.509 `SubjectPublicKeyInfo` — the encoding a
    /// certificate and a certificate signing request both carry a public key in.
    public_key_spki: Vec<u8>,
}

impl AsymmetricKeyPair {
    /// The public half, as the DER `SubjectPublicKeyInfo` this library computed from
    /// the private key it generated.
    pub fn public_key_spki(&self) -> &[u8] {
        &self.public_key_spki
    }
}

/// Why no certificate signing request could be made for a key.
#[derive(Debug, Clone)]
pub enum CertificateRequestRejection {
    /// The request is inadmissible; the message says why, naming the member.
    Invalid(String),
    /// The request is admissible and this service could not encode or sign it.
    Internal,
}

/// Which key pair a key of one `KeyAlgorithm` is: the modulus size of an RSA key, or
/// the curve of an elliptic curve key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPairAlgorithm {
    /// An RSA key pair of this modulus size.
    Rsa(::aws_lc_rs::rsa::KeySize),
    /// An elliptic curve key pair on the curve this signature algorithm names.
    Ecdsa(&'static ::aws_lc_rs::signature::EcdsaSigningAlgorithm),
}

/// The key pair a key of `key_algorithm` IS, or `None` when that algorithm names no
/// key pair this service generates.
///
/// Answered before anything is generated, so a caller tells an algorithm it may not
/// ask for from a generation that failed.
pub fn key_pair_algorithm(key_algorithm: &str) -> Option<KeyPairAlgorithm> {
    // The modelled `KeyAlgorithm` values, from
    // `smithy_query(query="shape",shape="KeyAlgorithm")`, named one by one rather than
    // matched on a name prefix — the rule every table over that enum in this crate
    // follows, so an algorithm the model gains later answers `None` and the create is
    // refused instead of being generated as something else.
    let algorithm = match key_algorithm {
        "RSA_2048" => KeyPairAlgorithm::Rsa(::aws_lc_rs::rsa::KeySize::Rsa2048),
        "RSA_3072" => KeyPairAlgorithm::Rsa(::aws_lc_rs::rsa::KeySize::Rsa3072),
        "RSA_4096" => KeyPairAlgorithm::Rsa(::aws_lc_rs::rsa::KeySize::Rsa4096),
        "ECC_NIST_P256" => {
            KeyPairAlgorithm::Ecdsa(&::aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING)
        }
        "ECC_NIST_P384" => {
            KeyPairAlgorithm::Ecdsa(&::aws_lc_rs::signature::ECDSA_P384_SHA384_ASN1_SIGNING)
        }
        "ECC_NIST_P521" => {
            KeyPairAlgorithm::Ecdsa(&::aws_lc_rs::signature::ECDSA_P521_SHA512_ASN1_SIGNING)
        }
        _ => return None,
    };
    Some(algorithm)
}

/// Generate one key pair of `algorithm`, or `None` when the library could not produce
/// or encode it.
///
/// Every byte is real: the pair comes from `aws-lc-rs` (the RSA modulus size or the
/// NIST curve the key's algorithm names), and the public half is the DER
/// `SubjectPublicKeyInfo` that library computes from the private key it just
/// generated — so a signature made with the private half verifies under the public
/// half a caller is handed.
///
/// Generating an RSA key pair is hundreds of milliseconds of CPU for the larger
/// modulus sizes, which is what a create of an RSA key costs.
pub fn generate_key_pair(algorithm: KeyPairAlgorithm) -> Option<AsymmetricKeyPair> {
    match algorithm {
        KeyPairAlgorithm::Rsa(key_size) => rsa_key_pair(key_size),
        KeyPairAlgorithm::Ecdsa(signature) => ecdsa_key_pair(signature),
    }
}

/// A fresh RSA key pair of `key_size`.
fn rsa_key_pair(key_size: ::aws_lc_rs::rsa::KeySize) -> Option<AsymmetricKeyPair> {
    let generated = RsaKeyPairDer::generate(key_size)?;
    Some(AsymmetricKeyPair {
        private_key_pkcs8: KeyMaterial(generated.private_key_pkcs8),
        public_key_spki: generated.public_key_spki,
    })
}

/// A fresh elliptic curve key pair on the curve `algorithm` names.
///
/// `aws-lc-rs` binds an ECDSA key pair to the curve-and-hash pair it signs with, so the
/// pair is generated under the signature that curve's own strength calls for — the one
/// [`csr_signature`] signs a request for this key with.
fn ecdsa_key_pair(
    algorithm: &'static ::aws_lc_rs::signature::EcdsaSigningAlgorithm,
) -> Option<AsymmetricKeyPair> {
    use ::aws_lc_rs::encoding::{AsDer, PublicKeyX509Der};
    use ::aws_lc_rs::signature::KeyPair;

    let pkcs8 = ::aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(
        algorithm,
        &::aws_lc_rs::rand::SystemRandom::new(),
    )
    .ok()?;
    let pair =
        ::aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(algorithm, pkcs8.as_ref()).ok()?;
    let public_key_spki: PublicKeyX509Der<'static> = pair.public_key().as_der().ok()?;
    Some(AsymmetricKeyPair {
        private_key_pkcs8: KeyMaterial(pkcs8.as_ref().to_vec()),
        public_key_spki: public_key_spki.as_ref().to_vec(),
    })
}

impl StoredKey {
    /// The PKCS#10 certificate signing request for this key: a request naming
    /// `subject`, carrying this key's own public half, signed by its own private half
    /// under `signing_algorithm`.
    ///
    /// Nothing is stored and nothing is derived from the caller's bytes: the request's
    /// subject is the metadata the call gave, its `SubjectPublicKeyInfo` is the public
    /// half of the pair this record has held since the key was minted, and its
    /// signature is made over the DER of the request's own `CertificationRequestInfo`
    /// by the private half of that same pair — so the same key answers the same public
    /// key to every call, and a verifier can check the signature with the public key
    /// the request carries.
    pub fn certificate_signing_request(
        &self,
        subject: &CertificateSubjectType,
        signing_algorithm: &str,
    ) -> Result<String, CertificateRequestRejection> {
        let key_pair = self.key_pair.as_ref().ok_or_else(|| {
            CertificateRequestRejection::Invalid(format!(
                "KeyIdentifier names the key {}, which is a {}, and a certificate signing \
                 request is created from the key pair of an {KEY_CLASS_ASYMMETRIC_PAIR}",
                self.key_arn, self.key_attributes.key_class
            ))
        })?;
        let key_algorithm = self.key_attributes.key_algorithm.as_str();
        let signature = csr_signature(key_algorithm, signing_algorithm).ok_or_else(|| {
            CertificateRequestRejection::Invalid(format!(
                "SigningAlgorithm {signing_algorithm} is not valid for the KeyAlgorithm \
                 {key_algorithm}. Use one of {}.",
                admissible_signing_algorithms(key_algorithm).join(", ")
            ))
        })?;

        encode_request(subject, key_pair, &signature)
            .ok_or(CertificateRequestRejection::Internal)
    }
}

/// Which modelled `SigningAlgorithm` values a certificate signing request for a key of
/// `key_algorithm` can be signed with, in the order the model lists them.
///
/// Derived from [`csr_signature`] over the generated value list of
/// `smithy_query(query="shape",shape="SigningAlgorithmType")`, so the set a refusal
/// names is exactly the set the next call would be served.
fn admissible_signing_algorithms(key_algorithm: &str) -> Vec<&'static str> {
    SIGNING_ALGORITHM_TYPE_VALUES
        .iter()
        .copied()
        .filter(|candidate| csr_signature(key_algorithm, candidate).is_some())
        .collect()
}

/// The whole PKCS#10 request (RFC 2986): the `CertificationRequestInfo`, the signature
/// algorithm that was applied to it and the signature itself, PEM framed and base64
/// encoded.
///
/// `None` when a field cannot be encoded or the signature cannot be made, which is
/// this service's own failure rather than anything the request asked for.
fn encode_request(
    subject: &CertificateSubjectType,
    key_pair: &AsymmetricKeyPair,
    signature: &CsrSignature,
) -> Option<String> {
    let info = CertReqInfo {
        // "version shall be 1 for this version of the standard" (RFC 2986 section
        // 4.1), which `x509-cert` spells `Version::V1`.
        version: CertReqVersion::V1,
        subject: subject_name(subject)?,
        public_key: SubjectPublicKeyInfoOwned::from_der(key_pair.public_key_spki()).ok()?,
        // No attributes: RFC 2986 section 4.1 makes the set optional, and this request
        // carries nothing beyond the subject the caller named.
        attributes: Default::default(),
    };
    let algorithm = signature.identifier()?;
    let signed = signature.sign(key_pair, &info.to_der().ok()?)?;
    let request = CertReq {
        info,
        algorithm,
        signature: BitString::from_bytes(&signed).ok()?,
    };
    // Every certificate this service hands out is "in PEM format (base64 encoded)"
    // (the `CertificateType` members' own documentation) and a certificate signing
    // request is the same kind of document, so it is handed out in the same form: the
    // PKCS#10 PEM block, base64 encoded. The PEM framing comes from `x509-cert`'s own
    // encoder, not from anything written here.
    Some(::aws_smithy_types::base64::encode(
        request.to_pem(::x509_cert::der::pem::LineEnding::LF).ok()?,
    ))
}

/// The X.500 distinguished name a `CertificateSubjectType` names.
///
/// One relative distinguished name per member the caller gave, in the order a subject
/// is written from most general to most specific (RFC 4514 section 2.1 renders the
/// sequence in reverse, so the DER order is country first and common name last), with
/// the PKCS#9 `emailAddress` last of all. `CommonName` is the one member the model
/// requires, so a subject always names at least that.
fn subject_name(subject: &CertificateSubjectType) -> Option<Name> {
    let attributes = [
        (oid::COUNTRY, DerTag::PrintableString, subject.country.as_deref()),
        (
            oid::STATE_OR_PROVINCE,
            DerTag::Utf8String,
            subject.state_or_province.as_deref(),
        ),
        (oid::LOCALITY, DerTag::Utf8String, subject.city.as_deref()),
        (
            oid::ORGANIZATION,
            DerTag::Utf8String,
            subject.organization.as_deref(),
        ),
        (
            oid::ORGANIZATIONAL_UNIT,
            DerTag::Utf8String,
            subject.organization_unit.as_deref(),
        ),
        (
            oid::COMMON_NAME,
            DerTag::Utf8String,
            Some(subject.common_name.as_str()),
        ),
        (
            oid::EMAIL_ADDRESS,
            DerTag::Ia5String,
            subject.email_address.as_deref(),
        ),
    ];
    let mut sequence = Vec::new();
    for (attribute, tag, value) in attributes {
        if let Some(value) = value {
            sequence.push(relative_distinguished_name(
                attribute,
                string_tag(tag, value),
                value,
            )?);
        }
    }
    Some(Name::from(RdnSequence(sequence)))
}

/// The DER string type one subject attribute's value is encoded as.
///
/// X.520 gives `countryName` as a `PrintableString` and the PKCS#9 `emailAddress` as an
/// `IA5String`, and both of those admit fewer characters than the modelled members do
/// (`Country` is `^[A-Za-z]+$`, but `EmailAddress` admits a non-ASCII apostrophe) — so a
/// value outside the narrower type is encoded as the `UTF8String` every other subject
/// attribute uses rather than failing to encode at all.
fn string_tag(preferred: DerTag, value: &str) -> DerTag {
    let fits = match preferred {
        // The PrintableString character set (ITU-T X.680 table 10).
        DerTag::PrintableString => value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " '()+,-./:=?".contains(c)),
        DerTag::Ia5String => value.is_ascii(),
        _ => true,
    };
    if fits {
        preferred
    } else {
        DerTag::Utf8String
    }
}

/// What a certificate signing request is signed with: the signature algorithm the
/// request names and the primitive that makes that signature.
struct CsrSignature {
    /// The OID of the `AlgorithmIdentifier` the request carries.
    algorithm: ObjectIdentifier,
    /// The primitive the key's own algorithm signs with.
    primitive: SigningPrimitive,
}

/// The signature scheme one key family signs with.
enum SigningPrimitive {
    /// RSASSA-PKCS1-v1_5 over the named hash (RFC 8017 section 8.2).
    Rsa(&'static ::aws_lc_rs::signature::RsaSignatureEncoding),
    /// ECDSA over the named hash, with the DER `Ecdsa-Sig-Value` encoding RFC 5758
    /// section 3.2 requires — the `_ASN1_SIGNING` form.
    Ecdsa(&'static ::aws_lc_rs::signature::EcdsaSigningAlgorithm),
}

impl CsrSignature {
    /// The `AlgorithmIdentifier` the request names this signature by.
    fn identifier(&self) -> Option<AlgorithmIdentifierOwned> {
        Some(AlgorithmIdentifierOwned {
            oid: self.algorithm,
            parameters: match self.primitive {
                // RSASSA-PKCS1-v1_5 takes an explicit NULL parameter (RFC 8017
                // appendix C); for ECDSA "the parameters field MUST be absent"
                // (RFC 5758 section 3.2).
                SigningPrimitive::Rsa(_) => Some(Any::new(DerTag::Null, Vec::new()).ok()?),
                SigningPrimitive::Ecdsa(_) => None,
            },
        })
    }

    /// `message` signed by the private half of `key_pair` under this signature's own
    /// primitive.
    fn sign(&self, key_pair: &AsymmetricKeyPair, message: &[u8]) -> Option<Vec<u8>> {
        let private_key = key_pair.private_key_pkcs8.bytes();
        match self.primitive {
            SigningPrimitive::Rsa(encoding) => {
                let pair =
                    ::aws_lc_rs::signature::RsaKeyPair::from_pkcs8(private_key).ok()?;
                // A PKCS #1 signature is exactly as long as the key's modulus.
                let mut signature = vec![0u8; pair.public_modulus_len()];
                pair.sign(
                    encoding,
                    &::aws_lc_rs::rand::SystemRandom::new(),
                    message,
                    &mut signature,
                )
                .ok()?;
                Some(signature)
            }
            SigningPrimitive::Ecdsa(algorithm) => {
                let pair = ::aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
                    algorithm,
                    private_key,
                )
                .ok()?;
                let signature = pair
                    .sign(&::aws_lc_rs::rand::SystemRandom::new(), message)
                    .ok()?;
                Some(signature.as_ref().to_vec())
            }
        }
    }
}

/// The signature a certificate signing request for a key of `key_algorithm` carries
/// when the caller asks for `signing_algorithm`, or `None` when the two name no
/// signature this service can make.
///
/// A key signs with the scheme its own algorithm defines — RSASSA-PKCS1-v1_5 for an RSA
/// key, ECDSA for a NIST curve key — over the hash the request names, and the OID is
/// the one that pair has: `sha{256,384,512}WithRSAEncryption` (RFC 8017 appendix C) or
/// `ecdsa-with-SHA{256,384,512}` (RFC 5758 section 3.2).
///
/// Only pairs a real primitive signs are here. A signature this service cannot make is
/// refused rather than answered with a signature over some other hash, so every
/// request a caller is handed is signed by exactly the algorithm it names: an RSA key
/// signs under any of the SHA-2 hashes the scheme defines an OID for, and an elliptic
/// curve key under the hash matched to its curve's own strength (FIPS 186-4 section
/// 6.1.1 pairs each curve with a hash of at least its security strength).
fn csr_signature(key_algorithm: &str, signing_algorithm: &str) -> Option<CsrSignature> {
    let (algorithm, primitive) = match (key_algorithm, signing_algorithm) {
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA256") => (
            oid::SHA256_WITH_RSA,
            SigningPrimitive::Rsa(&::aws_lc_rs::signature::RSA_PKCS1_SHA256),
        ),
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA384") => (
            oid::SHA384_WITH_RSA,
            SigningPrimitive::Rsa(&::aws_lc_rs::signature::RSA_PKCS1_SHA384),
        ),
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA512") => (
            oid::SHA512_WITH_RSA,
            SigningPrimitive::Rsa(&::aws_lc_rs::signature::RSA_PKCS1_SHA512),
        ),
        ("ECC_NIST_P256", "SHA256") => (
            oid::ECDSA_WITH_SHA256,
            SigningPrimitive::Ecdsa(&::aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING),
        ),
        ("ECC_NIST_P384", "SHA384") => (
            oid::ECDSA_WITH_SHA384,
            SigningPrimitive::Ecdsa(&::aws_lc_rs::signature::ECDSA_P384_SHA384_ASN1_SIGNING),
        ),
        ("ECC_NIST_P521", "SHA512") => (
            oid::ECDSA_WITH_SHA512,
            SigningPrimitive::Ecdsa(&::aws_lc_rs::signature::ECDSA_P521_SHA512_ASN1_SIGNING),
        ),
        _ => return None,
    };
    Some(CsrSignature {
        algorithm,
        primitive,
    })
}
