// SPDX-License-Identifier: CC0-1.0

//! SIGNED ACCESS: whether a cache behaviour that restricts its content to trusted signers
//! serves one viewer request.
//!
//! `CacheBehavior$TrustedKeyGroups` is "a list of key groups that CloudFront can use to
//! validate signed URLs or signed cookies", and what enabling it means is stated on the
//! member itself: "When a cache behavior contains trusted key groups, CloudFront requires
//! signed URLs or signed cookies for all requests that match the cache behavior. The URLs or
//! cookies must be signed with a private key whose corresponding public key is in the key
//! group." So a behaviour that enables one is an AUTHORIZATION boundary, and this module is
//! the whole of it:
//!
//! 1. the request must CARRY a signature. It travels either in the query string of a signed
//!    URL (`Key-Pair-Id`, `Signature`, and `Policy` for a custom policy or `Expires` for a
//!    canned one) or in the signed cookies of the same names under the `CloudFront-` prefix.
//!    A request that carries both is read as the signed URL alone, the query string being
//!    what the viewer addressed;
//! 2. the KEY the signature names must be one of the public keys that are in the behaviour's
//!    trusted key groups AT THIS INSTANT. The groups are re-read from the store per request,
//!    so a public key an `UpdateKeyGroup` has taken out of the group stops admitting the URLs
//!    it signed, while the other keys of that group keep admitting theirs — which is what
//!    makes removing a key a REVOCATION rather than a relabelling;
//! 3. the SIGNATURE must be the one that key verifies over the policy statement: RSA
//!    PKCS#1 v1.5 over the statement's SHA-1 digest, the signature CloudFront's own signing
//!    procedure produces, with the base64 spelled in the alphabet that procedure substitutes
//!    for a URL ([`decode_base64`]);
//! 4. the POLICY's own conditions must hold for THIS request: the `Resource` it was signed
//!    for must name what the viewer asked for, `DateLessThan` must not have passed,
//!    `DateGreaterThan` must have, and `IpAddress` must contain the address the request came
//!    from — the one address this edge reads for a viewer
//!    ([`super::generated_headers::viewer_address`]).
//!
//! Each failure is answered by CLOUDFRONT, with `403` and no origin request made, and the
//! error code says WHICH of the four it was: a request that names no key at all cannot be
//! told whose it is ([`Refusal::MissingKey`]), a key that is not in the trusted groups is an
//! unknown key ([`Refusal::InvalidKey`]) — the state a revoked key is in — and everything
//! else is a signature or a condition that did not hold, which is a denial of access and says
//! nothing more about the credential ([`Refusal::AccessDenied`]).
//!
//! The LEGACY member beside it, `TrustedSigners`, is not read here: its signers are "AWS
//! account IDs whose public keys CloudFront can use to validate signed URLs", meaning the
//! CloudFront key pairs of a root account, which have no API in CloudFront at all — nothing
//! in this emulator can hold one, so there is no key material to verify a signature against
//! and no admission this module could decide. `TrustedKeyGroups` is the mechanism AWS
//! publishes for new work and the one every key this service stores belongs to.

use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::TrustedKeyGroups;
use crate::state::{matches_glob, ComAmazonawsCloudfrontCloudfront20200531State};

/// The query parameter naming the public key a signature was made with: "The ID of the
/// CloudFront public key whose corresponding private key you're using to generate the
/// signature."
const KEY_PAIR_ID: &str = "Key-Pair-Id";

/// The query parameter carrying the signature itself: "The hashed, signed, and base64-encoded
/// version of the JSON policy statement."
const SIGNATURE: &str = "Signature";

/// The query parameter carrying a CUSTOM policy: "The base64-encoded version of the JSON
/// policy statement."
const POLICY: &str = "Policy";

/// The query parameter a CANNED policy states its expiry in: "The expiration date and time
/// for the URL, in Unix time format and Coordinated Universal Time (UTC)."
const EXPIRES: &str = "Expires";

/// The prefix the same four names carry as SIGNED COOKIES: `CloudFront-Key-Pair-Id`,
/// `CloudFront-Signature`, `CloudFront-Policy` and `CloudFront-Expires`.
const COOKIE_PREFIX: &str = "CloudFront-";

/// The condition key a policy's date conditions are stated under: `AWS:EpochTime`.
const EPOCH_TIME: &str = "AWS:EpochTime";

/// The condition key a policy's address condition is stated under: `AWS:SourceIp`.
const SOURCE_IP: &str = "AWS:SourceIp";

/// The three characters CloudFront's signing procedure substitutes in a base64 value so it
/// can travel in a URL, as `(in the value, in base64)`: "Replace all occurrences of `+`,
/// `=`, and `/` in the base64-encoded version with the characters in the following list."
const BASE64_SUBSTITUTIONS: [(char, char); 3] = [('-', '+'), ('_', '='), ('~', '/')];

/// The longest policy statement this module will read, in bytes.
///
/// A viewer chooses the value, and every byte of it is base64-decoded and then parsed as
/// JSON, so what one request can ask this process for is bounded here. A CloudFront policy
/// statement is one statement naming one resource and at most three conditions — far inside
/// this bound — and a value past it is read as a policy that did not verify rather than
/// parsed.
const MAX_POLICY_BYTES: usize = 8 * 1024;

/// The longest signature this module will read, in bytes: a PKCS#1 signature is exactly as
/// long as the signing key's modulus, and this is the widest RSA modulus the verification
/// algorithm accepts (8192 bits).
const MAX_SIGNATURE_BYTES: usize = 1024;

/// What CloudFront answers a viewer whose signed access it did not admit.
///
/// Every one of them is `403`, which is the status the member documents for the restriction
/// ("CloudFront requires signed URLs or signed cookies for all requests that match the cache
/// behavior"); they differ in the error code, which says which of the four checks above the
/// request failed.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Refusal {
    /// The request named no public key at all, so there is no credential to judge.
    MissingKey,
    /// The key the request named is not one of the public keys in the behaviour's trusted key
    /// groups: an unknown key, which is also the state a key an `UpdateKeyGroup` removed
    /// from the group is in.
    InvalidKey,
    /// The signature did not verify, or a condition of the policy it was made over does not
    /// hold for this request.
    AccessDenied,
}

impl Refusal {
    /// The `Code` CloudFront's error body carries.
    pub fn code(self) -> &'static str {
        match self {
            Self::MissingKey => "MissingKey",
            Self::InvalidKey => "InvalidKey",
            Self::AccessDenied => "AccessDenied",
        }
    }

    /// The `Message` CloudFront's error body carries beside that code.
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingKey => "Missing Key-Pair-Id query parameter or cookie value",
            Self::InvalidKey => "Unknown Key",
            Self::AccessDenied => "Access denied",
        }
    }
}

/// Whether a behaviour restricts its content to trusted signers at all.
///
/// "When a cache behavior contains trusted key groups, CloudFront requires signed URLs or
/// signed cookies for all requests that match the cache behavior": a list that is not
/// `Enabled`, or one that names no group, contains none, so such a behaviour serves every
/// viewer and this module decides nothing about it.
pub fn restricted(trusted_key_groups: Option<&TrustedKeyGroups>) -> bool {
    trusted_key_groups.is_some_and(|groups| {
        groups.enabled
            && groups
                .items
                .as_ref()
                .is_some_and(|items| !items.is_empty())
    })
}

/// The query string of the URL a policy was signed for: the viewer's own parameters, in the
/// order it sent them, without the ones that carry the signature itself.
///
/// A signed URL's `Resource` is "the base URL, including the query string, if any" — the URL
/// the viewer asked for before CloudFront's own parameters were appended to it, so those four
/// are not part of what was signed.
pub fn resource_query(query: &str) -> String {
    super::forwarding::selected_query(query, |name| {
        ![KEY_PAIR_ID, SIGNATURE, POLICY, EXPIRES].contains(&name)
    })
}

/// One viewer request, as the signed-access decision reads it.
#[derive(Debug)]
pub struct SignedRequest<'a> {
    /// The query string the viewer addressed, without its leading `?`.
    pub query: &'a str,
    /// The viewer's `Cookie` header, or the empty string when it sent none.
    pub cookies: &'a str,
    /// The URLs this request addresses, which a policy's `Resource` is matched against: the
    /// distribution's own domain name and each alternate domain name it answers at, since a
    /// policy may be written for any name the viewer reaches the distribution by.
    pub resource_urls: Vec<String>,
    /// The address the request came from, by the one reading of it this edge has.
    pub address: &'a str,
}

/// What the behaviour's trusted key groups make of `request`, or `None` when it is admitted.
///
/// `store` is read for the groups and the keys themselves, so the decision is made against the
/// membership THIS instant rather than against anything captured when the distribution was
/// created.
pub fn refusal(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    trusted_key_groups: Option<&TrustedKeyGroups>,
    request: &SignedRequest<'_>,
    now: SystemTime,
) -> Option<Refusal> {
    let groups = trusted_key_groups.filter(|_| restricted(trusted_key_groups))?;
    let presented = match Presented::of_request(request) {
        Ok(presented) => presented,
        Err(refusal) => return Some(refusal),
    };
    // The key must be in the trusted groups AS THEY ARE NOW: a key an update took out of
    // every trusted group is an unknown key, so the URLs it signed stop being admitted.
    let Some(encoded_key) = trusted_public_key(store, groups, &presented.key_pair_id) else {
        ::tracing::debug!(
            "the key this signed request names is in none of the behaviour's trusted key \
             groups, so the request is refused"
        );
        return Some(Refusal::InvalidKey);
    };
    let Some(signature) = decode_base64(&presented.signature, MAX_SIGNATURE_BYTES) else {
        return Some(Refusal::AccessDenied);
    };
    let Some(key) = rsa_public_key(&encoded_key) else {
        // The stored `EncodedKey` is not an RSA public key this process can read, so no
        // signature can be verified against it.
        return Some(Refusal::AccessDenied);
    };
    // A canned policy names ONE resource, which is the URL the viewer asked for, so the
    // statement it was signed over is rebuilt per candidate URL; a custom policy carries its
    // own statement and is verified once.
    let statements: Vec<Vec<u8>> = match &presented.policy {
        Policy::Custom(statement) => vec![statement.clone()],
        Policy::Canned { expires } => request
            .resource_urls
            .iter()
            .map(|url| canned_statement(url, *expires).into_bytes())
            .collect(),
    };
    let Some(verified) = statements
        .into_iter()
        .find(|statement| verifies(&key, statement, &signature))
    else {
        return Some(Refusal::AccessDenied);
    };
    let admitted = match &presented.policy {
        // "DateLessThan … This is the only required parameter in the Condition section" of a
        // canned policy, and the canned form states it alone: the statement that verified is
        // the one built for a resource URL this request addresses, so the resource holds by
        // construction.
        Policy::Canned { expires } => before(now, *expires),
        Policy::Custom(_) => admits(&verified, request, now),
    };
    (!admitted).then_some(Refusal::AccessDenied)
}

/// The signature material one viewer request presented.
#[derive(Debug)]
struct Presented {
    /// `Key-Pair-Id`: the public key the signature was made with.
    key_pair_id: String,
    /// `Signature`, still in the alphabet CloudFront's procedure spelled it in.
    signature: String,
    /// Which of the two policy forms the request carried.
    policy: Policy,
}

/// The two forms a CloudFront policy takes.
#[derive(Debug)]
enum Policy {
    /// A CUSTOM policy: the statement the viewer carried, base64-decoded, which is exactly
    /// the bytes the signature was made over.
    Custom(Vec<u8>),
    /// A CANNED policy: `Expires` alone, the statement being rebuilt from it and the URL the
    /// viewer asked for.
    Canned { expires: i64 },
}

impl Presented {
    /// The signature material `request` carried, or the refusal that material earns.
    ///
    /// "If the viewer request contains both signed cookies and a signed URL, CloudFront uses
    /// only the signed URL": a request whose QUERY STRING carries any of the four names is
    /// read as a signed URL, cookies and all, and the cookies are read only for a request
    /// whose query string carries none of them.
    ///
    /// A request that names no key at all is [`Refusal::MissingKey`] — there is no credential
    /// to judge. A request that names one but carries no policy to judge it against, or one
    /// that is not the encoding the signing procedure produces, is a denial of access: the key
    /// it named is not reported on.
    fn of_request(request: &SignedRequest<'_>) -> Result<Self, Refusal> {
        let signed_url = [KEY_PAIR_ID, SIGNATURE, POLICY, EXPIRES]
            .iter()
            .any(|name| query_value(request.query, name).is_some());
        let value = |name: &str| {
            if signed_url {
                query_value(request.query, name)
            } else {
                cookie_value(request.cookies, &format!("{COOKIE_PREFIX}{name}"))
            }
        };
        let key_pair_id = value(KEY_PAIR_ID).ok_or(Refusal::MissingKey)?;
        let signature = value(SIGNATURE).ok_or(Refusal::AccessDenied)?;
        let policy = match value(POLICY) {
            Some(encoded) => Policy::Custom(
                decode_base64(&encoded, MAX_POLICY_BYTES).ok_or(Refusal::AccessDenied)?,
            ),
            None => Policy::Canned {
                expires: value(EXPIRES)
                    .ok_or(Refusal::AccessDenied)?
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| Refusal::AccessDenied)?,
            },
        };
        Ok(Self {
            key_pair_id,
            signature,
            policy,
        })
    }
}

/// The `EncodedKey` of the public key `key_pair_id` names, when that key is in one of the key
/// groups `groups` trusts.
///
/// Both halves are read from the store as it is now: a group the list names but the account no
/// longer holds trusts nothing, and a key the group lists but the account no longer holds is no
/// key to verify with.
fn trusted_public_key(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    groups: &TrustedKeyGroups,
    key_pair_id: &str,
) -> Option<String> {
    let trusted = groups
        .items
        .as_ref()?
        .iter()
        .filter_map(|id| store.key_group(id))
        .any(|group| group.config.items.iter().any(|id| id == key_pair_id));
    if !trusted {
        return None;
    }
    Some(store.public_key(key_pair_id)?.config.encoded_key.clone())
}

/// The canned policy statement for `resource` expiring at `expires`, as CloudFront's signing
/// procedure builds it.
///
/// "When you use a canned policy, the `Statement` section of the policy statement contains
/// only the `Resource` and the `DateLessThan` condition", and it is rendered with no
/// whitespace at all — the signature is over these exact bytes, so the rendering is the
/// contract.
fn canned_statement(resource: &str, expires: i64) -> String {
    format!(
        "{{\"Statement\":[{{\"Resource\":\"{resource}\",\"Condition\":\
         {{\"DateLessThan\":{{\"{EPOCH_TIME}\":{expires}}}}}}}]}}"
    )
}

/// Whether the custom policy `statement` admits `request` at `now`.
///
/// A policy is "a JSON document" whose `Statement` list names, per entry, the `Resource` the
/// signature is for and the `Condition` section that bounds it. An entry admits the request
/// when its resource names what the viewer asked for and every condition it states holds; the
/// policy admits the request when one of its entries does.
fn admits(statement: &[u8], request: &SignedRequest<'_>, now: SystemTime) -> bool {
    let Ok(document) = ::serde_json::from_slice::<::serde_json::Value>(statement) else {
        return false;
    };
    let Some(entries) = document.get("Statement").and_then(|list| list.as_array()) else {
        return false;
    };
    entries
        .iter()
        .any(|entry| entry_admits(entry, request, now))
}

/// Whether one `Statement` entry admits the request.
fn entry_admits(
    entry: &::serde_json::Value,
    request: &SignedRequest<'_>,
    now: SystemTime,
) -> bool {
    // "The base URL … including the query string, if any": the resource a policy names may
    // carry the `*` and `?` wildcards, matched the way every other CloudFront pattern is.
    let Some(resource) = entry.get("Resource").and_then(|value| value.as_str()) else {
        return false;
    };
    if !request
        .resource_urls
        .iter()
        .any(|url| matches_glob(resource, url))
    {
        return false;
    }
    let condition = entry.get("Condition");
    // "DateLessThan … This is the only required parameter in the Condition section."
    let Some(expires) = condition
        .and_then(|condition| epoch_time(condition, "DateLessThan"))
    else {
        return false;
    };
    if !before(now, expires) {
        return false;
    }
    // "DateGreaterThan … An optional start date and time for the URL": the URL is not valid
    // before it.
    if let Some(starts) = condition.and_then(|condition| epoch_time(condition, "DateGreaterThan"))
    {
        if before(now, starts) {
            return false;
        }
    }
    // "IpAddress … The IP address of the client making the GET request": the address the
    // request came from has to be in the range the policy names.
    if let Some(range) = condition
        .and_then(|condition| condition.get("IpAddress"))
        .and_then(|address| address.get(SOURCE_IP))
    {
        let Some(range) = range.as_str() else {
            return false;
        };
        if !contains_address(range, request.address) {
            return false;
        }
    }
    true
}

/// The `AWS:EpochTime` of the condition `name` states, when it states one.
fn epoch_time(condition: &::serde_json::Value, name: &str) -> Option<i64> {
    condition.get(name)?.get(EPOCH_TIME)?.as_i64()
}

/// Whether `now` is before the Unix instant `epoch_seconds`.
///
/// An instant before the Unix epoch is before every expiry a policy can state, and one that
/// does not fit in the clock's own arithmetic is read as the far future, which is what an
/// expiry that large means.
fn before(now: SystemTime, epoch_seconds: i64) -> bool {
    let Ok(seconds) = u64::try_from(epoch_seconds) else {
        return false;
    };
    let Some(instant) = UNIX_EPOCH.checked_add(Duration::from_secs(seconds)) else {
        return true;
    };
    now < instant
}

/// Whether the network `range` contains `address`.
///
/// `AWS:SourceIp` carries "the IP address or range of addresses of the client making the GET
/// request", in "standard IPv4 CIDR format" — a single address being the `/32` of itself —
/// which is read by the crate that owns the format rather than by comparing text.
fn contains_address(range: &str, address: &str) -> bool {
    let Ok(address) = address.parse::<::std::net::IpAddr>() else {
        return false;
    };
    if let Ok(network) = range.parse::<::ipnet::IpNet>() {
        return network.contains(&address);
    }
    // A bare address, which names the one address it is.
    range
        .parse::<::std::net::IpAddr>()
        .is_ok_and(|named| named == address)
}

/// The bytes `value` encodes, in the base64 alphabet CloudFront's signing procedure spells a
/// URL-safe value in, or `None` for a value that is not that encoding or is longer than
/// `ceiling` bytes.
///
/// "Replace all occurrences of `+`, `=`, and `/` in the base64-encoded version with the
/// characters in the following list" — `-`, `_` and `~` — so decoding puts them back and then
/// reads standard base64. The value also travels in a URL, so percent-encoding is undone
/// first: a client that encoded the characters its own way has sent the same value.
fn decode_base64(value: &str, ceiling: usize) -> Option<Vec<u8>> {
    use ::base64::Engine as _;

    // The ENCODED text is read first, before anything is allocated from it: base64 carries
    // three bytes per four characters, so a value longer than this encodes more than the
    // ceiling allows whatever it decodes to.
    let encoded_ceiling = ceiling.saturating_mul(4).saturating_div(3).saturating_add(4);
    if value.len() > encoded_ceiling {
        return None;
    }
    let decoded = ::percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()?;
    let mut standard = String::with_capacity(decoded.len());
    for character in decoded.chars() {
        match BASE64_SUBSTITUTIONS
            .iter()
            .find(|(substituted, _)| *substituted == character)
        {
            Some((_, original)) => standard.push(*original),
            None => standard.push(character),
        }
    }
    if standard.len() > encoded_ceiling {
        return None;
    }
    let bytes = ::base64::engine::general_purpose::STANDARD
        .decode(standard.as_bytes())
        .ok()?;
    (bytes.len() <= ceiling).then_some(bytes)
}

/// The RSA public key the stored `EncodedKey` carries, as the verifier reads it.
///
/// `PublicKeyConfig$EncodedKey` is "the public key that you can use with signed URLs and
/// signed cookies, or with field-level encryption" — a PEM-enveloped X.509
/// `SubjectPublicKeyInfo`, which is what `openssl rsa -pubout` writes. The envelope is split
/// by the crate that owns the PEM format and the structure inside it decoded by the one that
/// owns `SubjectPublicKeyInfo`, so the key a signature is verified against is the key a
/// client really uploaded.
fn rsa_public_key(encoded_key: &str) -> Option<Vec<u8>> {
    use ::x509_cert::der::Decode as _;

    let block = ::pem::parse(encoded_key).ok()?;
    let info = ::x509_cert::spki::SubjectPublicKeyInfoRef::from_der(block.contents()).ok()?;
    Some(info.subject_public_key.raw_bytes().to_vec())
}

/// Whether `signature` is the signature of `statement` under the RSA public key `key`.
///
/// "Hash and sign the policy statement … Use SHA-1 to hash and RSA to sign": the one
/// signature scheme CloudFront's signing procedure names, verified by the cryptography
/// provider this workspace builds.
fn verifies(key: &[u8], statement: &[u8], signature: &[u8]) -> bool {
    ::aws_lc_rs::signature::UnparsedPublicKey::new(
        &::aws_lc_rs::signature::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
        key,
    )
    .verify(statement, signature)
    .is_ok()
}

/// The value of the query parameter `name`, or `None` when the query string does not carry
/// it.
fn query_value(query: &str, name: &str) -> Option<String> {
    pair_value(super::forwarding::query_pairs(query), name)
}

/// The value of the cookie `name`, or `None` when the `Cookie` header does not carry it.
fn cookie_value(cookies: &str, name: &str) -> Option<String> {
    pair_value(super::forwarding::cookie_pairs(cookies), name)
}

/// The value of the pair `name`, read out of the pairs a query string or a `Cookie` header
/// was split into.
///
/// A parameter stated twice carries the FIRST value, which is the one a reader taking the
/// query string in the order the viewer sent it reads.
fn pair_value(pairs: Vec<super::forwarding::Pair<'_>>, name: &str) -> Option<String> {
    pairs
        .iter()
        .find(|pair| pair.name == name)
        .and_then(|pair| pair.text.split_once('='))
        .map(|(_name, value)| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signed request carrying `query` and `cookies`, for the resource `url`, from
    /// `address`.
    fn request<'a>(query: &'a str, cookies: &'a str, address: &'a str) -> SignedRequest<'a> {
        SignedRequest {
            query,
            cookies,
            resource_urls: vec!["https://d111.cloudfront.net/private/report.pdf".to_string()],
            address,
        }
    }

    #[test]
    fn the_restriction_is_the_enabled_list_that_names_a_group() {
        // Rows differ along what the behaviour's list says: no list at all, a list that is
        // not enabled, an enabled list naming nothing, an enabled list with an empty item
        // list, and an enabled list naming a group.
        let cases: [(&str, Option<TrustedKeyGroups>, bool); 5] = [
            ("no list at all", None, false),
            (
                "a list that is not enabled",
                Some(TrustedKeyGroups {
                    enabled: false,
                    quantity: 1,
                    items: Some(vec!["KG1".to_string()]),
                }),
                false,
            ),
            (
                "an enabled list naming nothing",
                Some(TrustedKeyGroups {
                    enabled: true,
                    quantity: 0,
                    items: None,
                }),
                false,
            ),
            (
                "an enabled list whose items are empty",
                Some(TrustedKeyGroups {
                    enabled: true,
                    quantity: 0,
                    items: Some(Vec::new()),
                }),
                false,
            ),
            (
                "an enabled list naming a group",
                Some(TrustedKeyGroups {
                    enabled: true,
                    quantity: 1,
                    items: Some(vec!["KG1".to_string()]),
                }),
                true,
            ),
        ];
        for (case, groups, expected) in cases {
            let actual = restricted(groups.as_ref());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected restricted == {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_request_naming_no_key_presents_nothing() {
        // Rows differ along WHERE the material is stated and whether the key is among it:
        // nothing at all, a query string with the key, cookies with the key, a query string
        // that states a signature but no key, and both, where the query string decides.
        let cases: [(&str, &str, &str, Option<&str>); 6] = [
            ("nothing at all", "", "", None),
            ("another parameter only", "nonce=7", "", None),
            (
                "a signed URL",
                "Expires=99&Signature=aa&Key-Pair-Id=K1",
                "",
                Some("K1"),
            ),
            (
                "signed cookies",
                "",
                "CloudFront-Expires=99; CloudFront-Signature=aa; CloudFront-Key-Pair-Id=K2",
                Some("K2"),
            ),
            (
                "a query string that states no key",
                "Expires=99&Signature=aa",
                "CloudFront-Expires=99; CloudFront-Signature=aa; CloudFront-Key-Pair-Id=K2",
                None,
            ),
            (
                "both, where the signed URL decides",
                "Expires=99&Signature=aa&Key-Pair-Id=K1",
                "CloudFront-Expires=99; CloudFront-Signature=aa; CloudFront-Key-Pair-Id=K2",
                Some("K1"),
            ),
        ];
        for (case, query, cookies, expected) in cases {
            let presented = Presented::of_request(&request(query, cookies, "127.0.0.1"));
            let actual = presented
                .as_ref()
                .ok()
                .map(|presented| presented.key_pair_id.as_str());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the key {expected:?}, got {actual:?}"
            );
            // A request that named no key is told exactly that, and never that its key was
            // unknown: there is no credential to report on.
            if expected.is_none() {
                assert_eq!(
                    presented.as_ref().err().copied(),
                    Some(Refusal::MissingKey),
                    "case {case:?}: expected the missing-key refusal, got {:?}",
                    presented.as_ref().err()
                );
            }
        }
    }

    #[test]
    fn the_signing_procedures_alphabet_is_read_back() {
        use ::base64::Engine as _;

        // Rows differ along which substituted characters the value carries: none, each one
        // on its own, all three, a percent-encoded value, and values that are no encoding at
        // all.
        let plain: Vec<u8> = (0u8..=255).collect();
        let standard = ::base64::engine::general_purpose::STANDARD.encode(&plain);
        let cloudfront: String = standard
            .chars()
            .map(|character| match character {
                '+' => '-',
                '=' => '_',
                '/' => '~',
                other => other,
            })
            .collect();
        assert_eq!(
            decode_base64(&cloudfront, MAX_POLICY_BYTES).as_deref(),
            Some(plain.as_slice()),
            "a value in the signing procedure's alphabet must decode to the bytes it \
             encodes, got {cloudfront:?}"
        );
        let cases: [(&str, &str, Option<&[u8]>); 5] = [
            ("a value needing no substitution", "YWJj", Some(b"abc")),
            ("a padded value", "YWJjZA__", Some(b"abcd")),
            ("a percent-encoded value", "YWJj%0A", None),
            ("a value that is not base64", "not base64!", None),
            ("the empty value", "", Some(b"")),
        ];
        for (case, value, expected) in cases {
            let actual = decode_base64(value, MAX_POLICY_BYTES);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?} ({value:?}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_source_address_condition_is_a_network() {
        // Rows differ along the form the condition names and whether the address lies in it:
        // a /32 of the address itself, a /32 of another address, a wider prefix that holds
        // it, a prefix that excludes it, every address, a bare address, an IPv6 prefix, and a
        // range that is not a network at all.
        let cases: [(&str, &str, &str, bool); 9] = [
            ("the address as a /32", "127.0.0.1/32", "127.0.0.1", true),
            ("another address as a /32", "203.0.113.9/32", "127.0.0.1", false),
            ("a prefix that holds it", "127.0.0.0/8", "127.0.0.1", true),
            ("a prefix that excludes it", "10.0.0.0/8", "127.0.0.1", false),
            ("every address", "0.0.0.0/0", "203.0.113.9", true),
            ("a bare address", "203.0.113.9", "203.0.113.9", true),
            ("another bare address", "203.0.113.9", "203.0.113.10", false),
            ("an IPv6 prefix", "2001:db8::/32", "2001:db8::1", true),
            ("a range that is not a network", "not-a-range", "127.0.0.1", false),
        ];
        for (case, range, address, expected) in cases {
            let actual = contains_address(range, address);
            assert_eq!(
                actual, expected,
                "case {case:?} ({range:?} against {address:?}): expected {expected}, got \
                 {actual}"
            );
        }
    }

    #[test]
    fn a_custom_policys_conditions_are_evaluated_against_the_request() {
        let resource = "https://d111.cloudfront.net/private/report.pdf";
        let at = |seconds: u64| UNIX_EPOCH + Duration::from_secs(seconds);
        // Rows differ along which condition the policy states and whether it holds: the
        // resource (exact, wildcard, another object), the expiry (ahead, passed), the start
        // (passed, ahead) and the address (holding, excluding).
        let cases: [(&str, String, u64, &str, bool); 10] = [
            (
                "the exact resource, not yet expired",
                policy(resource, 2_000, None, None),
                1_000,
                "127.0.0.1",
                true,
            ),
            (
                "a wildcard resource",
                policy("https://d111.cloudfront.net/private/*", 2_000, None, None),
                1_000,
                "127.0.0.1",
                true,
            ),
            (
                "a resource naming another object",
                policy("https://d111.cloudfront.net/public/*", 2_000, None, None),
                1_000,
                "127.0.0.1",
                false,
            ),
            (
                "an expiry that has passed",
                policy(resource, 2_000, None, None),
                2_000,
                "127.0.0.1",
                false,
            ),
            (
                "a start that has passed",
                policy(resource, 2_000, Some(500), None),
                1_000,
                "127.0.0.1",
                true,
            ),
            (
                "a start that is still ahead",
                policy(resource, 2_000, Some(1_500), None),
                1_000,
                "127.0.0.1",
                false,
            ),
            (
                "an address range that holds the viewer",
                policy(resource, 2_000, None, Some("127.0.0.1/32")),
                1_000,
                "127.0.0.1",
                true,
            ),
            (
                "an address range that excludes the viewer",
                policy(resource, 2_000, None, Some("10.0.0.0/8")),
                1_000,
                "127.0.0.1",
                false,
            ),
            (
                "a policy that states no expiry",
                "{\"Statement\":[{\"Resource\":\"*\",\"Condition\":{}}]}".to_string(),
                1_000,
                "127.0.0.1",
                false,
            ),
            (
                "a document that is not a policy",
                "{\"Statement\":\"everything\"}".to_string(),
                1_000,
                "127.0.0.1",
                false,
            ),
        ];
        for (case, statement, now, address, expected) in cases {
            let actual = admits(
                statement.as_bytes(),
                &request("", "", address),
                at(now),
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected admits == {expected}, got {actual} (policy \
                 {statement})"
            );
        }
    }

    /// A custom policy statement naming `resource`, expiring at `expires`, optionally
    /// starting at `starts` and optionally restricted to `source_ip`.
    fn policy(
        resource: &str,
        expires: u64,
        starts: Option<u64>,
        source_ip: Option<&str>,
    ) -> String {
        let mut conditions = vec![format!("\"DateLessThan\":{{\"{EPOCH_TIME}\":{expires}}}")];
        if let Some(starts) = starts {
            conditions.push(format!("\"DateGreaterThan\":{{\"{EPOCH_TIME}\":{starts}}}"));
        }
        if let Some(source_ip) = source_ip {
            conditions.push(format!("\"IpAddress\":{{\"{SOURCE_IP}\":\"{source_ip}\"}}"));
        }
        format!(
            "{{\"Statement\":[{{\"Resource\":\"{resource}\",\"Condition\":{{{}}}}}]}}",
            conditions.join(",")
        )
    }

    #[test]
    fn the_canned_statement_is_the_documented_rendering() {
        assert_eq!(
            canned_statement("https://d111.cloudfront.net/obj", 1_357_034_400),
            "{\"Statement\":[{\"Resource\":\"https://d111.cloudfront.net/obj\",\"Condition\":\
             {\"DateLessThan\":{\"AWS:EpochTime\":1357034400}}}]}",
            "the canned policy statement is the bytes the signature is made over, so its \
             rendering is the contract"
        );
    }

    #[test]
    fn every_refusal_carries_its_own_code() {
        let cases: [(Refusal, &str); 3] = [
            (Refusal::MissingKey, "MissingKey"),
            (Refusal::InvalidKey, "InvalidKey"),
            (Refusal::AccessDenied, "AccessDenied"),
        ];
        for (refusal, code) in cases {
            assert_eq!(
                refusal.code(),
                code,
                "case {refusal:?}: expected the code {code:?}, got {:?}",
                refusal.code()
            );
            assert!(
                !refusal.message().is_empty(),
                "case {refusal:?}: every refusal states a message"
            );
        }
    }
}
