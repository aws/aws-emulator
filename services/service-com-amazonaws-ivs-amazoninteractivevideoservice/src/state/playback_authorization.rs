// SPDX-License-Identifier: CC0-1.0

//! Who may PLAY BACK a channel, and the key material that decides it.
//!
//! RULE: a channel that is "private (enabled for playback authorization)"
//! (`smithy_query(query="doc",shape="Channel",member="authorized")`) serves its playback
//! only to a viewer presenting a PLAYBACK TOKEN signed with the private half of one of the
//! account's imported playback key pairs, naming that channel and still inside its own
//! validity window. A channel that is not enabled for it is public: its playback URL is
//! fetchable by a player holding no credentials at all, which is why this plane is served
//! at the unauthenticated seam in the first place.
//!
//! So the import keeps what the check needs. `ImportPlaybackKeyPair` takes "the public
//! portion of a customer-generated key pair" and no modelled shape reports it back — a
//! `PlaybackKeyPair` carries only its ARN, name, fingerprint and tags — so the DER
//! `SubjectPublicKeyInfo` is held beside the modelled resource in
//! [`PlaybackKeyPairRecord`] and reported by nothing. It is the PUBLIC half: anyone holding
//! the same key set could verify the same token, and no private key or secret is in this
//! store.
//!
//! The token itself is a compact JWS, read and verified through the tree's ONE reading of
//! that format (`service_common::jwt`): the signature is checked over the token's own bytes
//! under the algorithm its header names, against the keys this account imported, and the
//! claim set is then held to naming THIS channel and to its `exp`. Nothing here treats the
//! presence of a token, of a channel id or of a header as proof.

use super::*;

/// One imported playback key pair as this store holds it.
#[derive(Debug, Clone)]
pub struct PlaybackKeyPairRecord {
    /// The modelled resource every read of the pair answers.
    pub pair: PlaybackKeyPair,
    /// The DER `SubjectPublicKeyInfo` of the public half the import carried, which is what
    /// a presented playback token's signature is checked against. Never reported.
    pub public_key_der: Vec<u8>,
}

/// The query parameter a viewer presents a playback token in.
///
/// AWS documents the private-channel playback flow as appending the token to the playback
/// URL (`…/channel.m3u8?token=<token>`), which is how a player that holds no credentials
/// presents one, and the only place this plane reads one from.
pub const PLAYBACK_TOKEN_PARAMETER: &str = "token";

/// The claim a playback token names the channel it authorizes in.
pub const PLAYBACK_CHANNEL_ARN_CLAIM: &str = "aws:channel-arn";

/// Whether a playback request may be served the channel it addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackAuthorization {
    /// Serve it: the channel is public, or the viewer presented a token this account's
    /// imported keys verify for this channel.
    Granted,
    /// Refuse it: the channel is private and the request carries no token this account's
    /// imported keys verify for it.
    Refused,
    /// No channel of this store has that identity, so there is nothing to authorize — the
    /// plane answers such a path exactly as it answers an address nobody published.
    UnknownChannel,
}

/// What this store decides about a playback request on `target` carrying `query`.
///
/// The decision is about the CHANNEL the path names, not about which of its three playback
/// paths was fetched: a playlist and a segment of one private channel are the same
/// resource to a viewer, so one rule covers them.
#[must_use]
pub fn playback_authorization(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    target: &PlaybackTarget,
    query: Option<&str>,
) -> PlaybackAuthorization {
    let Some(channel_arn) = playback_channel_arn(state, target) else {
        return PlaybackAuthorization::UnknownChannel;
    };
    // The channel the path resolved to, read back under the same guard: a path whose
    // channel this store does not hold authorizes nothing, and is answered as an address
    // nobody published rather than as a refusal or a grant.
    let Some(channel) = state.channels.get(&channel_arn) else {
        return PlaybackAuthorization::UnknownChannel;
    };
    if !channel.authorized {
        return PlaybackAuthorization::Granted;
    }
    let presented = query.and_then(|query| {
        ::service_common::query_parameter(query, PLAYBACK_TOKEN_PARAMETER)
            .filter(|token| !token.is_empty())
    });
    match presented {
        Some(token) if token_authorizes(state, &channel_arn, token) => {
            PlaybackAuthorization::Granted
        }
        _ => PlaybackAuthorization::Refused,
    }
}

/// Whether `token` authorizes playback of `channel_arn` right now.
///
/// Three things have to hold, in this order: the token is a compact JWS whose signature
/// checks out against one of the keys this account imported; it is inside its own validity
/// window; and it names THIS channel in [`PLAYBACK_CHANNEL_ARN_CLAIM`], so a token minted
/// for one channel does not open another.
///
/// The window is the consumer's rule and this consumer requires one: `outside_window`
/// reports a token carrying NO `exp` as outside its window (`WindowDefect::NoExpiry`)
/// rather than deciding it, and every reason it reports — no expiry, expired, not yet valid
/// — refuses the token here, so a token without an expiry is not accepted.
fn token_authorizes(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    token: &str,
) -> bool {
    let Ok(verified) = ::service_common::jwt::verify(token, &playback_key_set(state)) else {
        return false;
    };
    if verified.outside_window(SystemTime::now()).is_some() {
        return false;
    }
    verified.claim_str(PLAYBACK_CHANNEL_ARN_CLAIM) == Some(channel_arn)
}

/// The JSON Web Key Set of the playback key pairs this account has imported: the public
/// half of each one this store can check a signature under, in the parameters RFC 7518 §6
/// publishes.
///
/// A key whose material is not one a JOSE signature can be checked under — a curve the JWS
/// algorithms do not cover — is simply not in the set, which makes a token signed with it
/// unverifiable rather than accepted.
fn playback_key_set(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
) -> ::serde_json::Value {
    let keys: Vec<::serde_json::Value> = state
        .playback_key_pairs
        .values()
        .filter_map(|record| playback_jwk(&record.public_key_der))
        .collect();
    ::serde_json::json!({ "keys": keys })
}

/// The JWK of one imported key's DER `SubjectPublicKeyInfo`, or `None` for material no JOSE
/// signature can be checked under.
///
/// An imported playback key is an elliptic-curve public key (the import refuses anything
/// else), so the structure carries the curve as its algorithm parameter and the key as the
/// uncompressed point of SEC 1 §2.3.3 — `0x04` then the two coordinates, each the curve's
/// field width. Those coordinates ARE the JWK's `x` and `y` (RFC 7518 §6.2.1), and the
/// `kid` is the key's own RFC 7638 thumbprint, so a token naming a key id finds the key it
/// was signed with and one naming none is tried against every imported key.
fn playback_jwk(public_key_der: &[u8]) -> Option<::serde_json::Value> {
    use ::spki::der::Decode as _;

    let key = ::spki::SubjectPublicKeyInfoRef::from_der(public_key_der).ok()?;
    let (curve, coordinate_bytes) = match key.algorithm.parameters_oid().ok()?.to_string().as_str()
    {
        SECP256R1_OID => (P256_CURVE, P256_COORDINATE_BYTES),
        SECP384R1_OID => (P384_CURVE, P384_COORDINATE_BYTES),
        _ => return None,
    };
    let point = key.subject_public_key.as_bytes()?;
    let (form, coordinates) = point.split_first()?;
    if *form != UNCOMPRESSED_POINT || coordinates.len() != 2 * coordinate_bytes {
        return None;
    }
    let (x, y) = coordinates.split_at(coordinate_bytes);
    let mut jwk = ::serde_json::json!({
        "kty": EC_KEY_TYPE,
        "crv": curve,
        "x": ::service_common::jwt::encode_base64url(x),
        "y": ::service_common::jwt::encode_base64url(y),
    });
    if let Some(kid) = ::service_common::jwt::jwk_thumbprint(&jwk) {
        jwk["kid"] = ::serde_json::Value::String(kid);
    }
    Some(jwk)
}

/// The JWK key type of an elliptic-curve key (RFC 7518 §6.2).
const EC_KEY_TYPE: &str = "EC";

/// `prime256v1`/`secp256r1` (RFC 5480 §2.1.1.1), the curve JOSE publishes as `P-256`.
const SECP256R1_OID: &str = "1.2.840.10045.3.1.7";

/// `secp384r1` (the same section), which JOSE publishes as `P-384` — the curve AWS's
/// playback-token signing uses (`ES384`).
const SECP384R1_OID: &str = "1.3.132.0.34";

/// The JOSE names of those two curves (RFC 7518 §6.2.1.1).
const P256_CURVE: &str = "P-256";
const P384_CURVE: &str = "P-384";

/// The width of one coordinate on each: the curve's field size in bytes, which is also the
/// width RFC 7518 §6.2.1.2 requires of the `x` and `y` members.
const P256_COORDINATE_BYTES: usize = 32;
const P384_COORDINATE_BYTES: usize = 48;

/// The first byte of an UNCOMPRESSED elliptic-curve point (SEC 1 §2.3.3), which is the form
/// a `SubjectPublicKeyInfo` carries and the only one these coordinates can be read from.
const UNCOMPRESSED_POINT: u8 = 0x04;
