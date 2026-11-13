// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the storage class an object WRITE may name.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::kms_key_references`] already own — so what is pinned is the wire behaviour
//! (status, error code, the refusal's own members, what the bucket holds afterwards).
//!
//! The rule under test, asserted over the model's whole `StorageClass` catalog rather than
//! one value:
//!
//! > Membership in the `StorageClass` enum is what a READ of an object reports, not
//! > permission to ask a write for it. A class whose objects only the AWS service that owns
//! > them places in a bucket cannot be named by `PutObject`, `CopyObject` or
//! > `CreateMultipartUpload`: the request is refused HTTP 400 `InvalidArgument` whose message
//! > is that class followed by " is not allowed.", naming the `x-amz-storage-class` header
//! > and echoing the value, and nothing is written. A class that belongs to another KIND of
//! > bucket than the one addressed is refused `InvalidStorageClass`. Every other published
//! > class is stored as asked and reported back by a read.
//!
//! Its sources: the catalog is the model's own
//! (`smithy_query(query="shape",shape="StorageClass")` reports `STANDARD`,
//! `REDUCED_REDUNDANCY`, `STANDARD_IA`, `ONEZONE_IA`, `INTELLIGENT_TIERING`, `GLACIER`,
//! `DEEP_ARCHIVE`, `OUTPOSTS`, `GLACIER_IR`, `SNOW`, `EXPRESS_ONEZONE`, `FSX_OPENZFS`,
//! `FSX_ONTAP`, `AWS_BACKUP_WARM`, `AWS_BACKUP_LOW_COST_WARM`); "Directory buckets only
//! support `EXPRESS_ONEZONE` … and `ONEZONE_IA` …" and "Amazon S3 on Outposts only uses the
//! OUTPOSTS Storage Class" (`PutObjectRequest$StorageClass`); the published error list
//! ("InvalidStorageClass — The storage class you specified is not valid"); and
//! `InvalidArgument`'s own `<ArgumentName>` / `<ArgumentValue>` pair (Amazon S3 API
//! Reference, *Error responses*). The refusal keeps a bucket EMPTY where AWS kept it empty,
//! which is what lets a `DeleteBucket` after such a write succeed — asserted here too.

use service_com_amazonaws_s3_amazons3::state::DEFAULT_STORAGE_CLASS;

use super::bucket_lifecycle::{router, Answer};
use super::kms_key_references::{copy_object, create_multipart_upload};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_storage::delete_bucket;
use super::object_versioning::{delete_object, head_object};

/// The header every one of these writes names its class in.
const HEADER: &str = "x-amz-storage-class";

/// How a class a write names is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Stored in that class, and reported back by a read of the object.
    Stored,
    /// Refused `InvalidArgument`: a class only another AWS service places objects in, so no
    /// client write may name it.
    NotAllowed,
    /// Refused `InvalidStorageClass`: a class belonging to another kind of bucket than the
    /// general purpose one these cases address.
    NotHosted,
}

/// Every value of the model's `StorageClass` enum, paired with how a write naming it into a
/// GENERAL PURPOSE bucket must be answered. The issue cited `FSX_OPENZFS` alone; every other
/// row is a value it named nowhere.
const CATALOG: &[(&str, Outcome)] = &[
    ("STANDARD", Outcome::Stored),
    ("REDUCED_REDUNDANCY", Outcome::Stored),
    ("STANDARD_IA", Outcome::Stored),
    ("ONEZONE_IA", Outcome::Stored),
    ("INTELLIGENT_TIERING", Outcome::Stored),
    ("GLACIER", Outcome::Stored),
    ("DEEP_ARCHIVE", Outcome::Stored),
    ("GLACIER_IR", Outcome::Stored),
    ("OUTPOSTS", Outcome::NotHosted),
    ("EXPRESS_ONEZONE", Outcome::NotHosted),
    ("SNOW", Outcome::NotAllowed),
    ("FSX_OPENZFS", Outcome::NotAllowed),
    ("FSX_ONTAP", Outcome::NotAllowed),
    ("AWS_BACKUP_WARM", Outcome::NotAllowed),
    ("AWS_BACKUP_LOW_COST_WARM", Outcome::NotAllowed),
];

/// The bytes every write in this suite carries.
const BODY: &[u8] = b"storage class under test";

/// Assert `answer` is the refusal `outcome` names, over the class `class` the write named.
fn assert_refused(case: &str, class: &str, outcome: Outcome, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a write naming {class:?} is refused with HTTP 400, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    match outcome {
        Outcome::NotAllowed => {
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case:?}: a class no write may name is a bad argument, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                format!("{class} is not allowed."),
                "case {case:?}: expected the class then the fixed text, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                answer.element("ArgumentName"),
                Some(HEADER.to_string()),
                "case {case:?}: the refusal names the header the class arrived in, got {:?}",
                answer.element("ArgumentName")
            );
            assert_eq!(
                answer.element("ArgumentValue"),
                Some(class.to_string()),
                "case {case:?}: the refusal echoes the value, got {:?}",
                answer.element("ArgumentValue")
            );
        }
        Outcome::NotHosted => {
            assert_eq!(
                answer.error_code(),
                "InvalidStorageClass",
                "case {case:?}: a class of another kind of bucket is InvalidStorageClass, got \
                 {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                "The storage class you specified is not valid",
                "case {case:?}: expected the published sentence, got {:?}",
                answer.error_message()
            );
        }
        Outcome::Stored => unreachable!("case {case:?}: a stored class is no refusal"),
    }
}

#[tokio::test]
async fn a_write_may_name_only_a_storage_class_the_addressed_bucket_accepts_from_a_client() {
    for (class, outcome) in CATALOG {
        // A fresh store per row, so one row's object cannot decide another row's emptiness.
        let router = router();
        let name = "storage-class-writes";
        bucket(&router, name).await;
        // The SOURCE a copy reads, written in no particular class so the row's class is the
        // only thing under test.
        let source = put_object(&router, name, "source.bin", BODY, &[]).await;
        assert_eq!(
            source.status, 200,
            "case {class:?}: the source write must be accepted, got {} with body {:?}",
            source.status,
            text(&source)
        );

        // The same class, named by each of the three writes that take one.
        let key = format!("{}.bin", class.to_ascii_lowercase());
        let written = put_object(&router, name, &key, BODY, &[(HEADER, class)]).await;
        let copied = copy_object(
            &router,
            name,
            &format!("copy-{key}"),
            &format!("{name}/source.bin"),
            &[(HEADER, class)],
        )
        .await;
        let initiated =
            create_multipart_upload(&router, name, &format!("mpu-{key}"), &[(HEADER, class)]).await;

        match outcome {
            Outcome::Stored => {
                assert_eq!(
                    written.status, 200,
                    "case {class:?}: a class a write may name is stored, got {} with body {:?}",
                    written.status,
                    text(&written)
                );
                assert_eq!(
                    copied.status, 200,
                    "case {class:?}: the copy naming it is stored too, got {} with body {:?}",
                    copied.status,
                    text(&copied)
                );
                assert_eq!(
                    initiated.status, 200,
                    "case {class:?}: the initiation naming it is accepted, got {} with body {:?}",
                    initiated.status,
                    text(&initiated)
                );
                // A read reports the class it was stored in — the write's effect, not its
                // echo: an absent header means the default class. The read is a HEAD, so an
                // archived class (`GLACIER`, `DEEP_ARCHIVE`) reports its metadata rather
                // than the `InvalidObjectState` a GET of archived bytes earns.
                let read = head_object(&router, name, &key, "").await;
                assert_eq!(
                    read.status, 200,
                    "case {class:?}: the stored object must read back, got {}",
                    read.status
                );
                let reported = read
                    .header("x-amz-storage-class")
                    .unwrap_or_else(|| DEFAULT_STORAGE_CLASS.to_string());
                assert_eq!(
                    &reported, class,
                    "case {class:?}: the read must report the class the write asked for, got \
                     {reported:?}"
                );
            }
            refusal => {
                for (operation, answer) in [
                    ("PutObject", &written),
                    ("CopyObject", &copied),
                    ("CreateMultipartUpload", &initiated),
                ] {
                    assert_refused(&format!("{class}/{operation}"), class, *refusal, answer);
                }
                // NOTHING was written: the refused key holds no object, and the bucket holds
                // only the source — so a client that empties what it did write can delete the
                // bucket, which a stored-anyway object would make impossible.
                let read = head_object(&router, name, &key, "").await;
                assert_eq!(
                    read.status, 404,
                    "case {class:?}: a refused write stores nothing, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
                let copied_read = head_object(&router, name, &format!("copy-{key}"), "").await;
                assert_eq!(
                    copied_read.status, 404,
                    "case {class:?}: a refused copy stores nothing, got {}",
                    copied_read.status
                );
                let still_full = delete_bucket(&router, name).await;
                assert_eq!(
                    still_full.status, 409,
                    "case {class:?}: the bucket still holds the source object, got {} with body \
                     {:?}",
                    still_full.status,
                    text(&still_full)
                );
                let cleared = delete_object(&router, name, "source.bin", "").await;
                assert_eq!(
                    cleared.status, 204,
                    "case {class:?}: the source delete is 204, got {}",
                    cleared.status
                );
                let removed = delete_bucket(&router, name).await;
                assert_eq!(
                    removed.status, 204,
                    "case {class:?}: with every write it made deleted, the bucket deletes, got \
                     {} with body {:?}",
                    removed.status,
                    text(&removed)
                );
            }
        }
    }
}
