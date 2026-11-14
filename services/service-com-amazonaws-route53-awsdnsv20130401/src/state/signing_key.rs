// SPDX-License-Identifier: CC0-1.0

//! The DNSSEC key material a key-signing key (KSK) publishes, computed from the
//! customer managed key the KSK was created from.
//!
//! A KSK "is a complex type that represents a public/private key pair"
//! (`KeySigningKey`) whose private half stays in Key Management Service: Route 53
//! holds only the ARN of that customer managed key and derives everything a
//! resolver needs from its PUBLIC half. So every value a reported KSK carries
//! beside its name and status — the base64 public key, the key tag, the
//! delegation signer digest, and the DS and DNSKEY record text a zone's parent is
//! given — is COMPUTED here from the public key of the KMS key the request named,
//! never minted beside it. Two KSKs over the same KMS key would publish the same
//! DNSKEY, which is why `KeySigningKey$KmsArn` must be unique within a zone.
//!
//! The public half is a fact of the KMS store — the one `GetPublicKey` answers
//! from — reached through the shared engine, exactly as the CloudWatch alarm a
//! health check monitors ([`super::monitored_alarm`]) and the VPCs the account
//! holds ([`super::account_vpcs`]) are. An ARN that names no usable
//! sign-and-verify `ECC_NIST_P256` key of that store is an ARN Route 53 cannot
//! sign with, which is what the modelled `InvalidKMSArn` reports: nothing here
//! invents key material for a key that is not there. That store's lock is taken
//! for this one read and released before returning, and a handler calls this with
//! no lock of its own held, so the two stores are never locked at once.

use ::service_com_amazonaws_kms_trentservice as kms;

use crate::ops::types::shared_types::{InvalidKMSArn, NoSuchKeySigningKey};

/// The modelled `InvalidKMSArn` (HTTP 400) for a `KeyManagementServiceArn` this
/// service cannot sign a zone with.
///
/// The ONE place this service words that refusal, so every operation that
/// resolves a customer managed key answers an unusable one identically. The
/// message is REQUEST-DERIVED: it quotes the ARN that arrived and then states
/// what to check about that key — whether it still exists, and whether its
/// configuration and key policy meet this service's DNSSEC requirements —
/// because the conditions behind the refusal (a deleted key, a key of another
/// spec or usage, a disabled key, a key policy that withholds the signing
/// permissions) are all facts of the key the caller named and are not
/// distinguishable from one another by the caller without being told which key
/// was rejected.
pub fn invalid_kms_arn(kms_arn: &str) -> InvalidKMSArn {
    InvalidKMSArn {
        message: Some(format!(
            "The customer managed key with ARN '{kms_arn}' could not be used by Route 53 DNSSEC. \
             Check to make sure that the customer managed key has not been deleted. Otherwise, \
             review its configurations and key policy, to make sure it is in compliance with \
             Route 53 DNSSEC requirements."
        )),
    }
}

/// The modelled `NoSuchKeySigningKey` (HTTP 404) for a name that names no
/// key-signing key of the addressed hosted zone.
///
/// The ONE place this service words that refusal, so every key-addressed
/// operation — the activate, the deactivate and the delete, each of which
/// addresses a key by `(HostedZoneId, Name)` — answers a key it does not hold
/// identically. REQUEST-DERIVED: it quotes the `Name` the request asked about,
/// which is the half of the address a caller driving several keys cannot infer
/// from the error class alone.
pub fn no_such_key_signing_key(name: &str) -> NoSuchKeySigningKey {
    NoSuchKeySigningKey {
        message: Some(format!(
            "The Key Signing Key with name '{name}', could not be found."
        )),
    }
}

/// The refusal of a key-signing key status that is not one this API accepts, in
/// `InvalidKeySigningKeyStatus`'s own wording: "The key-signing key (KSK) status
/// isn't valid or another KSK has the status INTERNAL_FAILURE."
///
/// One sentence for the whole service, so the operation that refuses the status a
/// request ASKED for and the one that refuses a zone holding a key left in error
/// cannot word the same modelled refusal differently.
pub const INVALID_KEY_SIGNING_KEY_STATUS_MESSAGE: &str =
    "The key-signing key (KSK) status isn't valid or another KSK has the status INTERNAL_FAILURE.";

/// The key spec the customer managed key behind a KSK must have.
///
/// `KeySigningKey$KmsArn`'s own documentation lists what the key has to be
/// configured as: "Status Enabled", "Key spec ECC_NIST_P256", "Key usage Sign and
/// verify". That spec is a NIST P-256 key pair, which is what makes the signing
/// algorithm below the one Route 53 signs the zone with.
pub const REQUIRED_KEY_SPEC: &str = "ECC_NIST_P256";

/// The `Flag` of a key-signing key.
///
/// `KeySigningKey$Flag`: "An integer that specifies how the key is used. For
/// key-signing key (KSK), this value is always 257." It is also the DNSKEY flags
/// field of the published record (Zone Key plus Secure Entry Point, RFC 4034
/// § 2.1.1 and RFC 3757), so the same value is reported and published.
pub const KEY_SIGNING_KEY_FLAG: u16 = 257;

/// The DNSKEY protocol octet, which "MUST have value 3" (RFC 4034 § 2.1.2).
const DNSKEY_PROTOCOL: u8 = 3;

/// The signing algorithm a KSK over a NIST P-256 key pair publishes, as
/// `KeySigningKey$SigningAlgorithmType` numbers it.
///
/// The member's documentation points at RFC 8624 § 3.1, whose ECDSAP256SHA256 is
/// algorithm number 13 (RFC 6605 § 5), the signature algorithm of a P-256 key.
pub const SIGNING_ALGORITHM_TYPE: u8 = 13;

/// The same algorithm as `KeySigningKey$SigningAlgorithmMnemonic` spells it: the
/// mnemonic RFC 8624 § 3.1 lists beside algorithm number 13.
pub const SIGNING_ALGORITHM_MNEMONIC: &str = "ECDSAP256SHA256";

/// The delegation signer digest algorithm, as
/// `KeySigningKey$DigestAlgorithmType` numbers it.
///
/// That member's documentation points at RFC 8624 § 3.3, whose SHA-256 is digest
/// algorithm number 2 (RFC 4509).
pub const DIGEST_ALGORITHM_TYPE: u8 = 2;

/// The same digest as `KeySigningKey$DigestAlgorithmMnemonic` spells it: the
/// mnemonic RFC 8624 § 3.3 lists beside digest algorithm number 2.
pub const DIGEST_ALGORITHM_MNEMONIC: &str = "SHA-256";

/// The leading octet of an uncompressed elliptic-curve point (SEC 1 § 2.3.3),
/// which is the form a NIST curve's `SubjectPublicKeyInfo` carries its point in.
const UNCOMPRESSED_POINT_TAG: u8 = 0x04;

/// How many octets the two coordinates of a P-256 point take: a 256-bit field
/// element each, which is the DNSKEY key material of an ECDSA P-256 key — "the
/// public key ... is the pair (x, y) ... each 32 octets long" (RFC 6605 § 4).
const COORDINATE_PAIR_OCTETS: usize = 64;

/// The `key/<key id>` prefix of a KMS key ARN's resource field.
const KEY_RESOURCE_PREFIX: &str = "key/";

/// Everything a reported key-signing key publishes about the key pair behind it,
/// computed from that key pair's public half.
#[derive(Debug, Clone)]
pub struct SigningKeyMaterial {
    /// `KeySigningKey$PublicKey`: "The public key, represented as a Base64
    /// encoding, as required by RFC-4034 Page 5" — the DNSKEY key material, which
    /// for this algorithm is the point's two coordinates.
    pub public_key: String,
    /// `KeySigningKey$KeyTag`: "An integer used to identify the DNSSEC record for
    /// the domain name", computed as RFC 4034 Appendix B describes.
    pub key_tag: u16,
    /// `KeySigningKey$DigestValue`: "A cryptographic digest of a DNSKEY resource
    /// record (RR)", in the upper-case hexadecimal a DS record is presented in.
    pub digest_value: String,
    /// `KeySigningKey$DSRecord`: the RDATA of the delegation signer record the
    /// zone's parent publishes — key tag, algorithm, digest type, digest
    /// (RFC 4034 § 5.1, § 5.3).
    pub ds_record: String,
    /// `KeySigningKey$DNSKEYRecord`: the RDATA of the DNSKEY record this zone
    /// publishes — flags, protocol, algorithm, key (RFC 4034 § 2.1, § 2.2).
    pub dnskey_record: String,
}

/// What the key-signing key over the customer managed key `kms_arn` publishes for
/// the zone named `zone_name`, or `None` when that ARN names no key this emulator
/// can sign a zone with.
///
/// `zone_name` is the zone's apex name: the DNSKEY record's owner name, which is
/// the first half of the delegation signer digest (RFC 4034 § 5.1.4).
pub fn signing_key_material<Eng: Send + Sync + 'static>(
    engine: &Eng,
    kms_arn: &str,
    zone_name: &str,
) -> Option<SigningKeyMaterial> {
    let public_key = signing_public_key(engine, kms_arn)?;
    Some(material_of(&public_key, zone_name))
}

/// [`SigningKeyMaterial`] over the DNSKEY key material `public_key`, for a DNSKEY
/// record owned by `zone_name`.
///
/// Separate from the lookup above so the computation is one step over bytes: the
/// DNSKEY RDATA is built once and the key tag, the digest and both record
/// renderings are read off that one RDATA, which is what keeps the DS record's
/// key tag and digest the ones the published DNSKEY actually hashes to.
fn material_of(public_key: &[u8], zone_name: &str) -> SigningKeyMaterial {
    let rdata = dnskey_rdata(public_key);
    let key_tag = key_tag(&rdata);
    let digest_value = delegation_signer_digest(zone_name, &rdata);
    let encoded_key = ::aws_smithy_types::base64::encode(public_key);
    SigningKeyMaterial {
        ds_record: format!(
            "{key_tag} {SIGNING_ALGORITHM_TYPE} {DIGEST_ALGORITHM_TYPE} {digest_value}"
        ),
        dnskey_record: format!(
            "{KEY_SIGNING_KEY_FLAG} {DNSKEY_PROTOCOL} {SIGNING_ALGORITHM_TYPE} {encoded_key}"
        ),
        public_key: encoded_key,
        key_tag,
        digest_value,
    }
}

/// The DNSKEY key material of the customer managed key `kms_arn`: the two
/// coordinates of the public point of the KMS key pair it names.
///
/// `None` when the ARN is not the ARN of a KMS key, when the KMS store holds no
/// key of that ARN's Region and id, when that key is not a usable
/// sign-and-verify [`REQUIRED_KEY_SPEC`] key, or when the store holds no key pair
/// for it — in each case this emulator has no public half to publish, which is
/// the ARN `InvalidKMSArn` is about.
fn signing_public_key<Eng: Send + Sync + 'static>(engine: &Eng, kms_arn: &str) -> Option<Vec<u8>> {
    let arn = ::service_common::arn::parse(kms_arn)?;
    if arn.vendor != kms::state::ARN_SERVICE {
        return None;
    }
    let key_id = arn.resource.strip_prefix(KEY_RESOURCE_PREFIX)?;
    let state = kms::state::service_state(engine)?;
    let store = state.lock();
    let key = store.key_in(arn.region, key_id)?;
    if key.key_spec != REQUIRED_KEY_SPEC
        || key.key_usage != kms::state::KEY_USAGE_SIGN_VERIFY
        || !key.is_usable()
    {
        return None;
    }
    dnskey_key_material(&key.public_key()?)
}

/// The DNSKEY key material inside the X.509 `SubjectPublicKeyInfo` `spki_der`:
/// the uncompressed point's coordinates, with the SEC 1 form octet dropped,
/// which is how RFC 6605 § 4 carries an ECDSA P-256 public key in a DNSKEY.
///
/// The structure is read by the maintained `SubjectPublicKeyInfo` decoder rather
/// than by slicing the encoding here. `None` when the bytes are not one, or not
/// an uncompressed point of the expected width.
fn dnskey_key_material(spki_der: &[u8]) -> Option<Vec<u8>> {
    use ::spki::der::Decode as _;

    let spki = ::spki::SubjectPublicKeyInfoRef::from_der(spki_der).ok()?;
    let (tag, coordinates) = spki.subject_public_key.as_bytes()?.split_first()?;
    if *tag != UNCOMPRESSED_POINT_TAG || coordinates.len() != COORDINATE_PAIR_OCTETS {
        return None;
    }
    Some(coordinates.to_vec())
}

/// The RDATA of the DNSKEY record a key-signing key publishes: flags, protocol,
/// algorithm, then the key material (RFC 4034 § 2.1).
fn dnskey_rdata(public_key: &[u8]) -> Vec<u8> {
    let mut rdata = Vec::with_capacity(4 + public_key.len());
    rdata.extend_from_slice(&KEY_SIGNING_KEY_FLAG.to_be_bytes());
    rdata.push(DNSKEY_PROTOCOL);
    rdata.push(SIGNING_ALGORITHM_TYPE);
    rdata.extend_from_slice(public_key);
    rdata
}

/// The key tag of the DNSKEY whose RDATA is `rdata`.
///
/// RFC 4034 Appendix B.1, which every algorithm other than number 1 uses: the
/// RDATA read as a sequence of big-endian 16-bit words (a trailing odd octet
/// taken as the high half of one), summed, with the carry out of the low 16 bits
/// folded back in. It is a checksum over the published key, so the DS record
/// below and the DNSKEY beside it carry the same tag.
fn key_tag(rdata: &[u8]) -> u16 {
    let mut accumulator: u32 = 0;
    for (index, octet) in rdata.iter().enumerate() {
        accumulator += if index % 2 == 0 {
            u32::from(*octet) << 8
        } else {
            u32::from(*octet)
        };
    }
    accumulator += (accumulator >> 16) & 0xffff;
    // The fold leaves a value inside 16 bits, so the conversion cannot fail.
    u16::try_from(accumulator & 0xffff).unwrap_or(0)
}

/// The delegation signer digest of the DNSKEY `rdata` owned by `owner_name`:
/// "digest = digest_algorithm( DNSKEY owner name | DNSKEY RDATA )"
/// (RFC 4034 § 5.1.4), over the canonical wire form of that name
/// ([`super::dns_wire_name`]).
///
/// Presented as upper-case hexadecimal, which is how a DS record's digest is
/// written in zone-file presentation format and what `KeySigningKey$DigestValue`
/// carries.
fn delegation_signer_digest(owner_name: &str, rdata: &[u8]) -> String {
    use ::sha2::Digest as _;

    let mut digest = ::sha2::Sha256::new();
    digest.update(super::dns_wire_name(owner_name));
    digest.update(rdata);
    ::hex::encode_upper(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published DNSKEY and its delegation signer digest, against an oracle
    /// laid out by hand from the RFCs rather than by this module.
    ///
    /// The digest is "digest_algorithm( DNSKEY owner name | DNSKEY RDATA )"
    /// (RFC 4034 § 5.1.4), the owner name in the canonical wire form of
    /// RFC 4034 § 6.2 (here `7example3net0`) and the RDATA the flags, protocol,
    /// algorithm and key of RFC 4034 § 2.1 (here `0x0101`, `3`, `13` and the
    /// 64-octet coordinate pair) — so this pins the layout, the order and the
    /// presentation the values are reported in, over a key whose every octet is
    /// known.
    #[test]
    fn the_digest_is_over_the_owner_name_and_the_dnskey_rdata() {
        use ::sha2::Digest as _;

        let public_key = [0x2bu8; COORDINATE_PAIR_OCTETS];
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x07example\x03net\x00");
        expected.extend_from_slice(&[0x01, 0x01, 0x03, 0x0d]);
        expected.extend_from_slice(&public_key);
        let expected: String = ::sha2::Sha256::digest(&expected)
            .iter()
            .map(|octet| format!("{octet:02X}"))
            .collect();

        let material = material_of(&public_key, "example.net.");
        assert_eq!(
            material.digest_value, expected,
            "expected the digest of the owner name and the DNSKEY RDATA ({expected}), got {}",
            material.digest_value
        );
        assert_eq!(
            material.ds_record,
            format!("{} 13 2 {expected}", material.key_tag),
            "the DS record carries that digest, got {:?}",
            material.ds_record
        );
    }

    /// The key tag of a DNSKEY is the folded sum of its RDATA's 16-bit words
    /// (RFC 4034 Appendix B.1), so a hand-computed RDATA and the function must
    /// agree — over both branches of the odd/even case and over a sum that
    /// carries out of 16 bits.
    #[test]
    fn key_tag_sums_the_rdata_as_sixteen_bit_words() {
        let cases: [(&[u8], u16); 4] = [
            (&[], 0),
            // One octet, at an even index: the high half of a word.
            (&[0x01], 0x0100),
            // Two words that cannot carry.
            (&[0x01, 0x02, 0x03, 0x04], 0x0406),
            // Two words whose sum exceeds 16 bits, so the carry folds back in.
            (&[0xff, 0xff, 0xff, 0xff], 0xffff),
        ];
        for (rdata, expected) in cases {
            let actual = key_tag(rdata);
            assert_eq!(
                actual, expected,
                "case {rdata:?}: expected key tag {expected}, got {actual}"
            );
        }
    }

    /// Every value a KSK publishes is read off the DNSKEY RDATA built from the
    /// key material, so the record renderings carry the tag, the algorithms and
    /// the key the rest of the structure reports.
    #[test]
    fn the_records_render_the_values_beside_them() {
        let public_key = [0x2au8; COORDINATE_PAIR_OCTETS];
        let material = material_of(&public_key, "example.com.");
        assert_eq!(
            material.ds_record,
            format!(
                "{} {SIGNING_ALGORITHM_TYPE} {DIGEST_ALGORITHM_TYPE} {}",
                material.key_tag, material.digest_value
            ),
            "the DS record must carry the key tag and digest reported beside it, \
             got {:?}",
            material.ds_record
        );
        assert_eq!(
            material.dnskey_record,
            format!(
                "{KEY_SIGNING_KEY_FLAG} {DNSKEY_PROTOCOL} {SIGNING_ALGORITHM_TYPE} {}",
                material.public_key
            ),
            "the DNSKEY record must carry the public key reported beside it, got {:?}",
            material.dnskey_record
        );
        assert_eq!(
            material.public_key,
            ::aws_smithy_types::base64::encode(&public_key[..]),
            "the reported public key is the base64 of the key material"
        );
    }

    /// The digest is over the OWNER NAME as well as the key, so the same key
    /// published in two zones gives two digests — and one spelling of a name
    /// gives one digest.
    #[test]
    fn the_digest_covers_the_owner_name() {
        let public_key = [0x7fu8; COORDINATE_PAIR_OCTETS];
        let first = material_of(&public_key, "example.com.");
        let same = material_of(&public_key, "EXAMPLE.com");
        let other = material_of(&public_key, "example.net.");
        assert_eq!(
            first.digest_value, same.digest_value,
            "one name spelled two ways is one owner name: got {:?} and {:?}",
            first.digest_value, same.digest_value
        );
        assert_ne!(
            first.digest_value, other.digest_value,
            "a different owner name gives a different digest, got {:?} twice",
            first.digest_value
        );
        assert_eq!(
            first.key_tag, other.key_tag,
            "the key tag is over the key alone, so it does not move with the zone"
        );
    }
}
