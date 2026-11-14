// SPDX-License-Identifier: CC0-1.0

//! The DKIM signature Amazon SES puts on a message it sends for an identity whose Easy DKIM
//! setup is in place, and the verdict the account's receive path reads off one.
//!
//! Easy DKIM is a fact about an IDENTITY of the account, not about the API that recorded it:
//! `GetIdentityDkimAttributes` reports `DkimEnabled` ("true if DKIM signing is enabled for
//! email sent from the identity"), `DkimVerificationStatus` ("describes whether Amazon SES has
//! successfully verified the DKIM DNS records (tokens) published in the domain name's DNS")
//! and `DkimTokens`, "a set of character strings that represent the domain's identity"; a
//! domain publishes each token as a `<token>._domainkey.<domain>` record, and that token is the
//! SELECTOR a signature for the domain names. So a message this account sends from an identity
//! whose signing is on and whose tokens are verified carries a `DKIM-Signature` naming that
//! domain and that selector, and a message from an identity without that setup carries none.
//!
//! The signature itself is REAL: the field is the one RFC 6376 §3.5 defines, its `bh=` is the
//! hash of the canonicalized body, its `b=` is a signature over the canonicalized signed
//! header fields, and the key pair is the identity's own — held with the account's identity
//! list and never served back to any caller, which is where the verification key of an Easy
//! DKIM identity lives at Amazon SES too (the records a domain publishes are aliases of
//! Amazon's own). The algorithm is the Edwards-curve one RFC 8463 registers for DKIM
//! (`ed25519-sha256`), computed by the maintained cryptography crate this workspace signs and
//! verifies with rather than by anything written here.
//!
//! That is what lets the receiving half REPORT on the signature rather than assert it: a
//! message the receive path holds is verified against the key the signing identity holds
//! ([`outcome`]), so the `dkim=` result an `Authentication-Results` field carries is the
//! outcome of a check over those bytes — a message whose body or signed headers changed on the
//! way does not pass, and one signed by a domain this account holds no key for reports that the
//! key could not be found rather than a pass.

use std::time::{SystemTime, UNIX_EPOCH};

use super::composed_message::CRLF;

/// `DKIM-Signature`, the header field RFC 6376 §3.5 carries a signature in.
pub const DKIM_SIGNATURE_HEADER: &str = "DKIM-Signature";

/// The signing algorithm a signature is made and read under: the Edwards-curve algorithm
/// RFC 8463 §3 registers for DKIM, whose keys are 32 bytes and whose hash is SHA-256.
pub const SIGNING_ALGORITHM: &str = "ed25519-sha256";

/// The canonicalization a signature is made under (RFC 6376 §3.4): `relaxed` for the header
/// fields (§3.4.2) and `relaxed` for the body (§3.4.4), the pair that survives the whitespace
/// and folding changes a message meets in transit.
const CANONICALIZATION: &str = "relaxed/relaxed";

/// The header fields a signature covers, in the order `h=` lists them.
///
/// The fields that say what the message IS and who it is from — which is what a receiver
/// checks a signature to learn — and no field a relay adds, because `h=` names fields the
/// signer wrote. A field the message does not carry is left out of the list rather than signed
/// as empty.
const SIGNED_FIELDS: &[&str] = &[
    "from",
    "to",
    "cc",
    "subject",
    "date",
    "message-id",
    "mime-version",
    "content-type",
];

/// How many bytes an Ed25519 signing key is seeded from (RFC 8032 §5.1.5).
pub const SIGNING_KEY_BYTES: usize = 32;

/// An identity's DKIM signing key.
///
/// Key MATERIAL, so it is held in a type whose `Debug` carries NONE of it: a derived one would put
/// an identity's signing key into whatever line formatted a value holding it, and no operation
/// reports the key either — exactly as Amazon SES publishes no Easy DKIM private key.
#[derive(Clone, PartialEq, Eq)]
pub struct SigningKey(Vec<u8>);

impl SigningKey {
    /// The key material, for the one thing that uses it: deriving the key pair that signs and the
    /// public key a signature is checked against.
    fn material(&self) -> &[u8] {
        &self.0
    }
}

impl ::std::fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(formatter, "SigningKey({} bytes)", self.0.len())
    }
}

/// How many tags one `DKIM-Signature` field is read from.
///
/// An EMULATOR ceiling: the field is a tag list a sender wrote, so a message carrying an
/// unbounded one is read up to here and no further. RFC 6376 defines a dozen tags, so no
/// signature a signer really writes reaches it.
const MAX_SIGNATURE_TAGS: usize = 32;

/// How many field NAMES one signature's `h=` list is read from.
///
/// An EMULATOR ceiling, for the same reason and one level down: the signed data is rebuilt by
/// looking each name the list carries up among the message's own fields, so a list a sender can
/// grow without bound would multiply that lookup by the size of the message. A signature covers
/// the handful of fields its signer wrote, so no signature a signer really writes reaches it.
const MAX_SIGNED_FIELD_NAMES: usize = 32;

/// The identity a message is signed for, and the key it is signed with.
#[derive(Debug, Clone)]
pub struct Signer {
    /// The domain the signature claims (`d=`): the identity's own domain.
    pub domain: String,
    /// The selector the signature names (`s=`): one of the identity's Easy DKIM tokens, which
    /// is the label its owner published the key under.
    pub selector: String,
    /// The identity's signing key, as the seed an Ed25519 key pair is derived from.
    pub key: SigningKey,
}

/// `message` with the `DKIM-Signature` field Amazon SES signs it with, added at the top of its
/// header block — which is where a signer puts its own field, so every field already in the
/// block keeps its order and its bytes and the body is untouched.
///
/// The message unchanged when it is not a message whose fields can be read at all, or when the
/// identity's key cannot be used: a send is not refused because its signature could not be
/// made, and an unsigned message is what a receiver then reports on.
#[must_use]
pub fn signed(message: &[u8], signer: &Signer, signed_at: SystemTime) -> Vec<u8> {
    match signature_field(message, signer, signed_at) {
        Some(field) => [
            format!("{DKIM_SIGNATURE_HEADER}: {field}{CRLF}").as_bytes(),
            message,
        ]
        .concat(),
        None => message.to_vec(),
    }
}

/// The value of the `DKIM-Signature` field covering `message`, and `None` when the message's
/// header block cannot be read or the signer's key cannot sign.
fn signature_field(message: &[u8], signer: &Signer, signed_at: SystemTime) -> Option<String> {
    let Ok((headers, body_at)) = ::mailparse::parse_headers(message) else {
        ::tracing::warn!(
            domain = ?signer.domain,
            "a message whose header block could not be read is sent unsigned"
        );
        return None;
    };
    let key = key_pair(signer.key.material())?;
    let covered = covered_fields(&headers);
    let body_hash = ::aws_smithy_types::base64::encode(digest(&canonical_body(
        message.get(body_at..).unwrap_or_default(),
    )));
    // "t=" is the instant the signature was made, in seconds since the epoch (RFC 6376 §3.5).
    let signed_at = signed_at
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let unsigned = format!(
        "v=1; a={SIGNING_ALGORITHM}; c={CANONICALIZATION}; d={}; s={}; t={signed_at}; \
         h={}; bh={body_hash}; b=",
        signer.domain,
        signer.selector,
        covered.join(":")
    );
    let signature = key.sign(signed_data(&headers, &covered, &unsigned).as_bytes());
    Some(format!(
        "{unsigned}{}",
        ::aws_smithy_types::base64::encode(signature.as_ref())
    ))
}

/// The signed-field list of a message: the fields of [`SIGNED_FIELDS`] the message carries, in
/// that order.
fn covered_fields(headers: &[::mailparse::MailHeader<'_>]) -> Vec<&'static str> {
    SIGNED_FIELDS
        .iter()
        .copied()
        .filter(|name| {
            headers
                .iter()
                .any(|header| header.get_key_ref().eq_ignore_ascii_case(name))
        })
        .collect()
}

/// The bytes a signature is made over (RFC 6376 §3.7): the canonicalized signed header fields
/// in the order `h=` lists them, then the canonicalized `DKIM-Signature` field itself with its
/// `b=` value empty and with no trailing line ending.
fn signed_data(
    headers: &[::mailparse::MailHeader<'_>],
    covered: &[&str],
    unsigned: &str,
) -> String {
    let mut data = String::new();
    for name in covered {
        let value = headers
            .iter()
            .find(|header| header.get_key_ref().eq_ignore_ascii_case(name))
            .map(|header| String::from_utf8_lossy(header.get_value_raw()).into_owned())
            .unwrap_or_default();
        data.push_str(&canonical_field(name, &value));
        data.push_str(CRLF);
    }
    data.push_str(&canonical_field(DKIM_SIGNATURE_HEADER, unsigned));
    data
}

/// The header field `name: value` as relaxed header canonicalization writes it (RFC 6376
/// §3.4.2): the name lowercased, the value unfolded with every run of whitespace reduced to one
/// space and the whitespace around the colon removed. The line ending the canonicalization adds
/// is appended by the caller, which is what lets the `DKIM-Signature` field itself be
/// canonicalized without one.
fn canonical_field(name: &str, value: &str) -> String {
    format!(
        "{}:{}",
        name.to_ascii_lowercase(),
        collapsed_whitespace(value)
    )
}

/// `text` with every run of whitespace reduced to a single space and the leading and trailing
/// whitespace removed — which is unfolding and the whitespace reduction relaxed
/// canonicalization performs, in one pass.
fn collapsed_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// `body` as relaxed body canonicalization writes it (RFC 6376 §3.4.4): the whitespace at the
/// end of each line ignored, every run of whitespace inside a line reduced to one space, the
/// empty lines at the end of the body ignored, and a non-empty body ending in exactly one
/// line ending.
fn canonical_body(body: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(body);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| collapsed_whitespace(line.strip_suffix('\r').unwrap_or(line)))
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.is_empty() {
        return Vec::new();
    }
    let mut canonical = lines.join(CRLF);
    canonical.push_str(CRLF);
    canonical.into_bytes()
}

/// The SHA-256 of `bytes`, which is the hash [`SIGNING_ALGORITHM`] names.
fn digest(bytes: &[u8]) -> Vec<u8> {
    use ::sha2::Digest;
    ::sha2::Sha256::digest(bytes).to_vec()
}

/// The Ed25519 key pair `seed` derives, and `None` for a seed that is not one — in which case
/// nothing is signed with it.
fn key_pair(seed: &[u8]) -> Option<::ring::signature::Ed25519KeyPair> {
    ::ring::signature::Ed25519KeyPair::from_seed_unchecked(seed)
        .map_err(|_| {
            ::tracing::warn!("an identity's DKIM key is not a signing key, so nothing signs with it");
        })
        .ok()
}

/// The verification key of the identity whose signing key is `key`.
fn verification_key(key: &SigningKey) -> Option<Vec<u8>> {
    use ::ring::signature::KeyPair;
    key_pair(key.material()).map(|pair| pair.public_key().as_ref().to_vec())
}

/// What checking a received message's `DKIM-Signature` established, in the value set RFC 7601
/// §2.7.1 defines for the `dkim` method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DkimResult {
    /// The message carries no signature at all.
    None,
    /// A signature was checked and is the signer's.
    Pass,
    /// A signature was checked and is not: the body or a signed field is not what was signed.
    Fail,
    /// A signature is there and could not be checked: its algorithm is not one this service
    /// signs with, its field is not a readable tag list, or no key is held for the domain and
    /// selector it names — at a receiver, a key that is not published.
    PermError,
}

impl DkimResult {
    /// The result as an `Authentication-Results` field spells it (RFC 7601 §2.7.1).
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::PermError => "permerror",
        }
    }
}

/// What a received message's `DKIM-Signature` established, and the domain it claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DkimOutcome {
    /// The result of the check.
    pub result: DkimResult,
    /// The `d=` domain the signature claimed, and `None` when there is no signature to claim
    /// one.
    pub domain: Option<String>,
}

/// What checking `message`'s first `DKIM-Signature` against the key its signing identity holds
/// establishes.
///
/// The key comes from this account's own identity list: a signature naming a domain and
/// selector the account holds a key for is checked against that key, and one naming any other
/// domain cannot be checked at all, because the key a third-party signer published lives in
/// that signer's DNS and this emulator resolves no zone.
#[must_use]
pub fn outcome(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    message: &[u8],
) -> DkimOutcome {
    let Ok((headers, body_at)) = ::mailparse::parse_headers(message) else {
        return DkimOutcome {
            result: DkimResult::None,
            domain: None,
        };
    };
    let Some(field) = headers
        .iter()
        .find(|header| {
            header
                .get_key_ref()
                .eq_ignore_ascii_case(DKIM_SIGNATURE_HEADER)
        })
        .map(|header| String::from_utf8_lossy(header.get_value_raw()).into_owned())
    else {
        return DkimOutcome {
            result: DkimResult::None,
            domain: None,
        };
    };
    let tags = signature_tags(&field);
    let tag = |name: &str| {
        tags.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let domain = tag("d");
    let outcome = |result: DkimResult| DkimOutcome {
        result,
        domain: domain.clone(),
    };
    let (Some(signing_domain), Some(selector), Some(algorithm), Some(body_hash), Some(signature)) =
        (domain.clone(), tag("s"), tag("a"), tag("bh"), tag("b"))
    else {
        return outcome(DkimResult::PermError);
    };
    if algorithm != SIGNING_ALGORITHM {
        return outcome(DkimResult::PermError);
    }
    let Some(key) =
        verification_key_for(state, &signing_domain, &selector).and_then(|key| verification_key(&key))
    else {
        return outcome(DkimResult::PermError);
    };
    let (Ok(body_hash), Ok(signature)) = (
        ::aws_smithy_types::base64::decode(&body_hash),
        ::aws_smithy_types::base64::decode(collapsed_whitespace(&signature).replace(' ', "")),
    ) else {
        return outcome(DkimResult::PermError);
    };
    if digest(&canonical_body(message.get(body_at..).unwrap_or_default())) != body_hash {
        return outcome(DkimResult::Fail);
    }
    // The signed data is rebuilt from the fields `h=` named, with this very field's own `b=`
    // emptied, which is the data the signer signed (RFC 6376 §3.7).
    let named: Vec<String> = tag("h")
        .as_deref()
        .unwrap_or_default()
        .split(':')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .take(MAX_SIGNED_FIELD_NAMES)
        .map(str::to_string)
        .collect();
    let covered: Vec<&str> = named.iter().map(String::as_str).collect();
    let Some(unsigned) = unsigned_signature(&field) else {
        return outcome(DkimResult::PermError);
    };
    let verified = ::ring::signature::UnparsedPublicKey::new(&::ring::signature::ED25519, &key)
        .verify(
            signed_data(&headers, &covered, &unsigned).as_bytes(),
            &signature,
        )
        .is_ok();
    outcome(if verified {
        DkimResult::Pass
    } else {
        DkimResult::Fail
    })
}

/// The identity whose Easy DKIM setup signs a message sent from `address`, and `None` when this
/// account signs nothing for that sender.
///
/// The identity that COVERS the sender is the one that signs — an address identity, or the
/// domain identity above it, exactly as the sender gate reads a sender
/// ([`super::ComAmazonawsSesSimpleemailserviceState::is_verified_sender`]) — and it signs only
/// where Easy DKIM is in place: `DkimEnabled` on, a token set published (the selector a
/// signature names is one of those tokens), and the search for those records settled. A setup
/// still waiting on them belongs to an identity whose own verification has succeeded — an
/// identity whose published records Amazon SES HAS read, which is the reading this surface
/// already applies to a setup the account holds for an identity it adopts
/// ([`super::ComAmazonawsSesSimpleemailserviceState`]'s registry reconciliation) — so the
/// identity's verification is what settles it, and a message from an identity that is not
/// verified at all is never signed.
///
/// The key pair belongs to the DOMAIN, which is what a DKIM key is published for, and is minted
/// the first time that domain signs.
#[must_use]
pub fn signer(
    state: &mut super::ComAmazonawsSesSimpleemailserviceState,
    address: &str,
) -> Option<Signer> {
    let sender = super::identity_key(address);
    let mut signing = None;
    for name in super::sender_identity_keys(&sender) {
        let Some(identity) = state.identities.get(name) else {
            continue;
        };
        if identity.verification_status != super::VerificationStatus::Success
            || !identity.dkim_enabled
        {
            continue;
        }
        let Some(setup) = identity.dkim.as_ref().filter(|setup| {
            setup.status == super::VerificationStatus::Success
                || setup.status == super::VerificationStatus::Pending
        }) else {
            continue;
        };
        if let Some(selector) = setup.tokens.first() {
            signing = Some((
                super::identity_domain(&identity.name).to_string(),
                selector.clone(),
            ));
            break;
        }
    }
    let (domain, selector) = signing?;
    let key = signing_key(state, &domain);
    Some(Signer {
        domain,
        selector,
        key,
    })
}

/// The signing key of `domain`, minted the first time that domain signs and kept so every
/// message it signs is signed with the one key its published selector stands for.
///
/// The store holds one key per DOMAIN of the identity list, so the keys of domains the account
/// no longer holds an identity in are dropped before another is minted: the identity list is
/// bounded ([`super::MAX_IDENTITIES`]), so this store is too.
fn signing_key(
    state: &mut super::ComAmazonawsSesSimpleemailserviceState,
    domain: &str,
) -> SigningKey {
    let key = super::identity_key(domain);
    if let Some(held) = state.dkim_signing_keys.get(&key) {
        return held.clone();
    }
    let identities = &state.identities;
    state.dkim_signing_keys.retain(|held, _| {
        identities.values().any(|identity| {
            super::identity_key(super::identity_domain(&identity.name)) == *held
        })
    });
    let minted = minted_signing_key();
    state.dkim_signing_keys.insert(key, minted.clone());
    minted
}

/// The key the signature of `domain` under `selector` is checked against: the key that domain
/// holds, and only where `selector` is a token an identity of that domain really published —
/// a selector the account never handed out names no key of it.
fn verification_key_for(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    domain: &str,
    selector: &str,
) -> Option<SigningKey> {
    let published = state.identities.values().any(|identity| {
        super::identity_domain(&identity.name).eq_ignore_ascii_case(domain)
            && identity
                .dkim
                .as_ref()
                .is_some_and(|setup| setup.tokens.iter().any(|token| token == selector))
    });
    published
        .then(|| state.dkim_signing_keys.get(&super::identity_key(domain)))
        .flatten()
        .cloned()
}

/// A fresh signing key: [`SIGNING_KEY_BYTES`] from the system's randomness, which is what an
/// Ed25519 key pair is seeded from.
///
/// Drawn from the system rather than from anything a request carried or anything stored, so no
/// caller can derive an identity's key. A draw the system refuses yields no key material at all,
/// and a message that would be signed with it is sent unsigned rather than signed with
/// something a caller could guess.
fn minted_signing_key() -> SigningKey {
    let mut seed = vec![0_u8; SIGNING_KEY_BYTES];
    if ::ring::rand::SecureRandom::fill(&::ring::rand::SystemRandom::new(), &mut seed).is_err() {
        ::tracing::warn!("no DKIM signing key could be drawn, so this identity signs nothing");
        return SigningKey(Vec::new());
    }
    SigningKey(seed)
}

/// The tags a `DKIM-Signature` field carries, as `(name, value)` pairs: the `tag=value` list
/// RFC 6376 §3.2 defines, read up to [`MAX_SIGNATURE_TAGS`] of them and with the whitespace a
/// folded field left in each value removed.
fn signature_tags(field: &str) -> Vec<(String, String)> {
    field
        .split(';')
        .take(MAX_SIGNATURE_TAGS)
        .filter_map(|tag| tag.split_once('='))
        .map(|(name, value)| {
            (
                collapsed_whitespace(name).replace(' ', ""),
                collapsed_whitespace(value),
            )
        })
        .collect()
}

/// `field` with the value of its own `b=` tag emptied, which is how the field a signature
/// covers itself with is canonicalized (RFC 6376 §3.7), and `None` for a field carrying no
/// `b=` tag at all — which is a field carrying no signature.
///
/// Found by TAG rather than by searching the text for `b=`, because the base64 of another tag's
/// value can end in those two characters.
fn unsigned_signature(field: &str) -> Option<String> {
    let mut rebuilt = String::new();
    let mut signed = false;
    // Bounded exactly as the tag READER above is ([`MAX_SIGNATURE_TAGS`]): both readings of a
    // received signature take the same number of tags from it.
    for (index, segment) in field.split(';').take(MAX_SIGNATURE_TAGS).enumerate() {
        if index > 0 {
            rebuilt.push(';');
        }
        match segment.split_once('=') {
            Some((name, _)) if name.trim() == "b" => {
                rebuilt.push_str(name);
                rebuilt.push('=');
                signed = true;
            }
            _ => rebuilt.push_str(segment),
        }
    }
    signed.then_some(rebuilt)
}
