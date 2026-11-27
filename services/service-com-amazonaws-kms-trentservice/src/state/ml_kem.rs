// SPDX-License-Identifier: CC0-1.0

//! The key-encapsulation half of this service: the ML-KEM key pair behind a KMS key
//! whose `KeyUsage` is `ENCAPSULATE_DECAPSULATE`, the two directions of the mechanism
//! `Encapsulate` and `Decapsulate` drive, and the ML-KEM DATA key pair
//! `GenerateDataKeyPair` hands a caller.
//!
//! A KMS key of an `ML_KEM_*` key spec holds a DECAPSULATION key — the private half,
//! which never leaves this process. Its ENCAPSULATION key (the public half) is derived
//! from that key on demand, so the two cannot drift apart, and is what `GetPublicKey`
//! answers with, wrapped in the X.509 `SubjectPublicKeyInfo` every public key of this
//! service is reported in. A DATA key pair of the same spec is generated per request and
//! is the caller's: both halves are reported, the private one in the PKCS#8
//! `PrivateKeyInfo` every other data key pair's private half is reported in.
//!
//! Both directions come from `aws-lc-rs` (`aws_lc_rs::kem`), the library this crate
//! already generates every other key pair with: this file composes the KMS surface over
//! the mechanism, it implements no cryptography. That is also what gives the mechanism's
//! own IMPLICIT REJECTION — decapsulating a ciphertext made under another key of the same
//! parameter set succeeds and yields an unrelated shared secret rather than failing,
//! which is the behaviour FIPS 203 specifies — while a byte string that is not a
//! ciphertext of this parameter set at all is refused.

use super::*;

/// The `EncapsulationAlgorithm` every `ML_KEM_*` key spec encapsulates under
/// (`smithy_query(shape="EncapsulationAlgorithmSpec")`): the mechanism of FIPS 203, whose
/// parameter set is the one the key's own spec names.
pub const ENCAPSULATION_ALGORITHM_ML_KEM: &str = "ML_KEM";

/// The lowest-strength ML-KEM key spec (`smithy_query(shape="KeySpec")`).
pub const KEY_SPEC_ML_KEM_512: &str = "ML_KEM_512";
/// See [`KEY_SPEC_ML_KEM_512`].
pub const KEY_SPEC_ML_KEM_768: &str = "ML_KEM_768";
/// See [`KEY_SPEC_ML_KEM_512`].
pub const KEY_SPEC_ML_KEM_1024: &str = "ML_KEM_1024";

/// One ML-KEM key spec: the parameter set it names, the object identifier an X.509
/// `SubjectPublicKeyInfo` names that parameter set by, and the size of its ciphertext.
struct MlKemSpec {
    key_spec: &'static str,
    algorithm: &'static ::aws_lc_rs::kem::Algorithm<::aws_lc_rs::kem::AlgorithmId>,
    /// The NIST Computer Security Objects Register's `id-alg-ml-kem-512`, `-768` and
    /// `-1024`, under the `2.16.840.1.101.3.4.4` arc.
    object_identifier: &'static str,
    /// How many bytes a ciphertext of this parameter set is, from FIPS 203 Table 3
    /// ("Sizes (in bytes) of keys and ciphertexts of ML-KEM"): 768 for ML-KEM-512, 1088
    /// for ML-KEM-768 and 1568 for ML-KEM-1024. A byte string of any other length is not
    /// a ciphertext of the parameter set at all, which is what a caller is told.
    ciphertext_len: usize,
}

/// Every `KeySpec` whose key material is an ML-KEM key pair. A spec missing from this
/// table is not one this service encapsulates under.
const ML_KEM_SPECS: &[MlKemSpec] = &[
    MlKemSpec {
        key_spec: KEY_SPEC_ML_KEM_512,
        algorithm: &::aws_lc_rs::kem::ML_KEM_512,
        object_identifier: "2.16.840.1.101.3.4.4.1",
        ciphertext_len: 768,
    },
    MlKemSpec {
        key_spec: KEY_SPEC_ML_KEM_768,
        algorithm: &::aws_lc_rs::kem::ML_KEM_768,
        object_identifier: "2.16.840.1.101.3.4.4.2",
        ciphertext_len: 1088,
    },
    MlKemSpec {
        key_spec: KEY_SPEC_ML_KEM_1024,
        algorithm: &::aws_lc_rs::kem::ML_KEM_1024,
        object_identifier: "2.16.840.1.101.3.4.4.3",
        ciphertext_len: 1568,
    },
];

/// The parameter set `key_spec` names, or `None` when it names no ML-KEM key pair.
fn ml_kem_spec(key_spec: &str) -> Option<&'static MlKemSpec> {
    ML_KEM_SPECS.iter().find(|spec| spec.key_spec == key_spec)
}

/// One ML-KEM key pair: the DECAPSULATION key, together with the key spec whose parameter
/// set it belongs to.
///
/// Two kinds of pair are the same mechanism, so both are this one type: the pair behind a
/// KMS key whose `KeyUsage` is `ENCAPSULATE_DECAPSULATE`, which the store keeps, and the
/// DATA key pair `GenerateDataKeyPair` generates for one request and hands to the caller.
///
/// What may leave the struct differs by what the caller asked for, not by this type: the
/// public encapsulation key and the two directions of the mechanism are reachable for
/// either, while [`Self::private_key_pkcs8`] is reached only from the data key pair's
/// generator ([`generate_key_pair`]) — the private half of a KMS KEY "never leaves KMS
/// unencrypted" (`smithy_query(query="doc",shape="CreateKey")`), and no operation on a
/// stored key reads it.
pub struct MlKemKeyPair {
    /// The parameter set this pair belongs to, resolved once when the pair is generated,
    /// so neither the key spec it reports nor the lengths and identifier that follow from
    /// it can be absent later.
    spec: &'static MlKemSpec,
    /// The private half, held as the library's own key object rather than as bytes.
    decapsulation_key: ::aws_lc_rs::kem::DecapsulationKey<::aws_lc_rs::kem::AlgorithmId>,
}

/// The private half never appears: the key spec identifies the pair without disclosing
/// anything about its material.
impl std::fmt::Debug for MlKemKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MlKemKeyPair")
            .field("key_spec", &self.spec.key_spec)
            .field("decapsulation_key", &"<redacted>")
            .finish()
    }
}

impl MlKemKeyPair {
    /// A freshly generated key pair of the parameter set `key_spec` names, drawn from the
    /// library's own CSPRNG.
    ///
    /// `None` means `key_spec` names no ML-KEM key pair, or the library refused to
    /// produce one — in which case the caller reports a server error rather than a key
    /// whose material nothing generated.
    pub fn generate(key_spec: &str) -> Option<Self> {
        let spec = ml_kem_spec(key_spec)?;
        Some(Self {
            spec,
            decapsulation_key: ::aws_lc_rs::kem::DecapsulationKey::generate(spec.algorithm).ok()?,
        })
    }

    /// How many bytes a ciphertext this pair decapsulates is: the one of its parameter
    /// set, which is what makes a blob of any other length no ciphertext of this key.
    pub fn ciphertext_len(&self) -> usize {
        self.spec.ciphertext_len
    }

    /// The raw ENCAPSULATION key: the public half, as FIPS 203 encodes it.
    fn encapsulation_key_bytes(&self) -> Option<Vec<u8>> {
        let encapsulation_key = self.decapsulation_key.encapsulation_key().ok()?;
        Some(encapsulation_key.key_bytes().ok()?.as_ref().to_vec())
    }

    /// The PUBLIC half of this pair as a DER-encoded X.509 `SubjectPublicKeyInfo`, which
    /// is the encoding `PublicKeyType` carries — the same encoding every other public key
    /// of this service is downloaded in.
    ///
    /// The algorithm is named by the identifier registered for the pair's parameter set,
    /// with no parameters, and the key itself is the raw encapsulation key, which is how
    /// an ML-KEM public key is carried in that structure. The structure is composed here
    /// rather than exported by `aws-lc-rs` the way every other public half of this crate
    /// is ([`public_key_spki`]): that library's `kem::EncapsulationKey` implements no DER
    /// export (it answers the raw key alone), so the structure around it comes from the
    /// maintained SPKI crate and the identifier from the table above.
    ///
    /// `None` means the library refused to export the public half or the structure could
    /// not be encoded.
    pub fn public_key_spki(&self) -> Option<Vec<u8>> {
        use ::spki::der::Encode;

        let encapsulation_key = self.encapsulation_key_bytes()?;
        let spki = ::spki::SubjectPublicKeyInfoOwned {
            algorithm: ::spki::AlgorithmIdentifierOwned {
                oid: ::spki::ObjectIdentifier::new(self.spec.object_identifier).ok()?,
                parameters: None,
            },
            subject_public_key: ::spki::der::asn1::BitString::from_bytes(&encapsulation_key).ok()?,
        };
        spki.to_der().ok()
    }

    /// The PRIVATE half of this pair as a DER-encoded PKCS#8 `PrivateKeyInfo`, which is
    /// the encoding `PlaintextType` carries a DATA key pair's private half in — the same
    /// encoding every other key pair of this service is held and reported in.
    ///
    /// Reached only from [`generate_key_pair`], for the pair `GenerateDataKeyPair`
    /// generates for one request: a KMS key's own private half is never read out.
    ///
    /// The algorithm is named by the identifier registered for the pair's parameter set,
    /// with no parameters, and `privateKey` carries the `expandedKey` alternative of
    /// `ML-KEM-PrivateKey`: the decapsulation key as FIPS 203 encodes it, inside an OCTET
    /// STRING (draft-ietf-lamps-kyber-certificates). The structures come from the
    /// maintained PKCS#8 and DER crates rather than from bytes laid out here, because
    /// `aws-lc-rs`'s `kem::DecapsulationKey` implements no DER export.
    ///
    /// `None` means the library refused to export the private half, or the structure
    /// could not be encoded.
    pub fn private_key_pkcs8(&self) -> Option<Vec<u8>> {
        use ::spki::der::{asn1::OctetStringRef, Encode};

        let decapsulation_key = self.decapsulation_key.key_bytes().ok()?;
        let expanded_key = OctetStringRef::new(decapsulation_key.as_ref())
            .ok()?
            .to_der()
            .ok()?;
        let info = ::pkcs8::PrivateKeyInfoRef {
            algorithm: ::spki::AlgorithmIdentifierRef {
                oid: ::spki::ObjectIdentifier::new(self.spec.object_identifier).ok()?,
                parameters: None,
            },
            private_key: OctetStringRef::new(&expanded_key).ok()?,
            public_key: None,
        };
        info.to_der().ok()
    }

    /// One run of the ENCAPSULATION direction: a fresh shared secret and the ciphertext a
    /// holder of this key recovers it from, in that order.
    ///
    /// Each call draws its own randomness, so two encapsulations under one key yield
    /// different secrets and different ciphertexts — the mechanism's own property, and
    /// what makes this a key ESTABLISHMENT step rather than a lookup.
    ///
    /// `None` means the library refused to encapsulate.
    pub fn encapsulate(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let encapsulation_key = self.decapsulation_key.encapsulation_key().ok()?;
        let (ciphertext, shared_secret) = encapsulation_key.encapsulate().ok()?;
        Some((shared_secret.as_ref().to_vec(), ciphertext.as_ref().to_vec()))
    }

    /// The DECAPSULATION direction: the shared secret `ciphertext` yields under this
    /// key's private half.
    ///
    /// A ciphertext this key's own [`Self::encapsulate`] produced yields exactly the
    /// secret that call produced. A ciphertext of the same parameter set made under
    /// ANOTHER key yields an unrelated secret rather than an error — the implicit
    /// rejection FIPS 203 specifies, which is why nothing here compares the ciphertext to
    /// anything first.
    ///
    /// `None` means the bytes are not a ciphertext of this key's parameter set at all,
    /// which the caller refuses as an invalid ciphertext.
    pub fn decapsulate(&self, ciphertext: &[u8]) -> Option<Vec<u8>> {
        let shared_secret = self
            .decapsulation_key
            .decapsulate(::aws_lc_rs::kem::Ciphertext::from(ciphertext))
            .ok()?;
        Some(shared_secret.as_ref().to_vec())
    }

    /// How KMS refuses a blob that is not a ciphertext of this pair's parameter set: the
    /// member it arrived in, the key spec whose parameter set decides the length, the
    /// length that parameter set requires and the length the request carried.
    ///
    /// The wording is KMS's own for this class — unlike the service's OTHER
    /// invalid-ciphertext refusal (`Decrypt`), which carries no message at all — so the
    /// fixed text is reproduced as the service states it and every value in it is read
    /// from this key and this request.
    pub fn ciphertext_length_mismatch_message(&self, submitted: usize) -> String {
        format!(
            "SharedSecretCiphertextBlob length mismatch for {} KeySpec: expected {} bytes, \
             got {submitted} bytes.",
            self.spec.key_spec,
            self.ciphertext_len()
        )
    }

    /// A value identifying this pair to THIS store, derived from the PUBLIC half so the
    /// private key is never fed to anything but the mechanism — what
    /// [`KeyMaterial::digest`] reports for key material of this kind.
    pub fn digest(&self) -> Option<String> {
        Some(sha256_hex(&[
            b"kms-key-material\0ml-kem\0",
            self.spec.key_spec.as_bytes(),
            b"\0",
            &self.encapsulation_key_bytes()?,
        ]))
    }
}
