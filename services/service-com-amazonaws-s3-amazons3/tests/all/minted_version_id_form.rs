// SPDX-License-Identifier: CC0-1.0

//! The FORM of every version id this service mints, as a write reports it on the wire.
//!
//! RULE under test: a version id is an opaque token S3 issues per stored version —
//! `ObjectVersionId` is a plain string in the model
//! (`smithy_query(query="shape",shape="ObjectVersionId")` reports no `@pattern` and no
//! `@length`), and "if you enable versioning for a bucket, Amazon S3 automatically generates a
//! unique version ID for the object being stored" (`PutObjectOutput$VersionId`) — written in
//! the published form of such a token: [`VERSION_ID_CHARS`] characters of the ASCII
//! alphanumerics, which is what Amazon S3's own published version ids are written in
//! (`3HL4kqtJlcpXroDTDmJ_RGrmHJ1PyFCY`, Amazon S3 User Guide, *Working with object versioning*,
//! is alphanumeric but for one character).
//!
//! That the form holds for EVERY draw rather than for some is the point of this suite: the id
//! is derived from the write that minted it, so an encoding whose alphabet is wider than the
//! published form — base64's `-` and `_`, say — writes some ids inside it and some outside, and
//! which one a given write lands on is not something a client, a recording or a comparison of
//! recorded answers can be asked to tolerate. Two further invariants come with the token: the
//! value is the one the version is ADDRESSED by (a read naming it serves exactly the bytes that
//! write stored), and no two versions anywhere in the store carry the same one.
//!
//! The rows span what the rule generalises over — what the minted id is derived from, which is
//! the bucket, the key and how many versions the store has minted: the bucket (three of them,
//! so the ids of one bucket are not the only ones read), the key (one character, a flat name, a
//! nested path, a name of non-ASCII text, a name carrying a space), the revision count (every
//! row is written twice over, so the mint sequence advances), and the three paths that mint one
//! — a plain write, a COPY into a versioned bucket, and the DELETE MARKER a delete without a
//! `versionId` lays down.
//!
//! Every call goes over the wire through the harness the other object suites own, so what is
//! pinned is the wire document.

use std::collections::BTreeSet;

use super::bucket_lifecycle::{router, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, set_versioning};

/// The header a write reports the id it minted in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// How many characters a version id this service mints carries: the width of the opaque token
/// S3's own published ids are written at. Spelled out here rather than read from the crate, so a
/// changed constant cannot make this suite pass.
const VERSION_ID_CHARS: usize = 32;

/// Whether `id` is written in the published form of a version id: exactly
/// [`VERSION_ID_CHARS`] characters, each an ASCII alphanumeric.
fn is_published_form(id: &str) -> bool {
    id.chars().count() == VERSION_ID_CHARS && id.chars().all(|character| character.is_ascii_alphanumeric())
}

/// The id a write reported, failing the test when it reported none.
fn minted(case: &str, answer: &Answer) -> String {
    answer
        .header(VERSION_ID_HEADER)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a write into a versioning-enabled bucket must report \
                 {VERSION_ID_HEADER}, got status {} and body {:?}",
                answer.status,
                text(answer)
            )
        })
}

/// Assert one minted id: the published form, and a value no other version of this run carries.
fn assert_minted_form(case: &str, id: &str, seen: &mut BTreeSet<String>) {
    assert!(
        is_published_form(id),
        "case {case:?}: expected a version id of {VERSION_ID_CHARS} ASCII alphanumeric \
         characters, got {id:?}"
    );
    assert!(
        seen.insert(id.to_string()),
        "case {case:?}: expected a version id no other version carries, got the already-minted \
         {id:?}"
    );
}

/// The keys these rows write, each a (case, key) pair: the shapes an object key takes, since
/// the key is one of the three things a minted id is derived from.
fn key_rows() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a one-character key", "a"),
        ("a flat key", "notes.txt"),
        ("a nested path", "media/2024/april.jpg"),
        ("a key of non-ASCII text", "notes/ünïcode.txt"),
        ("a key carrying a space", "april%20report.jpg"),
    ]
}

/// The buckets these rows write into: the bucket is one of the three things a minted id is
/// derived from, so the rule is read over more than one.
const BUCKETS: [&str; 3] = [
    "minted-version-id-form-a",
    "minted-version-id-form-b",
    "minted-version-id-form-c",
];

/// RULE: every id a write into a versioning-enabled bucket mints is written in the published
/// form, is unique across the store, and addresses exactly the bytes that write stored.
#[tokio::test]
async fn every_minted_version_id_is_written_in_the_published_form() {
    let router = router();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for name in BUCKETS {
        bucket(&router, name).await;
        set_versioning(&router, name, "Enabled").await;
    }

    for (bucket_index, name) in BUCKETS.iter().enumerate() {
        for (case, key) in key_rows() {
            // Two revisions of each key, so the mint sequence advances under the same bucket
            // and key and a second id is read at the next position.
            for revision in 0..2_usize {
                let case = format!("{name}/{case} (revision {revision})");
                let body = format!("version {revision} of {key} in {name}").into_bytes();
                let written = put_object(&router, name, key, &body, &[]).await;
                assert_eq!(
                    written.status, 200,
                    "case {case:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
                    written.status,
                    text(&written)
                );
                let id = minted(&case, &written);
                assert_minted_form(&case, &id, &mut seen);

                // The id is what ADDRESSES that version: a read naming it serves those bytes,
                // handed back on the `versionId` parameter exactly as it was minted.
                let read = get_object(&router, name, key, &format!("?versionId={id}"), &[]).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected HTTP 200 reading version {id:?}, got {} with body \
                     {:?}",
                    read.status,
                    text(&read)
                );
                assert_eq!(
                    read.body.as_ref(),
                    body.as_slice(),
                    "case {case:?}: expected version {id:?} to serve the bytes that write \
                     stored, got {:?}",
                    text(&read)
                );
            }

            // The COPY path mints one of its own, for the version it writes at the destination.
            let destination = format!("copies/{bucket_index}/{key}");
            let copied = put_object(
                &router,
                name,
                &destination,
                b"",
                &[("x-amz-copy-source", &format!("/{name}/{key}"))],
            )
            .await;
            assert_eq!(
                copied.status, 200,
                "case {name}/{case} (copy): expected HTTP 200 from CopyObject, got {} with body \
                 {:?}",
                copied.status,
                text(&copied)
            );
            let copy_id = minted(&format!("{name}/{case} (copy)"), &copied);
            assert_minted_form(&format!("{name}/{case} (copy)"), &copy_id, &mut seen);

            // And the DELETE MARKER a delete naming no version lays down carries one too.
            let deleted = delete_object(&router, name, key, "").await;
            assert_eq!(
                deleted.status, 204,
                "case {name}/{case} (delete marker): expected HTTP 204 from DeleteObject, got {} \
                 with body {:?}",
                deleted.status,
                text(&deleted)
            );
            let marker_id = minted(&format!("{name}/{case} (delete marker)"), &deleted);
            assert_minted_form(
                &format!("{name}/{case} (delete marker)"),
                &marker_id,
                &mut seen,
            );
        }
    }

    // The rows minted an id on every path, over every bucket and key: three paths per key row,
    // two revisions on the write path.
    let expected = BUCKETS.len() * key_rows().len() * 4;
    assert_eq!(
        seen.len(),
        expected,
        "expected one distinct version id per minting call, got {} for {expected} calls",
        seen.len()
    );
}
