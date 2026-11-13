// SPDX-License-Identifier: CC0-1.0

//! What an object WRITE must satisfy about the name and the class it names, before anything
//! is stored: the bytes an object key may carry, the storage classes the bucket it is
//! written into can host, and the query parameters that bucket's endpoint serves at all.
//!
//! Every rule here is the bucket's and the key's rather than one operation's, so every write
//! that names a key or a class — `PutObject`, `CopyObject`, `CreateMultipartUpload` — is
//! judged by the same functions here and answers the same code for the same value.

use crate::state::{self, Bucket};

/// The code S3 answers a key outside the published byte rules with, and the status it carries
/// (Amazon S3 "Error responses", list of error codes: "KeyTooLongError — Your key is too
/// long", HTTP 400).
pub const KEY_TOO_LONG_CODE: &str = "KeyTooLongError";

/// The code naming the refusal that carries NO error document at all: HTTP 400 with an empty
/// body, which is what S3 answers a request whose URI could not be read as a request in the
/// first place — the rejection happens before the S3 API layer, so there is no `<Code>` for a
/// client to branch on and an SDK reports the STATUS as the code it saw.
///
/// A key carrying a NUL (0x00) byte is that request: no URI can convey the byte, so there is
/// no addressable object for the API to refuse. [`crate::service::wire`] renders a refusal
/// naming this code as the bare status it is.
pub const ERRORLESS_REFUSAL_CODE: &str = "";

/// The status both refusals are answered with.
pub const KEY_REFUSAL_STATUS: u16 = 400;

/// The code S3 answers a storage class the addressed bucket cannot host with, and the
/// sentence that code carries (same published list: "InvalidStorageClass — The storage class
/// you specified is not valid", HTTP 400).
pub const INVALID_STORAGE_CLASS_CODE: &str = "InvalidStorageClass";
/// See [`INVALID_STORAGE_CLASS_CODE`].
pub const INVALID_STORAGE_CLASS_MESSAGE: &str = "The storage class you specified is not valid";
/// See [`INVALID_STORAGE_CLASS_CODE`].
pub const INVALID_STORAGE_CLASS_STATUS: u16 = 400;

/// The `StorageClass` values a GENERAL PURPOSE bucket hosts.
///
/// The model's `StorageClass` enum publishes fifteen values
/// (`smithy_query(query="shape",shape="StorageClass")`), and `PutObjectRequest$StorageClass`
/// states which of them belong to another kind of bucket: "Directory buckets only support
/// `EXPRESS_ONEZONE` (the S3 Express One Zone storage class) in Availability Zones and
/// `ONEZONE_IA` (the S3 One Zone-Infrequent Access storage class) in Dedicated Local Zones"
/// and "Amazon S3 on Outposts only uses the OUTPOSTS Storage Class". So a general purpose
/// bucket hosts every published class but those two — `EXPRESS_ONEZONE`, which is a directory
/// bucket's, and `OUTPOSTS`, which is an S3 on Outposts bucket's.
pub const GENERAL_PURPOSE_STORAGE_CLASSES: &[&str] = &[
    "STANDARD",
    "REDUCED_REDUNDANCY",
    "STANDARD_IA",
    "ONEZONE_IA",
    "INTELLIGENT_TIERING",
    "GLACIER",
    "DEEP_ARCHIVE",
    "GLACIER_IR",
    "SNOW",
    "FSX_OPENZFS",
    "FSX_ONTAP",
    "AWS_BACKUP_WARM",
    "AWS_BACKUP_LOW_COST_WARM",
];

/// The `StorageClass` values a DIRECTORY bucket hosts — the two that same member names for
/// one: `EXPRESS_ONEZONE` in an Availability Zone and `ONEZONE_IA` in a Dedicated Local Zone.
pub const DIRECTORY_BUCKET_STORAGE_CLASSES: &[&str] =
    &[state::EXPRESS_ONEZONE_STORAGE_CLASS, "ONEZONE_IA"];

/// The classes `bucket` can host, by the kind of bucket it is.
pub fn hosted_storage_classes(bucket: &Bucket) -> &'static [&'static str] {
    if bucket.is_directory() {
        DIRECTORY_BUCKET_STORAGE_CLASSES
    } else {
        GENERAL_PURPOSE_STORAGE_CLASSES
    }
}

/// Why a write naming `storage_class` into `bucket` is refused, or `None` for a class that
/// bucket hosts (and for a write that names none, which is stored in the bucket's own
/// default).
///
/// The class a write names has to be one the bucket it addresses can hold: the object would
/// otherwise be stored under a class the bucket cannot host, which every later read, lifecycle
/// transition and restore of it would then report. The value is not valid for the request,
/// which is what `InvalidStorageClass` reports, and it is judged before anything is written.
pub fn storage_class_refusal(bucket: &Bucket, storage_class: Option<&str>) -> Option<&'static str> {
    let named = storage_class?;
    (!hosted_storage_classes(bucket).contains(&named)).then_some(INVALID_STORAGE_CLASS_MESSAGE)
}

/// Why the object key `key` is refused, as `(code, message)`, or `None` for a key S3 stores.
///
/// Two published rules about the NAME, both of them about the bytes it is made of:
///
/// * "The name for a key is a sequence of Unicode characters whose UTF-8 encoding is at most
///   1,024 bytes long" (Amazon S3 User Guide, *Creating object key names*) — a longer name is
///   `KeyTooLongError`, the code a client branching on the limit reads;
/// * a NUL (0x00) byte can be conveyed by no request URI, so a key carrying one is not a
///   request the API gets to refuse at all: it is answered with the bare 400 that carries no
///   error document ([`ERRORLESS_REFUSAL_CODE`]).
///
/// Judged before anything is stored, so a refused write leaves whatever the key held exactly
/// as it was.
pub fn key_refusal(key: &str) -> Option<(&'static str, String)> {
    if key.contains('\0') {
        return Some((ERRORLESS_REFUSAL_CODE, String::new()));
    }
    (key.len() > state::MAX_OBJECT_KEY_BYTES)
        .then(|| (KEY_TOO_LONG_CODE, KEY_TOO_LONG_MESSAGE.to_string()))
}

/// The sentence S3 answers a key past the published ceiling with: fixed text, naming neither
/// the key nor the ceiling (Amazon S3 "Error responses": "KeyTooLongError — Your key is too
/// long").
pub const KEY_TOO_LONG_MESSAGE: &str = "Your key is too long";

/// The query parameter that addresses the RENAME of an object: `?renameObject`, as the model
/// pins it on the operation's URI (`smithy_query(query="operation",operation="RenameObject")`
/// reports `http {method: "PUT", uri: "/{Bucket}/{Key+}?renameObject"}`).
pub const RENAME_OBJECT_PARAMETER: &str = "renameObject";

/// The query parameters of an object WRITE that S3 serves for DIRECTORY buckets alone.
///
/// `RenameObject` "renames an existing object in a directory bucket that uses the S3 Express
/// One Zone storage class" (its own documentation) and is addressed by the one parameter
/// above, so a general purpose bucket's endpoint implements no functionality for it: the
/// PARAMETER, not the value in it, is what cannot be served there — which is the 501
/// `NotImplemented` S3 answers a query parameter it cannot honour with
/// ([`crate::scaffolded::Scaffolded::not_implemented_query_parameter`]), the query-parameter
/// sibling of the refusal a directory-bucket-only conditional HEADER earns.
pub const DIRECTORY_BUCKET_ONLY_QUERY_PARAMETERS: [&str; 1] = [RENAME_OBJECT_PARAMETER];

/// The directory-bucket-only query parameter `query` carries, or `None` when it carries none
/// — the one rule every object write addressed to a general purpose bucket is judged by,
/// whichever operation the request's shape routes it to.
///
/// `query` is the raw query string of the request (`ctx.parts().uri.query()`), because the
/// parameter is a discriminator the matched operation's own members do not carry: a write
/// that names it reaches `PutObject` when it is shaped like one, and `RenameObject` when it
/// carries that operation's required members. Each name is looked for through the one reader
/// this crate asks a query string for a parameter with ([`crate::service::query_carries`]),
/// so a parameter arriving as a bare flag is found here exactly as it is everywhere else.
pub fn directory_bucket_only_query_parameter(query: &str) -> Option<&'static str> {
    DIRECTORY_BUCKET_ONLY_QUERY_PARAMETERS
        .into_iter()
        .find(|parameter| crate::service::query_carries(query, parameter))
}

/// The server-side encryption algorithm a Zonal endpoint write into the directory bucket
/// `bucket` is judged against — the algorithm the write's own `x-amz-server-side-encryption`
/// has to match ([`session_encryption_refusal`]) — or `None` for a request that presents no
/// live session of that bucket, which committed nothing for a write to differ from.
///
/// "In the Zonal endpoint API calls … the encryption request headers must match the encryption
/// settings that are specified in the `CreateSession` request"
/// (`PutObjectRequest$ServerSideEncryption`, of a directory bucket), so it is the algorithm the
/// SESSION the request presents was established with: a session that named one was established
/// under it, and one that named none under the bucket's own default encryption configuration
/// (`CreateSession` settles that, which is why this reads the session rather than the bucket).
///
/// Read-only: the session is read, and nothing is written.
pub fn committed_encryption(
    store: &state::ComAmazonawsS3Amazons3State,
    bucket: &str,
    headers: &::http::HeaderMap,
    now: ::std::time::SystemTime,
) -> Option<String> {
    let presented = state::presented_session(headers)?;
    let scope = store.presented_session_scope(&presented, now)?;
    if scope.bucket != bucket {
        return None;
    }
    scope.encryption_algorithm.clone()
}

/// The sentence S3 refuses a Zonal endpoint write naming `requested` under a session
/// established with `committed` with, or `None` when the two name one encryption.
///
/// "You can't override the values of the encryption settings … that are specified in the
/// `CreateSession` request. You don't need to explicitly specify these encryption settings
/// values in Zonal endpoint API calls, and Amazon S3 will use the encryption settings values
/// from the `CreateSession` request to protect new objects in the directory bucket"
/// (`PutObjectRequest$ServerSideEncryption`) — so a write that names NO algorithm is stored
/// under the session's own and matches it, and a write that names ANOTHER one names an
/// encryption the object cannot be stored under, which is a request S3 cannot serve
/// ([`state::INVALID_REQUEST_CODE`], HTTP 400).
///
/// The sentence is that class's fixed text with the two algorithms substituted, each named the
/// way S3 names an encryption type in it ([`encryption_type_name`]).
pub fn session_encryption_refusal(
    committed: Option<&str>,
    requested: Option<&str>,
) -> Option<String> {
    let requested = requested.filter(|algorithm| !algorithm.is_empty())?;
    let committed = committed.filter(|algorithm| !algorithm.is_empty())?;
    (committed != requested).then(|| {
        format!(
            "Session was created with ServerSideEncryption as {} but request header specifies \
             {}.",
            encryption_type_name(committed),
            encryption_type_name(requested)
        )
    })
}

/// How S3 names a server-side encryption type in that sentence: the published SHORT NAME of
/// each KMS form — "server-side encryption with KMS keys (SSE-KMS) (`aws:kms`)" and its
/// dual-layer sibling (`PutObjectRequest$ServerSideEncryption`) — and the model's own
/// `ServerSideEncryption` value for every other, which for SSE-S3 is the algorithm name
/// `AES256` itself.
fn encryption_type_name(algorithm: &str) -> &str {
    match algorithm {
        kms if kms == state::KMS_SSE_ALGORITHMS[0] => "SSE-KMS",
        dsse if dsse == state::KMS_SSE_ALGORITHMS[1] => "DSSE-KMS",
        named => named,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key rules, at the ceiling and one byte past it, and over a key carrying the one
    /// byte no URI conveys.
    #[test]
    fn a_key_is_refused_for_its_bytes_alone() {
        let rows: [(&str, String, Option<&'static str>); 6] = [
            ("a short key", "reports/held.csv".to_string(), None),
            ("a one-byte key", "k".to_string(), None),
            (
                "a key of exactly the published maximum",
                "k".repeat(state::MAX_OBJECT_KEY_BYTES),
                None,
            ),
            (
                "a key one byte past the maximum",
                "k".repeat(state::MAX_OBJECT_KEY_BYTES + 1),
                Some(KEY_TOO_LONG_CODE),
            ),
            (
                "a multi-byte key of exactly the published maximum in BYTES",
                "é".repeat(state::MAX_OBJECT_KEY_BYTES / 2),
                None,
            ),
            (
                "a key carrying a NUL byte",
                "nul/a\0b.txt".to_string(),
                Some(ERRORLESS_REFUSAL_CODE),
            ),
        ];
        for (case, key, expected) in rows {
            let refusal = key_refusal(&key);
            assert_eq!(
                refusal.as_ref().map(|(code, _)| *code),
                expected,
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }
}
