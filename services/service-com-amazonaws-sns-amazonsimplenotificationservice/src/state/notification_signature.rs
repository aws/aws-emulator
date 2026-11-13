// SPDX-License-Identifier: CC0-1.0

//! The SIGNATURE a delivered notification carries, and the certificate a subscriber
//! verifies it against.
//!
//! "SignatureVersion — Version of the Amazon SNS signature used. If the SignatureVersion is
//! 1, Signature is a Base64-encoded SHA1withRSA signature of the Message, MessageId, Subject
//! (if present), Type, Timestamp, and TopicArn values. If the SignatureVersion is 2,
//! Signature is a Base64-encoded SHA256withRSA signature of the same values", and
//! "SigningCertURL — The URL of the certificate that was used to sign the message" (Amazon
//! SNS Developer Guide, "Verifying the signatures of Amazon SNS messages"); the version a
//! topic signs under is its own `SignatureVersion` attribute, "By default SignatureVersion
//! is set to 1" (`SetTopicAttributesInput$AttributeName`).
//!
//! So every member here is REAL: the key pair is generated in this process, the certificate
//! published at the signing-certificate URL is a self-signed X.509 certificate over that
//! key's public half, and the signature is the RSASSA-PKCS1-v1_5 signature (RFC 8017 section
//! 8.2) of the canonical string over the digest the version names. A subscriber that follows
//! the documented pattern — fetch the certificate, rebuild the canonical string, verify the
//! signature with the digest `SignatureVersion` selects — verifies what this service
//! delivered, and a member that only LOOKED like a signature would be one it could not.
//!
//! The key pair belongs to the process rather than to a topic: AWS signs every notification
//! of a Region with the service's own key, and a subscriber reads which key from the
//! certificate URL the notification carries. It is generated once, on the first notification
//! that needs it, and the private half is only ever read here — nothing writes it to a
//! response, a log or a file.

use std::sync::OnceLock;

use ::pkcs1::der::Decode;

use super::TopicRecord;

/// The member a notification carries its signature version under.
pub const SIGNATURE_VERSION_MEMBER: &str = "SignatureVersion";

/// The member a notification carries its signature under.
pub const SIGNATURE_MEMBER: &str = "Signature";

/// The member the JSON notification envelope carries the signing certificate's URL under,
/// as the documented envelope spells it.
pub const CERTIFICATE_URL_MEMBER: &str = "SigningCertURL";

/// The member a FUNCTION's event record carries that same URL under: a Lambda event spells
/// the URL members `SigningCertUrl` / `UnsubscribeUrl`, where the JSON envelope spells them
/// `SigningCertURL` / `UnsubscribeURL`.
pub const EVENT_CERTIFICATE_URL_MEMBER: &str = "SigningCertUrl";

/// The signature version SNS signs under unless the topic asks for the other one: "By
/// default SignatureVersion is set to 1" (`SetTopicAttributesInput$AttributeName`), which is
/// the SHA-1 digest.
pub const DEFAULT_SIGNATURE_VERSION: &str = "1";

/// The signature version that selects the SHA-256 digest.
pub const SHA256_SIGNATURE_VERSION: &str = "2";

/// The members of a `Notification` that are signed, in the order the canonical string
/// carries them (the Developer Guide's "Verifying the signatures of Amazon SNS messages":
/// each member's name, a newline, its value and a newline, with `Subject` present only when
/// the message carried one).
const SIGNED_MEMBERS: &[&str] = &[
    "Message",
    "MessageId",
    "Subject",
    "Timestamp",
    "TopicArn",
    "Type",
];

/// The first path segment of the signing certificate, as SNS names that document:
/// `SimpleNotificationService-<id>.pem`.
const CERTIFICATE_FILE_PREFIX: &str = "/SimpleNotificationService-";

/// The extension of the signing certificate's file, which is PEM.
const CERTIFICATE_FILE_SUFFIX: &str = ".pem";

/// How many hex characters name the key in the certificate's file name — the width AWS's
/// own `SimpleNotificationService-<32 hex characters>.pem` carries. It is a NAME, not a
/// secret: it is the digest of the published certificate, so two keys are published at two
/// URLs and the same key always at one.
const CERTIFICATE_ID_HEX_CHARS: usize = 32;

/// How long the certificate this service publishes is valid: ten years, as AWS's own
/// signing certificates are issued for, counted as the 3652 days ten Gregorian years span.
const CERTIFICATE_VALIDITY: std::time::Duration =
    std::time::Duration::from_secs(3652 * 24 * 60 * 60);

/// The common name the published certificate is issued to: the service principal Amazon SNS
/// performs a delivery as, which is the identity the notification is signed by.
const CERTIFICATE_SUBJECT: &str = super::SERVICE_PRINCIPAL;

/// The DigestInfo prefix of a SHA-1 digest, as the EMSA-PKCS1-v1_5 encoding of RFC 8017
/// (section 9.2, note 1) fixes it for that hash: the DER of
/// `DigestInfo { SEQUENCE { sha1 AlgorithmIdentifier, OCTET STRING (20) } }`.
const SHA1_DIGEST_INFO_PREFIX: &[u8] = &[
    0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14,
];

/// The signature version `topic`'s notifications are signed under: the topic's own
/// `SignatureVersion` attribute, and [`DEFAULT_SIGNATURE_VERSION`] for a topic that carries
/// none — or for a replay of a topic that is no longer there.
///
/// Answers a `&'static str` out of the two versions SNS publishes, so a value the attribute
/// could not hold (nothing writes one: `topic_attributes` refuses it) cannot reach a
/// delivered notification.
#[must_use]
pub fn signature_version(topic: Option<&TopicRecord>) -> &'static str {
    let asked = topic
        .and_then(|topic| {
            topic
                .attributes
                .get(super::topic_attributes::SIGNATURE_VERSION_ATTRIBUTE)
        })
        .map(String::as_str);
    match asked {
        Some(version) if version == SHA256_SIGNATURE_VERSION => SHA256_SIGNATURE_VERSION,
        _ => DEFAULT_SIGNATURE_VERSION,
    }
}

/// What signs the notifications of ONE topic for ONE request: the certificate URL a
/// subscriber fetches the verifying key from, and the digest that topic's
/// `SignatureVersion` selects.
///
/// Built per request because the certificate URL leads back to THIS emulator and is signed
/// for the caller that will fetch it; built per topic because the digest is the topic's own
/// setting.
#[derive(Debug, Clone)]
pub struct Signer {
    /// The URL the signing certificate is published at, or `None` when this process has no
    /// key — in which case nothing is signed and no signature member is delivered.
    certificate_url: Option<String>,
    /// The `SignatureVersion` the notifications sealed by this signer declare.
    version: &'static str,
    /// The last canonical string this signer signed, with its signature: one published
    /// message's canonical string is the same for every subscriber that receives it — the
    /// members it is built from are the MESSAGE's — so a fan-out signs it once rather than
    /// once per subscriber. The scheme is deterministic, so the held signature is the one
    /// signing again would produce.
    signed: ::std::cell::RefCell<Option<(String, String)>>,
}

impl Signer {
    /// The signer of `topic`'s notifications for the request `ctx` is serving.
    #[must_use]
    pub fn of_topic(
        ctx: &smithy_server_runtime::OperationContext<'_>,
        topic: Option<&TopicRecord>,
    ) -> Self {
        Self {
            certificate_url: certificate_url(ctx),
            version: signature_version(topic),
            signed: ::std::cell::RefCell::new(None),
        }
    }

    /// Write the three signature members of `envelope` — the version, the signature of the
    /// canonical string, and the URL of the certificate that verifies it under
    /// `certificate_member` — over the members the envelope already carries.
    ///
    /// Called with the envelope COMPLETE but for these members, because the canonical string
    /// is built from what is delivered: the signature a subscriber rebuilds is over the
    /// values it received.
    ///
    /// Writes nothing at all when there is no key to sign with, so an unsigned notification
    /// claims no signature.
    pub fn seal(
        &self,
        envelope: &mut ::serde_json::Map<String, ::serde_json::Value>,
        certificate_member: &str,
    ) {
        let Some(url) = self.certificate_url.as_deref() else {
            return;
        };
        let Some(signature) = self.signature(&canonical_string(envelope)) else {
            ::tracing::warn!(
                "the notification could not be signed, so it is delivered with no signature"
            );
            return;
        };
        envelope.insert(
            SIGNATURE_VERSION_MEMBER.to_string(),
            ::serde_json::Value::from(self.version),
        );
        envelope.insert(
            SIGNATURE_MEMBER.to_string(),
            ::serde_json::Value::from(signature),
        );
        envelope.insert(
            certificate_member.to_string(),
            ::serde_json::Value::from(url),
        );
    }

    /// The base64 signature of `canonical` under this signer's version, taken from the last
    /// one when this signer has already signed that same string ([`Signer::signed`]).
    fn signature(&self, canonical: &str) -> Option<String> {
        let held = self.signed.borrow().as_ref().and_then(|(signed, signature)| {
            (signed == canonical).then(|| signature.clone())
        });
        if let Some(signature) = held {
            return Some(signature);
        }
        let signature = signing_key()?.signature(self.version, canonical.as_bytes())?;
        let signature = ::aws_smithy_types::base64::encode(&signature);
        *self.signed.borrow_mut() = Some((canonical.to_string(), signature.clone()));
        Some(signature)
    }
}

/// The string the signature is over: each signed member of `envelope` that is present, as
/// its name, a newline, its value and a newline, in [`SIGNED_MEMBERS`] order.
fn canonical_string(envelope: &::serde_json::Map<String, ::serde_json::Value>) -> String {
    let mut canonical = String::new();
    for member in SIGNED_MEMBERS {
        let Some(value) = envelope.get(*member).and_then(::serde_json::Value::as_str) else {
            continue;
        };
        canonical.push_str(member);
        canonical.push('\n');
        canonical.push_str(value);
        canonical.push('\n');
    }
    canonical
}

/// Where the signing certificate is published for the caller of the request `ctx` is
/// serving: the path the certificate is served at on this emulator, signed so that fetching
/// it reaches this emulator and nothing else.
///
/// `None` when this process has no key — there is then no certificate to publish — or when
/// the URL cannot be signed, in which case a URL is withheld rather than one being delivered
/// that leads nowhere.
fn certificate_url(ctx: &smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    let path = signing_key()?.certificate_path();
    let request = ::smithy_server_runtime::SignedUrlRequest::new(
        ::http::Method::GET,
        ctx.identity().signing_name(),
        &path,
    );
    let signed = ctx
        .url_signer()
        .sign(request, ::smithy_server_runtime::MAX_SIGNED_URL_EXPIRY)
        .ok()?;
    Some(signed.as_str().to_string())
}

/// Whether `method` and `uri` address the signing certificate this service publishes, so
/// the request is answered with that document rather than read as a modelled action.
#[must_use]
pub fn addresses_certificate(method: &str, uri: &str) -> bool {
    method.eq_ignore_ascii_case("GET")
        && path_of(uri).starts_with(CERTIFICATE_FILE_PREFIX)
        && path_of(uri).ends_with(CERTIFICATE_FILE_SUFFIX)
}

/// The certificate published at `uri`, or `None` when it names no certificate this process
/// holds — a URL minted by an earlier run of the emulator, say.
#[must_use]
pub fn published_certificate(uri: &str) -> Option<&'static str> {
    let key = signing_key()?;
    (key.certificate_path() == path_of(uri)).then_some(key.certificate_pem.as_str())
}

/// The path part of `uri`, which is what names the document: a URL that leads here carries
/// the signature it was signed with in its query.
fn path_of(uri: &str) -> &str {
    uri.split('?').next().unwrap_or(uri)
}

/// Generate this process's signing key NOW, so no request pays for it.
///
/// RSA key generation is CPU-bound work of the SERVICE rather than of any caller: the key
/// exists before the first notification is published, the way AWS's own signing key does.
/// Called where the service is mounted; it generates at most one key per process, so
/// mounting the service again costs nothing.
pub fn prepare_signing_key() {
    let _ = signing_key();
}

/// The key pair this process signs its notifications with, generated on first use.
///
/// One per process: a subscriber reads WHICH key from the certificate URL the notification
/// carries, so one key published at one URL is what lets every delivered notification be
/// verified. `None` when the key or its certificate could not be produced, which leaves
/// notifications unsigned rather than claiming a signature nothing made.
fn signing_key() -> Option<&'static SigningKey> {
    static KEY: OnceLock<Option<SigningKey>> = OnceLock::new();
    KEY.get_or_init(SigningKey::generate).as_ref()
}

/// The key pair a notification is signed with, and the certificate that verifies it.
struct SigningKey {
    /// The private key, as the PKCS#8 `PrivateKeyInfo` the signer reads it back from.
    private_key_pkcs8: Vec<u8>,
    /// The same private key as the numbers the SHA-1 signature operation works on, read once
    /// when the key was generated rather than per signature.
    private_key_parts: PrivateKeyParts,
    /// The self-signed X.509 certificate over its public half, PEM encoded — the document
    /// published at the certificate URL.
    certificate_pem: String,
    /// The hex id the certificate's file name carries, which is the digest of that
    /// certificate.
    certificate_id: String,
}

/// The parts of the private key the RSA private-key operation of RFC 8017 section 5.1.2
/// needs, as the numbers it works on: the two primes of the modulus, the exponent of each,
/// and the inverse of the second prime modulo the first — the quintuple PKCS#1 keeps for
/// exactly that operation — beside the modulus whose width the signature takes.
struct PrivateKeyParts {
    /// How many octets the modulus, and so the signature, is long.
    modulus_octets: usize,
    /// `p`, the first prime of the modulus.
    prime1: ::num_bigint::BigUint,
    /// `q`, the second prime.
    prime2: ::num_bigint::BigUint,
    /// `dP`, the private exponent modulo `p - 1`.
    exponent1: ::num_bigint::BigUint,
    /// `dQ`, the private exponent modulo `q - 1`.
    exponent2: ::num_bigint::BigUint,
    /// `qInv`, the inverse of `q` modulo `p`.
    coefficient: ::num_bigint::BigUint,
}

/// Prints neither the private key nor the certificate: this value sits behind a shared
/// handle, and a private key must not reach a log through a formatter.
impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("certificate_id", &self.certificate_id)
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    /// A fresh 2048-bit RSA key pair and the self-signed certificate over its public half.
    ///
    /// RSA because the signature a subscriber verifies is `SHA1withRSA` / `SHA256withRSA`,
    /// and 2048 bits because that is the modulus AWS's own signing certificate carries —
    /// which is also the shortest modulus the documented verification algorithms accept.
    fn generate() -> Option<Self> {
        let key = ::aws_lc_rs::signature::RsaKeyPair::generate(
            ::aws_lc_rs::rsa::KeySize::Rsa2048,
        )
        .ok()?;
        let private_key_pkcs8: ::aws_lc_rs::encoding::Pkcs8V1Der<'_> =
            ::aws_lc_rs::encoding::AsDer::as_der(&key).ok()?;
        let private_key_pkcs8 = private_key_pkcs8.as_ref().to_vec();
        let private_key_parts = PrivateKeyParts::of(&private_key_pkcs8)?;
        let certificate_pem = self_signed_certificate(&private_key_pkcs8)?;
        let certificate_id = certificate_id(&certificate_pem);
        Some(Self {
            private_key_pkcs8,
            private_key_parts,
            certificate_pem,
            certificate_id,
        })
    }

    /// The path this key's certificate is published at:
    /// `/SimpleNotificationService-<id>.pem`.
    fn certificate_path(&self) -> String {
        format!(
            "{CERTIFICATE_FILE_PREFIX}{}{CERTIFICATE_FILE_SUFFIX}",
            self.certificate_id
        )
    }

    /// The signature of `message` under `version`: the RSASSA-PKCS1-v1_5 signature over the
    /// SHA-256 digest for [`SHA256_SIGNATURE_VERSION`], and over the SHA-1 digest for the
    /// default version — the two digests `SignatureVersion` selects between.
    fn signature(&self, version: &str, message: &[u8]) -> Option<Vec<u8>> {
        if version == SHA256_SIGNATURE_VERSION {
            return self.sha256_signature(message);
        }
        self.sha1_signature(message)
    }

    /// The `SHA256withRSA` signature of `message`, made by the cryptography provider that
    /// implements that scheme.
    fn sha256_signature(&self, message: &[u8]) -> Option<Vec<u8>> {
        let key =
            ::aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&self.private_key_pkcs8).ok()?;
        // A PKCS #1 signature is exactly as long as the key's modulus.
        let mut signature = vec![0u8; key.public_modulus_len()];
        key.sign(
            &::aws_lc_rs::signature::RSA_PKCS1_SHA256,
            &::aws_lc_rs::rand::SystemRandom::new(),
            message,
            &mut signature,
        )
        .ok()?;
        Some(signature)
    }

    /// The `SHA1withRSA` signature of `message`: the RSA private-key operation of RFC 8017
    /// section 5.1.2 over the EMSA-PKCS1-v1_5 encoding of the message's SHA-1 digest
    /// (section 9.2).
    ///
    /// Composed here rather than asked of the provider because no RSA implementation this
    /// workspace builds signs with SHA-1 — `aws-lc-rs` and `ring` both offer that digest for
    /// VERIFICATION only (`RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY`) and refuse to sign
    /// with it — while SNS's default signature version is defined in terms of it. The digest
    /// and the modular arithmetic are both the libraries' (`aws-lc-rs`, `num-bigint`); what is
    /// written out is only the padding layout and the exponentiation the standard fixes.
    fn sha1_signature(&self, message: &[u8]) -> Option<Vec<u8>> {
        let parts = &self.private_key_parts;
        let digest = ::aws_lc_rs::digest::digest(
            &::aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY,
            message,
        );
        let mut digest_info = SHA1_DIGEST_INFO_PREFIX.to_vec();
        digest_info.extend_from_slice(digest.as_ref());
        // EM = 0x00 || 0x01 || PS || 0x00 || T, with PS the 0xff octets that pad the
        // encoding out to the modulus (at least eight of them, RFC 8017 section 9.2).
        let padding_octets = parts
            .modulus_octets
            .checked_sub(digest_info.len() + 3)
            .filter(|padding| *padding >= 8)?;
        let mut encoded = vec![0x00, 0x01];
        encoded.resize(2 + padding_octets, 0xff);
        encoded.push(0x00);
        encoded.extend_from_slice(&digest_info);
        let signature = parts.signed(&::num_bigint::BigUint::from_bytes_be(&encoded))?;
        // The signature is exactly as long as the modulus, so a representative with leading
        // zero octets keeps them.
        let signature = signature.to_bytes_be();
        let mut padded = vec![0u8; parts.modulus_octets.checked_sub(signature.len())?];
        padded.extend_from_slice(&signature);
        Some(padded)
    }
}

impl PrivateKeyParts {
    /// The parts of the private key `private_key_pkcs8`, read out of the PKCS#8 structure the
    /// key was generated as: its `privateKey` octet string IS the PKCS#1 `RSAPrivateKey`
    /// (RFC 5958 section 2), so each number is read rather than rebuilt.
    fn of(private_key_pkcs8: &[u8]) -> Option<Self> {
        let info = ::pkcs8::PrivateKeyInfo::try_from(private_key_pkcs8).ok()?;
        let key = ::pkcs1::RsaPrivateKey::from_der(info.private_key).ok()?;
        let number = |field: ::pkcs1::UintRef<'_>| {
            ::num_bigint::BigUint::from_bytes_be(field.as_bytes())
        };
        Some(Self {
            modulus_octets: key.modulus.as_bytes().len(),
            prime1: number(key.prime1),
            prime2: number(key.prime2),
            exponent1: number(key.exponent1),
            exponent2: number(key.exponent2),
            coefficient: number(key.coefficient),
        })
    }

    /// `message` raised to the private exponent modulo the modulus — the signature
    /// representative of RFC 8017 section 5.1.2, computed the way that section's second form
    /// does, through the Chinese remainder theorem over the two primes.
    ///
    /// The CRT form is the one the standard states for a key that carries the quintuple, and
    /// it is also what keeps a signature cheap: two exponentiations modulo a prime half the
    /// modulus's width cost a fraction of one modulo the whole of it, and a notification is
    /// signed on the path that delivers it.
    fn signed(&self, message: &::num_bigint::BigUint) -> Option<::num_bigint::BigUint> {
        let first = message.modpow(&self.exponent1, &self.prime1);
        let second = message.modpow(&self.exponent2, &self.prime2);
        // h = qInv * (s1 - s2) mod p, with the difference taken in the positive residues of
        // p so nothing underflows.
        let difference = (&first + &self.prime1 - (&second % &self.prime1)) % &self.prime1;
        let factor = (&self.coefficient * difference) % &self.prime1;
        Some(second + &self.prime2 * factor)
    }
}

/// The self-signed X.509 certificate over the public half of the RSA key `private_key_pkcs8`,
/// PEM encoded: an end-entity certificate issued to [`CERTIFICATE_SUBJECT`], valid for
/// [`CERTIFICATE_VALIDITY`] from now, whose own signature is made with that same key.
fn self_signed_certificate(private_key_pkcs8: &[u8]) -> Option<String> {
    let key = ::rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
        &::rustls_pki_types::PrivatePkcs8KeyDer::from(private_key_pkcs8),
        &::rcgen::PKCS_RSA_SHA256,
    )
    .ok()?;
    // No subject alternative name: the certificate certifies the SIGNING IDENTITY of the
    // service rather than a host a client connects to.
    let mut params = ::rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, CERTIFICATE_SUBJECT);
    params.distinguished_name = subject;
    let issued_at = std::time::SystemTime::now();
    params.not_before = offset_date_time(issued_at)?;
    params.not_after = offset_date_time(issued_at.checked_add(CERTIFICATE_VALIDITY)?)?;
    // An end-entity certificate, explicitly not a certificate authority: it verifies the
    // notifications this service signs and certifies no other certificate.
    params.is_ca = ::rcgen::IsCa::ExplicitNoCa;
    params.key_usages = vec![::rcgen::KeyUsagePurpose::DigitalSignature];
    Some(params.self_signed(&key).ok()?.pem())
}

/// `at` as the date-time type the certificate generator takes a validity bound in, at the
/// one-second resolution an X.509 validity period is stated at.
fn offset_date_time(at: std::time::SystemTime) -> Option<::time::OffsetDateTime> {
    let seconds = at
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_secs()).ok())?;
    ::time::OffsetDateTime::from_unix_timestamp(seconds).ok()
}

/// The id the certificate's file name carries: the leading
/// [`CERTIFICATE_ID_HEX_CHARS`] hex characters of the SHA-256 digest of the published
/// certificate, so the URL names the document it serves.
fn certificate_id(certificate_pem: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(certificate_pem.as_bytes());
    let mut id = ::hex::encode(digest);
    id.truncate(CERTIFICATE_ID_HEX_CHARS);
    id
}
