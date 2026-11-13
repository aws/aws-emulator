// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an object write is refused for about the NAME it gives and the
//! CLASS it asks for, and for the retrieval tier a restore may ask of an archived object.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! answer: the status, the error document's `<Code>` and `<Message>`, and whether anything
//! was stored.
//!
//! The rules under test:
//!
//! * A storage class a write names has to be one the addressed bucket can HOST.
//!   `PutObjectRequest$StorageClass` states the division: "directory buckets only support
//!   `EXPRESS_ONEZONE` (the S3 Express One Zone storage class) in Availability Zones and
//!   `ONEZONE_IA` (the S3 One Zone-Infrequent Access storage class) in Dedicated Local Zones"
//!   and "Amazon S3 on Outposts only uses the OUTPOSTS Storage Class" — so a general purpose
//!   bucket hosts every published class but `EXPRESS_ONEZONE` and `OUTPOSTS`, and a directory
//!   bucket hosts only its own two. A class the bucket cannot host is refused
//!   `InvalidStorageClass` (400) and nothing is stored. The rows span the whole
//!   `StorageClass` enum the model publishes, on both kinds of bucket.
//! * An object key is refused for its BYTES alone: "the name for a key is a sequence of
//!   Unicode characters whose UTF-8 encoding is at most 1,024 bytes long" (Amazon S3 User
//!   Guide, *Creating object key names*), so a key of exactly 1,024 bytes is stored and a
//!   longer one is refused `KeyTooLongError` (400) — the code a client branching on the limit
//!   reads; and a key carrying a NUL (0x00) byte, which no request URI can convey, is not a
//!   request the API gets to refuse at all — it is answered with the bare HTTP 400 that
//!   carries no error document, which is what an SDK reports the status itself as the code
//!   for. Both hold for every operation that names a key: the one-shot write, the destination
//!   of a copy, and the initiation of a multipart upload.
//! * A restore may only ask for a retrieval tier the object's own storage class OFFERS: S3
//!   Glacier Flexible Retrieval offers Expedited, Standard and Bulk, while S3 Glacier Deep
//!   Archive offers Standard and Bulk alone (Amazon S3 User Guide, *Archive retrieval
//!   options*). A tier the class does not offer is refused `InvalidTier` (400) and no
//!   retrieval is recorded, so a later read still reports no restore state. A tier the class
//!   does offer answers the status the operation documents for a retrieval it INITIATED:
//!   "if the object is not previously restored, then Amazon S3 returns `202 Accepted` in the
//!   response."

use service_com_amazonaws_s3_amazons3::ops::types::{STORAGE_CLASS_VALUES, TIER_VALUES};
use service_com_amazonaws_s3_amazons3::service::object_write::{
    DIRECTORY_BUCKET_STORAGE_CLASSES, GENERAL_PURPOSE_STORAGE_CLASSES,
    SERVICE_PLACED_STORAGE_CLASSES,
};
use service_com_amazonaws_s3_amazons3::state::{
    ARCHIVED_STORAGE_CLASSES, DEFAULT_STORAGE_CLASS, MAX_OBJECT_KEY_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_versioning::head_object;

/// The bytes every write in this suite carries.
const BODY: &[u8] = b"class-and-key-subject";

/// The code and message S3 answers a class the addressed bucket cannot host with (Amazon S3
/// "Error responses", list of error codes: "InvalidStorageClass — The storage class you
/// specified is not valid", HTTP 400). Spelled out here rather than imported, so a wording
/// regression fails this suite.
const INVALID_STORAGE_CLASS: &str = "InvalidStorageClass";
/// See [`INVALID_STORAGE_CLASS`].
const INVALID_STORAGE_CLASS_MESSAGE: &str = "The storage class you specified is not valid";

/// The code S3 answers a key past the published byte ceiling with, from the same list
/// ("KeyTooLongError — Your key is too long", HTTP 400), and that list's own sentence.
const KEY_TOO_LONG: &str = "KeyTooLongError";
/// See [`KEY_TOO_LONG`].
const KEY_TOO_LONG_MESSAGE: &str = "Your key is too long";

/// The code an SDK reports for the refusal that carries NO error document: the bare status it
/// saw, which is what a key carrying a byte no URI can convey is answered with.
const ERRORLESS_400: &str = "400";

/// The code and message S3 answers a retrieval tier the object's class does not offer with.
const INVALID_TIER: &str = "InvalidTier";
/// See [`INVALID_TIER`].
const RETRIEVAL_OPTION_UNSUPPORTED: &str =
    "Retrieval option is not supported by this storage class.";

#[tokio::test]
async fn a_write_is_refused_a_storage_class_the_bucket_cannot_host() {
    let router = router();
    let general_purpose = "class-general-purpose";
    let directory = "class--use1-az4--x-s3";
    bucket(&router, general_purpose).await;
    directory_bucket(&router, directory, "use1-az4").await;

    // The catalog the rule divides: every class the model publishes, each driven against both
    // kinds of bucket, so no published value is left untested and neither list can quietly
    // stop covering the enum.
    assert!(
        STORAGE_CLASS_VALUES.len()
            > GENERAL_PURPOSE_STORAGE_CLASSES.len()
                .max(DIRECTORY_BUCKET_STORAGE_CLASSES.len()),
        "each kind of bucket must host fewer classes than the model publishes, got {:?}",
        STORAGE_CLASS_VALUES
    );

    for storage_class in STORAGE_CLASS_VALUES {
        for (kind, name, hosted) in [
            (
                "a general purpose bucket",
                general_purpose,
                GENERAL_PURPOSE_STORAGE_CLASSES.contains(storage_class),
            ),
            (
                "a directory bucket",
                directory,
                DIRECTORY_BUCKET_STORAGE_CLASSES.contains(storage_class),
            ),
        ] {
            let case = format!("{storage_class} on {kind}");
            let key = format!("classes/{}.bin", storage_class.to_ascii_lowercase());
            let written = put_object(
                &router,
                name,
                &key,
                BODY,
                &[("x-amz-storage-class", storage_class)],
            )
            .await;
            // A class only another AWS service places objects in is one no client write may
            // NAME, whichever kind of bucket it addresses: that is a bad ARGUMENT rather than
            // a class the bucket cannot host, and it is judged before the hosting below
            // ([`SERVICE_PLACED_STORAGE_CLASSES`]).
            if SERVICE_PLACED_STORAGE_CLASSES.contains(storage_class) {
                assert_eq!(
                    written.status, 400,
                    "case {case:?}: a class no write may name must be refused 400, got {} with \
                     body {:?}",
                    written.status,
                    text(&written)
                );
                assert_eq!(
                    written.error_code(),
                    "InvalidArgument",
                    "case {case:?}: expected the InvalidArgument code, got {:?}",
                    written.error_code()
                );
                assert_eq!(
                    written.error_message(),
                    format!("{storage_class} is not allowed."),
                    "case {case:?}: expected the class then the fixed text, got {:?}",
                    written.error_message()
                );
                let head = head_object(&router, name, &key, "").await;
                assert_eq!(
                    head.status, 404,
                    "case {case:?}: a refused write must store nothing, got {} from a HEAD",
                    head.status
                );
                continue;
            }
            if hosted {
                assert_eq!(
                    written.status, 200,
                    "case {case:?}: a class the bucket hosts must be stored, got {} with \
                     body {:?}",
                    written.status,
                    text(&written)
                );
                // Stored UNDER that class: a later read reports the class the write named —
                // except for STANDARD, which "Amazon S3 returns this header for all objects
                // except for" (`HeadObjectOutput$StorageClass`), so the commonest class is
                // reported by the absence of the header.
                let head = head_object(&router, name, &key, "").await;
                let reported = match *storage_class == DEFAULT_STORAGE_CLASS {
                    true => None,
                    false => Some(*storage_class),
                };
                assert_eq!(
                    head.header("x-amz-storage-class").as_deref(),
                    reported,
                    "case {case:?}: the stored object must report the class it was written \
                     in, got {:?}",
                    head.header("x-amz-storage-class")
                );
                continue;
            }
            assert_eq!(
                written.status, 400,
                "case {case:?}: a class the bucket cannot host must be refused 400, got {} \
                 with body {:?}",
                written.status,
                text(&written)
            );
            assert_eq!(
                written.error_code(),
                INVALID_STORAGE_CLASS,
                "case {case:?}: expected the {INVALID_STORAGE_CLASS:?} code, got {:?}",
                written.error_code()
            );
            assert_eq!(
                written.error_message(),
                INVALID_STORAGE_CLASS_MESSAGE,
                "case {case:?}: expected the published sentence, got {:?}",
                written.error_message()
            );
            // Nothing was stored: the address holds no object to report a class for.
            let head = head_object(&router, name, &key, "").await;
            assert_eq!(
                head.status, 404,
                "case {case:?}: a refused write must store nothing, got {} from a HEAD",
                head.status
            );
        }
    }
}

/// `PUT /<bucket>/<key>` with `x-amz-copy-source` — `CopyObject`, where `key` is already
/// spelled the way the request URI carries it.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    copy_source: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}"),
        &[("x-amz-copy-source", copy_source)],
        b"",
    )
    .await
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`.
async fn initiate(router: &MultiServiceRouter, bucket_name: &str, key: &str) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploads"),
        &[],
        "",
    )
    .await
}

#[tokio::test]
async fn a_key_is_refused_for_the_bytes_it_carries() {
    let router = router();
    let name = "key-bytes-bucket";
    bucket(&router, name).await;
    let source_key = "source.bin";
    let written = put_object(&router, name, source_key, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the copy source must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the key as the request URI spells it, how many BYTES that key is once decoded,
    // the code the write must answer — empty for a key S3 stores). The rows differ along the
    // LENGTH of the key either side of the published ceiling, whether its bytes are one per
    // character, and whether it carries the one byte no URI conveys.
    let at_ceiling = "k".repeat(MAX_OBJECT_KEY_BYTES);
    let past_ceiling = "k".repeat(MAX_OBJECT_KEY_BYTES + 1);
    // Two bytes per character, percent-encoded the way a client spells a non-ASCII key in a
    // request URI: 512 characters are 1,024 BYTES, because the ceiling counts the UTF-8
    // encoding rather than the characters.
    let multi_byte_at_ceiling = "%C3%A9".repeat(MAX_OBJECT_KEY_BYTES / 2);
    let multi_byte_past_ceiling = "%C3%A9".repeat(MAX_OBJECT_KEY_BYTES / 2 + 1);
    let rows: [(&str, &str, usize, &str); 7] = [
        ("an ordinary key", "reports/plain.csv", 17, ""),
        ("a one-character key", "k", 1, ""),
        (
            "a key of exactly the published ceiling",
            at_ceiling.as_str(),
            MAX_OBJECT_KEY_BYTES,
            "",
        ),
        (
            "a key one byte past the ceiling",
            past_ceiling.as_str(),
            MAX_OBJECT_KEY_BYTES + 1,
            KEY_TOO_LONG,
        ),
        (
            "a multi-byte key of exactly the ceiling in BYTES",
            multi_byte_at_ceiling.as_str(),
            MAX_OBJECT_KEY_BYTES,
            "",
        ),
        (
            "a multi-byte key two bytes past the ceiling",
            multi_byte_past_ceiling.as_str(),
            MAX_OBJECT_KEY_BYTES + 2,
            KEY_TOO_LONG,
        ),
        (
            "a key carrying a NUL byte",
            "nul/a%00b.txt",
            11,
            ERRORLESS_400,
        ),
    ];

    for (case, key, key_bytes, expected) in rows {
        // The same key is driven through every operation that NAMES one, since the rule is
        // the key's rather than one operation's.
        let calls = [
            ("PutObject", put_object(&router, name, key, BODY, &[]).await),
            (
                "CopyObject",
                copy_object(&router, name, key, &format!("{name}/{source_key}")).await,
            ),
            ("CreateMultipartUpload", initiate(&router, name, key).await),
        ];
        for (operation, answer) in calls {
            if expected.is_empty() {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}/{operation}: a key S3 stores must be accepted, got {} with \
                     body {:?}",
                    answer.status,
                    text(&answer)
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            if expected == ERRORLESS_400 {
                // No error document at all: the status is the whole answer, which is what an
                // SDK reports the status as the code for.
                assert!(
                    answer.body.is_empty(),
                    "case {case:?}/{operation}: expected no error document, got {:?}",
                    text(&answer)
                );
                continue;
            }
            assert_eq!(
                answer.error_code(),
                expected,
                "case {case:?}/{operation}: expected the {expected:?} code, got {:?} with \
                 body {:?}",
                answer.error_code(),
                text(&answer)
            );
            // The code's own published text, which names neither the key nor the ceiling.
            assert_eq!(
                answer.error_message(),
                KEY_TOO_LONG_MESSAGE,
                "case {case:?}/{operation}: expected the published sentence for a key of \
                 {key_bytes} bytes, got {:?}",
                answer.error_message()
            );
        }
    }
}

/// The `RestoreRequest` container a restore carries: the lifetime, and the tier where the row
/// names it.
fn restore_payload(days: i32, tier: Option<&str>, in_glacier_parameters: bool) -> String {
    let tier = match (tier, in_glacier_parameters) {
        (None, _) => String::new(),
        (Some(tier), false) => format!("<Tier>{tier}</Tier>"),
        (Some(tier), true) => {
            format!("<GlacierJobParameters><Tier>{tier}</Tier></GlacierJobParameters>")
        }
    };
    format!(
        "<RestoreRequest xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Days>{days}</Days>{tier}</RestoreRequest>"
    )
}

/// `POST /<bucket>/<key>?restore` — `RestoreObject`.
async fn restore_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    payload: &str,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?restore"),
        &[],
        payload,
    )
    .await
}

#[tokio::test]
async fn a_restore_is_refused_a_tier_the_objects_storage_class_does_not_offer() {
    let router = router();
    let name = "restore-tiers";
    bucket(&router, name).await;

    // The pairs the rule divides: every archive storage class against every tier the model
    // publishes, with the tier named in each of the two containers that carry it. Deep
    // Archive offers Standard and Bulk alone, so only its Expedited rows are refused.
    let offered: [(&str, &[&str]); 2] = [
        ("GLACIER", &["Expedited", "Standard", "Bulk"]),
        ("DEEP_ARCHIVE", &["Standard", "Bulk"]),
    ];
    let mut covered: Vec<&str> = offered
        .iter()
        .map(|(storage_class, _)| *storage_class)
        .collect();
    covered.sort_unstable();
    let mut archived: Vec<&str> = ARCHIVED_STORAGE_CLASSES.to_vec();
    archived.sort_unstable();
    assert_eq!(
        covered, archived,
        "the table must cover every archive storage class, got {covered:?}"
    );

    for (storage_class, tiers) in offered {
        for tier in TIER_VALUES {
            for in_glacier_parameters in [false, true] {
                let case = format!(
                    "{tier} on a {storage_class} object, named in {}",
                    match in_glacier_parameters {
                        true => "GlacierJobParameters",
                        false => "the container",
                    }
                );
                let key = format!(
                    "{}/{}-{}.bin",
                    storage_class.to_ascii_lowercase(),
                    tier.to_ascii_lowercase(),
                    u8::from(in_glacier_parameters)
                );
                let written = put_object(
                    &router,
                    name,
                    &key,
                    BODY,
                    &[("x-amz-storage-class", storage_class)],
                )
                .await;
                assert_eq!(
                    written.status, 200,
                    "case {case:?}: the archived object must be written first, got {} with \
                     body {:?}",
                    written.status,
                    text(&written)
                );

                let asked = restore_object(
                    &router,
                    name,
                    &key,
                    &restore_payload(1, Some(tier), in_glacier_parameters),
                )
                .await;
                if tiers.contains(tier) {
                    assert_eq!(
                        asked.status, 202,
                        "case {case:?}: a tier the class offers must initiate a retrieval, got \
                         {} with body {:?}",
                        asked.status,
                        text(&asked)
                    );
                    let head = head_object(&router, name, &key, "").await;
                    assert!(
                        head.header("x-amz-restore").is_some(),
                        "case {case:?}: an accepted restore must be reported as ongoing, got \
                         {:?}",
                        head.header("x-amz-restore")
                    );
                    continue;
                }
                assert_eq!(
                    asked.status, 400,
                    "case {case:?}: a tier the class does not offer must be refused 400, got \
                     {} with body {:?}",
                    asked.status,
                    text(&asked)
                );
                assert_eq!(
                    asked.error_code(),
                    INVALID_TIER,
                    "case {case:?}: expected the {INVALID_TIER:?} code, got {:?}",
                    asked.error_code()
                );
                assert_eq!(
                    asked.error_message(),
                    RETRIEVAL_OPTION_UNSUPPORTED,
                    "case {case:?}: expected the published sentence, got {:?}",
                    asked.error_message()
                );
                // The refusal recorded no retrieval: the version still reports none.
                let head = head_object(&router, name, &key, "").await;
                assert_eq!(
                    head.header("x-amz-restore"),
                    None,
                    "case {case:?}: a refused restore must record nothing, got {:?}",
                    head.header("x-amz-restore")
                );
            }
        }
    }
}
