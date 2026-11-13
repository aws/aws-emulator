// SPDX-License-Identifier: CC0-1.0

//! SERVER-SIDE ENCRYPTION WITH A CUSTOMER-PROVIDED KEY — what a write that supplies its own
//! key records about the object, and what a later read of that object has to present.
//!
//! "Server-side encryption with customer-provided keys (SSE-C) allows you to set your own
//! encryption keys. … Amazon S3 does not store the encryption key you provide. Instead, we
//! store a randomly salted HMAC value of the encryption key to validate future requests"
//! (Amazon S3 User Guide, *Using server-side encryption with customer-provided keys*). So the
//! object carries the ALGORITHM it was encrypted under and a DIGEST of the key, and nothing
//! that can reproduce the key itself; both are what `PutObject`, `GetObject` and `HeadObject`
//! report back in `x-amz-server-side-encryption-customer-algorithm` and
//! `x-amz-server-side-encryption-customer-key-MD5`.
//!
//! Three headers carry one key ([`SSECustomerAlgorithm`, `SSECustomerKey`,
//! `SSECustomerKeyMD5`] as the model names the members): the algorithm, the key itself and
//! the client's own MD5 of it, which "Amazon S3 uses … for a message integrity check to
//! ensure that the encryption key was transmitted without error"
//! (`PutObjectRequest$SSECustomerKeyMD5`). A request that supplies one of them supplies all
//! three, each is checked here, and the request is refused — nothing stored, nothing served —
//! when any of them does not hold. The ONE place the three are read, so every write and every
//! read of this service judges a customer key alike.
//!
//! And three conditions decide a READ of an object stored this way ([`read_refusal`]): a read
//! presenting no key cannot be served at all, a read presenting a key whose digest is not the
//! object's is refused the way S3 refuses a caller that is not authorized for the object's
//! plaintext, and a read presenting a key for an object stored under some OTHER form of
//! encryption names parameters that do not apply to it. The decision is the digest the object
//! carries against the digest of the key the request carried, which is the only comparison S3
//! itself can make either.
//!
//! What is NOT here: any cryptography. This emulator holds an object's bytes in this
//! process's memory and encrypts nothing (the same stance [`super::ObjectEncryption`] records
//! for every other form of server-side encryption), so what it can answer faithfully is which
//! configuration the object was stored under and whether the key a read presents is the one
//! the write supplied — which is what every response member and every refusal above is about.
//! The caller's key is read for its digest and dropped with the request: nothing here stores
//! it, logs it or answers it back.

use ::base64::Engine as _;

/// The one algorithm S3 serves a customer-provided key under: "`AES256` is the only supported
/// value" (`PutObjectRequest$SSECustomerAlgorithm`).
pub const SSE_CUSTOMER_ALGORITHM: &str = "AES256";

/// The length of such a key, in bytes: a 256-bit key, which is what that algorithm takes
/// ("Specifies the customer-provided encryption key for Amazon S3 to use in encrypting data",
/// `PutObjectRequest$SSECustomerKey`, under `AES256`).
const SSE_CUSTOMER_KEY_BYTES: usize = 32;

/// The number of base64 characters [`SSE_CUSTOMER_KEY_BYTES`] encodes to: four characters per
/// three bytes, padded. Longer text names no key of that length, so it is refused on its
/// LENGTH before anything decodes it — which is what keeps the work one header can ask for
/// bounded by the key the algorithm takes rather than by what the caller sent.
const SSE_CUSTOMER_KEY_BASE64_CHARS: usize = SSE_CUSTOMER_KEY_BYTES.div_ceil(3) * 4;

/// The sentence S3 answers a request whose SSE-C headers name no algorithm it serves — the
/// header missing where another one of the three is present, or carrying a value that is not
/// [`SSE_CUSTOMER_ALGORITHM`]. Fixed text, naming neither the header nor the value.
const INVALID_ALGORITHM_MESSAGE: &str =
    "Requests specifying Server Side Encryption with Customer provided keys must provide a \
     valid encryption algorithm.";

/// The sentence S3 answers a request whose SSE-C headers carry no usable KEY: none at all, or
/// one that is not a base64-encoded key of the length the algorithm takes.
const INVALID_KEY_MESSAGE: &str =
    "Requests specifying Server Side Encryption with Customer provided keys must provide an \
     appropriate secret key.";

/// The sentence S3 answers a request that supplies a key and no digest of it: the integrity
/// check that header is for cannot be made at all.
const MISSING_KEY_DIGEST_MESSAGE: &str =
    "Requests specifying Server Side Encryption with Customer provided keys must provide the \
     client calculated MD5 of the secret key.";

/// The sentence S3 answers a request whose digest does not match the key beside it — the
/// integrity check `x-amz-server-side-encryption-customer-key-MD5` exists for, failing.
const KEY_DIGEST_MISMATCH_MESSAGE: &str =
    "The calculated MD5 hash of the key did not match the hash that was provided.";

/// The sentence S3 answers a request that supplies its own key AND asks for an S3-managed
/// algorithm: "the x-amz-server-side-encryption header is not supported with SSE-C"
/// (`PutObjectRequest$SSECustomerAlgorithm`), so the two are alternatives and a request
/// naming both names no one encryption the object could be stored under.
const COMBINED_WITH_MANAGED_MESSAGE: &str =
    "Requests specifying Server Side Encryption with Customer provided keys must not specify \
     the x-amz-server-side-encryption header.";

/// The code that one carries: the digest a request computed for its own payload — here, for
/// its key — is not the digest the service computes ("InvalidDigest — The Content-MD5 or
/// checksum value that you specified is not valid", Amazon S3 "Error responses"), HTTP 400.
const INVALID_DIGEST_CODE: &str = "InvalidDigest";

/// The sentence S3 answers a read of an object stored under a customer-provided key that
/// presents none: the object's plaintext comes of the caller's own key, so a read that
/// supplies nothing cannot be served.
const KEY_REQUIRED_MESSAGE: &str =
    "The object was stored using a form of Server Side Encryption. The correct parameters \
     must be provided to retrieve the object.";

/// The sentence S3 answers a read that presents a customer key for an object stored under
/// some OTHER form of encryption: the parameters are well formed and name a key the object
/// has nothing to do with.
const KEY_NOT_APPLICABLE_MESSAGE: &str = "The encryption parameters are not applicable to this \
                                          object.";

/// The sentence S3 answers a PART of a multipart upload that was initiated under a
/// customer-provided key when the part presents none: "this must be the same encryption key
/// specified in the initiate multipart upload request" (`UploadPartRequest$SSECustomerKey`),
/// so what the caller is told is about the UPLOAD's parameters — the initiation asked for
/// encryption and every part of it has to carry the same key — rather than about reading a
/// stored object's bytes.
const INITIATE_ENCRYPTED_MESSAGE: &str =
    "The multipart upload initiate requested encryption. Subsequent part requests must \
     include the appropriate encryption parameters.";

/// The sentence S3 answers a PART that presents a DIFFERENT customer-provided key from the
/// one its upload was initiated under: "this must be the same encryption key specified in the
/// initiate multipart upload request" (`UploadPartRequest$SSECustomerKey`), so what the caller
/// is told is that the parameters are not the upload's own — a statement about the REQUEST,
/// which is why it carries [`super::INVALID_REQUEST_CODE`] rather than the 403 a read of a
/// stored object under another key earns.
const PARAMETERS_MISMATCH_MESSAGE: &str =
    "The provided encryption parameters did not match the ones used originally.";

/// The customer-provided key one request carries, as this service keeps it: the algorithm it
/// names and the DIGEST of the key, which is all an object records of it and all a later read
/// is compared against. The key itself is read for its digest and dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerKey {
    /// `SSECustomerAlgorithm` — [`SSE_CUSTOMER_ALGORITHM`], the one value S3 serves.
    pub algorithm: String,
    /// `SSECustomerKeyMD5` — the base64 MD5 of the key, as the request stated it and as this
    /// service recomputed it from the key (the two are equal or the request was refused).
    pub key_md5: String,
}

/// Why a request's SSE-C headers cannot be used, in the order they are judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomerKeyRejection {
    /// The algorithm is absent or is not one S3 serves.
    Algorithm,
    /// The key is absent, or is not a base64 key of the length the algorithm takes.
    Key,
    /// No digest of the key was supplied.
    MissingDigest,
    /// The digest supplied is not the digest of the key supplied.
    DigestMismatch,
    /// The request supplies its own key and also asks for an S3-managed algorithm.
    CombinedWithManaged,
}

impl CustomerKeyRejection {
    /// The wire error code this refusal carries.
    pub fn code(self) -> &'static str {
        match self {
            Self::DigestMismatch => INVALID_DIGEST_CODE,
            _ => super::INVALID_ARGUMENT_CODE,
        }
    }

    /// The HTTP status it is answered with: 400 for every one of them — each is a value the
    /// request itself carried that S3 cannot use.
    pub fn status(self) -> u16 {
        super::INVALID_ARGUMENT_STATUS
    }

    /// The sentence S3 words it in.
    pub fn message(self) -> &'static str {
        match self {
            Self::Algorithm => INVALID_ALGORITHM_MESSAGE,
            Self::Key => INVALID_KEY_MESSAGE,
            Self::MissingDigest => MISSING_KEY_DIGEST_MESSAGE,
            Self::DigestMismatch => KEY_DIGEST_MISMATCH_MESSAGE,
            Self::CombinedWithManaged => COMBINED_WITH_MANAGED_MESSAGE,
        }
    }
}

/// The customer key the three SSE-C members carry, `None` for a request that carries none of
/// them, or why the ones it carries cannot be used.
///
/// The order is the headers' own: the algorithm, which says the request is an SSE-C one at
/// all; then the key, which that algorithm takes; then the digest of it, which is the
/// integrity check the third header is for; and last what the request asks of S3's own
/// encryption (`server_side_encryption`, the `x-amz-server-side-encryption` value it
/// carried), which a request supplying its own key may not also name.
pub fn requested_customer_key(
    algorithm: Option<&str>,
    key: Option<&str>,
    key_md5: Option<&str>,
    server_side_encryption: Option<&str>,
) -> Result<Option<CustomerKey>, CustomerKeyRejection> {
    fn present(member: Option<&str>) -> Option<&str> {
        member.filter(|value| !value.is_empty())
    }
    let (algorithm, key, key_md5) = (present(algorithm), present(key), present(key_md5));
    if algorithm.is_none() && key.is_none() && key_md5.is_none() {
        return Ok(None);
    }
    let Some(SSE_CUSTOMER_ALGORITHM) = algorithm else {
        return Err(CustomerKeyRejection::Algorithm);
    };
    let key = key.ok_or(CustomerKeyRejection::Key)?;
    if key.len() > SSE_CUSTOMER_KEY_BASE64_CHARS {
        return Err(CustomerKeyRejection::Key);
    }
    let decoded = ::base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| CustomerKeyRejection::Key)?;
    if decoded.len() != SSE_CUSTOMER_KEY_BYTES {
        return Err(CustomerKeyRejection::Key);
    }
    let stated = key_md5.ok_or(CustomerKeyRejection::MissingDigest)?;
    let computed = key_digest(&decoded);
    if computed != stated {
        return Err(CustomerKeyRejection::DigestMismatch);
    }
    if server_side_encryption.is_some_and(|algorithm| !algorithm.is_empty()) {
        return Err(CustomerKeyRejection::CombinedWithManaged);
    }
    Ok(Some(CustomerKey {
        algorithm: SSE_CUSTOMER_ALGORITHM.to_string(),
        key_md5: computed,
    }))
}

/// The base64 MD5 of the key bytes `key` — the form
/// `x-amz-server-side-encryption-customer-key-MD5` carries ("Specifies the 128-bit MD5 digest
/// of the encryption key according to RFC 1321", `PutObjectRequest$SSECustomerKeyMD5`).
///
/// The same computation the `Content-MD5` of a payload is checked by, so it is the same
/// helper ([`super::content_md5`]): one digest-and-encode in this crate, not two.
fn key_digest(key: &[u8]) -> String {
    super::content_md5(key)
}

/// One refusal of a READ, as the operation answering it renders it: the code, the status and
/// the sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRefusal {
    /// The `<Code>` a client branches on.
    pub code: &'static str,
    /// The HTTP status it is answered with.
    pub status: u16,
    /// The `<Message>`. Fixed text for each condition: none of them states a value from the
    /// request, and none of them says anything about the key.
    pub message: &'static str,
}

/// Why a read presenting `presented` cannot be served the object whose `stored` encryption is
/// given, or `None` when it can.
///
/// Three conditions, each the whole of what the caller is told:
///
/// 1. the object was stored under a customer key and the read presents NONE — the plaintext
///    comes of that key, so there is nothing to serve;
/// 2. it presents a key whose DIGEST is not the object's — the key cannot open the object,
///    which S3 answers the way it answers any caller not authorized for its plaintext
///    ([`super::ACCESS_DENIED_CODE`], HTTP 403): the refusal says nothing about the key it
///    was given;
/// 3. the object was stored under some other form of encryption and the read presents a key
///    — well-formed parameters that do not apply to this object.
pub fn read_refusal(
    stored: &super::ObjectEncryption,
    presented: Option<&CustomerKey>,
) -> Option<ReadRefusal> {
    let refusal = |code: &'static str, status: u16, message: &'static str| {
        Some(ReadRefusal {
            code,
            status,
            message,
        })
    };
    match (stored.sse_customer_key_md5.as_deref(), presented) {
        (None, None) => None,
        (None, Some(_)) => refusal(
            super::INVALID_REQUEST_CODE,
            super::INVALID_REQUEST_STATUS,
            KEY_NOT_APPLICABLE_MESSAGE,
        ),
        (Some(_), None) => refusal(
            super::INVALID_REQUEST_CODE,
            super::INVALID_REQUEST_STATUS,
            KEY_REQUIRED_MESSAGE,
        ),
        (Some(stored), Some(presented)) if stored != presented.key_md5 => refusal(
            super::ACCESS_DENIED_CODE,
            super::ACCESS_DENIED_STATUS,
            super::ACCESS_DENIED_MESSAGE,
        ),
        (Some(_), Some(_)) => None,
    }
}

/// Why a PART of the multipart upload whose `initiated` encryption is given cannot be written
/// presenting `presented`, or `None` when it can.
///
/// The same comparison a READ of a stored object is decided by ([`read_refusal`]) — the digest
/// the upload recorded against the digest of the key the part carried — because "this must be
/// the same encryption key specified in the initiate multipart upload request"
/// (`UploadPartRequest$SSECustomerKey`). What differs is WHAT THE CALLER IS TOLD for the two
/// conditions that are about the upload rather than about an object's bytes, because there is
/// no stored object yet for the read's own sentences to be about:
///
/// * a part presenting NO key is told the initiation asked for encryption
///   ([`INITIATE_ENCRYPTED_MESSAGE`]); and
/// * a part presenting ANOTHER key is told its parameters are not the ones the upload was
///   initiated with ([`PARAMETERS_MISMATCH_MESSAGE`]).
///
/// Both are a request S3 cannot serve ([`super::INVALID_REQUEST_CODE`], HTTP 400). Every other
/// condition is the read's own.
pub fn part_refusal(
    initiated: &super::ObjectEncryption,
    presented: Option<&CustomerKey>,
) -> Option<ReadRefusal> {
    let refused = |message: &'static str| {
        Some(ReadRefusal {
            code: super::INVALID_REQUEST_CODE,
            status: super::INVALID_REQUEST_STATUS,
            message,
        })
    };
    match (initiated.sse_customer_key_md5.as_deref(), presented) {
        (Some(_), None) => refused(INITIATE_ENCRYPTED_MESSAGE),
        (Some(stored), Some(presented)) if stored != presented.key_md5 => {
            refused(PARAMETERS_MISMATCH_MESSAGE)
        }
        _ => read_refusal(initiated, presented),
    }
}

/// The encryption an object stored under the customer key `key` carries: the algorithm and
/// the digest, and none of the members another form of encryption has — an SSE-C object
/// reports no `x-amz-server-side-encryption` at all, which is what tells a client which form
/// it was stored under.
pub fn customer_key_encryption(key: &CustomerKey) -> super::ObjectEncryption {
    super::ObjectEncryption {
        sse_customer_algorithm: Some(key.algorithm.clone()),
        sse_customer_key_md5: Some(key.key_md5.clone()),
        ..super::ObjectEncryption::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of `SSE_CUSTOMER_KEY_BYTES` bytes, base64 encoded, with its digest — the pair a
    /// client's SDK computes for it.
    fn key_pair(seed: u8) -> (String, String) {
        let key: Vec<u8> = (0..SSE_CUSTOMER_KEY_BYTES)
            .map(|index| (index as u8).wrapping_mul(7).wrapping_add(seed))
            .collect();
        (
            ::base64::engine::general_purpose::STANDARD.encode(&key),
            key_digest(&key),
        )
    }

    /// One row of the member table: the case, the three members as the request carried them,
    /// and the rejection expected (`None` for a request whose members name one usable key, or
    /// none at all).
    type MemberRow<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<CustomerKeyRejection>,
    );

    /// Every way the three members can fail to name one usable key, and the one way they can.
    #[test]
    fn the_three_members_are_judged_in_the_headers_own_order() {
        let (key, digest) = key_pair(1);
        let (other_key, other_digest) = key_pair(2);
        let short = ::base64::engine::general_purpose::STANDARD.encode([0_u8; 16]);
        // (case, algorithm, key, digest, the rejection expected)
        let rows: [MemberRow<'_>; 9] = [
            ("no member at all", None, None, None, None),
            (
                "every member, consistent",
                Some(SSE_CUSTOMER_ALGORITHM),
                Some(&key),
                Some(&digest),
                None,
            ),
            (
                "a key with no algorithm",
                None,
                Some(&key),
                Some(&digest),
                Some(CustomerKeyRejection::Algorithm),
            ),
            (
                "an algorithm S3 does not serve",
                Some("AES128"),
                Some(&key),
                Some(&digest),
                Some(CustomerKeyRejection::Algorithm),
            ),
            (
                "an algorithm and no key",
                Some(SSE_CUSTOMER_ALGORITHM),
                None,
                Some(&digest),
                Some(CustomerKeyRejection::Key),
            ),
            (
                "a key that is not base64",
                Some(SSE_CUSTOMER_ALGORITHM),
                Some("not base64 at all!"),
                Some(&digest),
                Some(CustomerKeyRejection::Key),
            ),
            (
                "a key of the wrong length",
                Some(SSE_CUSTOMER_ALGORITHM),
                Some(&short),
                Some(&digest),
                Some(CustomerKeyRejection::Key),
            ),
            (
                "a key with no digest",
                Some(SSE_CUSTOMER_ALGORITHM),
                Some(&key),
                None,
                Some(CustomerKeyRejection::MissingDigest),
            ),
            (
                "another key's digest",
                Some(SSE_CUSTOMER_ALGORITHM),
                Some(&other_key),
                Some(&digest),
                Some(CustomerKeyRejection::DigestMismatch),
            ),
        ];
        for (case, algorithm, provided, stated, expected) in rows {
            let outcome = requested_customer_key(algorithm, provided, stated, None);
            assert_eq!(
                outcome.as_ref().err().copied(),
                expected,
                "case {case:?}: expected {expected:?}, got {outcome:?}"
            );
        }
        // A request supplying its own key may not also ask for an S3-managed algorithm.
        let combined = requested_customer_key(
            Some(SSE_CUSTOMER_ALGORITHM),
            Some(&key),
            Some(&digest),
            Some("aws:kms"),
        );
        assert_eq!(
            combined.as_ref().err().copied(),
            Some(CustomerKeyRejection::CombinedWithManaged),
            "case \"a customer key beside an S3-managed algorithm\": expected the combination \
             to be refused, got {combined:?}"
        );
        // The one that is accepted names the digest of the key it was given, which is what a
        // later read is matched against.
        let accepted =
            requested_customer_key(Some(SSE_CUSTOMER_ALGORITHM), Some(&key), Some(&digest), None);
        assert_eq!(
            accepted.expect("a consistent key must be accepted"),
            Some(CustomerKey {
                algorithm: SSE_CUSTOMER_ALGORITHM.to_string(),
                key_md5: digest.clone(),
            }),
            "a consistent key must be kept as its algorithm and its digest"
        );
        assert_ne!(
            digest, other_digest,
            "two different keys must have different digests"
        );
    }

    /// The three conditions a read of an SSE-C object is decided by, and the two cases that
    /// are not a refusal at all.
    #[test]
    fn a_read_is_decided_by_the_digest_the_object_carries() {
        let (_, digest) = key_pair(3);
        let (_, other_digest) = key_pair(4);
        let stored = super::super::ObjectEncryption {
            sse_customer_algorithm: Some(SSE_CUSTOMER_ALGORITHM.to_string()),
            sse_customer_key_md5: Some(digest.clone()),
            ..super::super::ObjectEncryption::default()
        };
        let managed = super::super::ObjectEncryption {
            sse_algorithm: Some("AES256".to_string()),
            ..super::super::ObjectEncryption::default()
        };
        let key = |md5: &str| CustomerKey {
            algorithm: SSE_CUSTOMER_ALGORITHM.to_string(),
            key_md5: md5.to_string(),
        };
        // (case, the object's encryption, the key the read presents, the code expected)
        let rows: [(&str, &super::super::ObjectEncryption, Option<CustomerKey>, Option<&str>); 5] = [
            ("the key it was stored with", &stored, Some(key(&digest)), None),
            (
                "another key",
                &stored,
                Some(key(&other_digest)),
                Some(super::super::ACCESS_DENIED_CODE),
            ),
            (
                "no key at all",
                &stored,
                None,
                Some(super::super::INVALID_REQUEST_CODE),
            ),
            (
                "a key for an object stored without one",
                &managed,
                Some(key(&digest)),
                Some(super::super::INVALID_REQUEST_CODE),
            ),
            ("no key for an object stored without one", &managed, None, None),
        ];
        for (case, encryption, presented, expected) in rows {
            let refusal = read_refusal(encryption, presented.as_ref());
            assert_eq!(
                refusal.map(|refusal| refusal.code),
                expected,
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }
}
