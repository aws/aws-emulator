// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `DeleteObject` answers an address the bucket does not hold.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`],
//! [`super::object_storage`] and [`super::object_versioning`] already own — so what is
//! pinned is the wire behaviour (status, bound headers, body, what the bucket still holds)
//! rather than any handler's internals.
//!
//! The rule under test, asserted over a SET of addresses rather than one request:
//!
//! > A delete is over the state it LEAVES, not over what was there to remove: `DeleteObject`
//! > models no error at all, so an address holding nothing is no failure for it. A key the
//! > bucket holds no version of, and a well-formed `versionId` it holds no version for, are
//! > answered with the operation's own success status — HTTP 204 with no body — exactly as a
//! > delete of a stored address is, and the 204 reports only what the call actually did: the
//! > `x-amz-delete-marker` / `x-amz-version-id` pair describes a marker the delete created or
//! > a version it removed, so a delete that did neither carries neither. Nothing else the
//! > bucket holds moves, and the bucket a sequence of such deletes has emptied is deletable.
//!
//! Its sources: `smithy_query(query="operation",operation="DeleteObject")` reports
//! `http {method: "DELETE", uri: "/{Bucket}/{Key+}?x-id=DeleteObject", code: 204}` with
//! `errors: []` and an output binding only the three response headers; "Removes an object
//! from a bucket… if there isn't a null version, Amazon S3 does not remove anything" and
//! "To permanently delete an object in a versioned bucket, you must include the object's
//! versionId in the request" (`DeleteObject`); "Returns the version ID of the delete marker
//! created as a result of the DELETE operation" (`DeleteObjectOutput$VersionId`); "All
//! objects (including all object versions and delete markers) in the bucket must be deleted
//! before the bucket itself can be deleted" (`DeleteBucket`).
//!
//! A token that is not SHAPED like a version id is a different answer — a malformed argument,
//! refused 400 — and [`super::version_id_classification`] holds that half. The keyless half of
//! the rule is also covered, from the listing side, by
//! [`super::object_listing_and_deletion`]'s own idempotence test; this suite spans the forms
//! of the EMPTY address, the repeats, and the teardown that ends in `DeleteBucket`.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{head_bucket, router, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_storage::delete_bucket;
use super::object_versioning::{
    delete_object, list_object_versions, set_versioning, version_listing_of,
};

/// A `versionId` of the published shape that this store never minted: thirty-two characters
/// of the alphabet a minted id is drawn from.
const NEVER_MINTED: &str = "0123456789abcdef0123456789abcdef";

/// The versioning state a case's bucket carries, as `BucketVersioningStatus` publishes them
/// (`PutBucketVersioning`: `Enabled`, `Suspended`), plus the bucket that carries none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Versioning {
    /// No `PutBucketVersioning` was ever called on the bucket.
    Never,
    /// `Enabled` — "All objects added to the bucket receive a unique version ID".
    Enabled,
    /// `Suspended` after `Enabled` — "All objects added to the bucket receive the version ID
    /// null", and every id minted before stays.
    Suspended,
}

impl Versioning {
    /// The status to put on the bucket, or `None` for the bucket that carries none.
    fn status(self) -> Option<&'static str> {
        match self {
            Self::Never => None,
            Self::Enabled => Some("Enabled"),
            Self::Suspended => Some("Suspended"),
        }
    }
}

/// Which address a row deletes, in terms the row's own setup supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The KEY, naming no `versionId` — on a bucket with a versioning state this lays a
    /// marker, so rows that use it name a key the bucket never held and assert no marker was
    /// laid where the key has no chain at all.
    Key,
    /// A `versionId` of the published shape the store never minted.
    NeverMinted,
    /// The `null` version id, on a key whose every version carries a minted id.
    NullVersion,
    /// The id the setup's own write minted, deleted once by the setup already — the retry a
    /// teardown makes when it cannot be sure the first delete landed.
    AlreadyDeleted,
}

/// Every `(key, version id)` the bucket holds, as `ListObjectVersions` reports them.
async fn held(router: &MultiServiceRouter, name: &str, case: &str) -> Vec<(String, String)> {
    let listed = list_object_versions(router, name, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: a version listing of a bucket that exists is 200, got {} with body {:?}",
        listed.status,
        text(&listed)
    );
    version_listing_of(&listed).entries()
}

/// Assert `answer` is the delete's own no-op success: the modelled status, no body, and
/// neither bound header, because nothing was removed and no marker was created.
fn assert_no_op_success(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: a delete of an address holding nothing answers HTTP 204, got {} with \
         body {:?}",
        answer.status,
        text(answer)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: that success carries no body, got {:?}",
        text(answer)
    );
    for header in ["x-amz-delete-marker", "x-amz-version-id"] {
        assert!(
            answer.header(header).is_none(),
            "case {case:?}: a delete that removed nothing reports no {header}, got {:?}",
            answer.header(header)
        );
    }
}

#[tokio::test]
async fn a_delete_of_an_address_the_bucket_does_not_hold_is_the_modelled_success() {
    // The rows differ along what the rule generalises over: the bucket's versioning state
    // (none, `Enabled`, `Suspended` after `Enabled`), the FORM of the address that holds
    // nothing (a key with no chain at all, a well-formed id nothing minted, the `null` id on
    // a key whose versions all carry minted ids, and the id of a version this very bucket
    // held and a delete already took away), the shape of the key (flat, nested, one carrying
    // a space and one carrying non-ASCII, each percent-encoded into the request URI the way
    // a client sends it), and how many times the delete is repeated. The
    // recorded case is ONE of them: a `versionId` whose version the teardown had already
    // removed.
    let rows: &[(&str, Versioning, &str, Address, usize)] = &[
        (
            "no versioning state, a key nothing ever wrote",
            Versioning::Never,
            "absent.txt",
            Address::Key,
            1,
        ),
        (
            "no versioning state, a nested key nothing ever wrote, deleted three times",
            Versioning::Never,
            "reports/2024/absent.csv",
            Address::Key,
            3,
        ),
        (
            "no versioning state, an id nothing minted",
            Versioning::Never,
            "absent.txt",
            Address::NeverMinted,
            1,
        ),
        (
            "versioning enabled, an id nothing minted",
            Versioning::Enabled,
            "held.txt",
            Address::NeverMinted,
            1,
        ),
        (
            "versioning enabled, an id nothing minted, deleted twice",
            Versioning::Enabled,
            "deep/nested/held.txt",
            Address::NeverMinted,
            2,
        ),
        (
            "versioning enabled, the null version of a key that has none",
            Versioning::Enabled,
            "held.txt",
            Address::NullVersion,
            1,
        ),
        (
            "versioning enabled, the version this bucket held until it was deleted",
            Versioning::Enabled,
            "held.txt",
            Address::AlreadyDeleted,
            1,
        ),
        (
            "versioning enabled, that same version deleted three times over",
            Versioning::Enabled,
            "a%20key%20with%20a%20space.txt",
            Address::AlreadyDeleted,
            3,
        ),
        (
            "versioning suspended, the version minted before suspension, already deleted",
            Versioning::Suspended,
            "rapport-trimestriel-%C3%A9.txt",
            Address::AlreadyDeleted,
            1,
        ),
        (
            "versioning suspended, an id nothing minted",
            Versioning::Suspended,
            "held.txt",
            Address::NeverMinted,
            1,
        ),
    ];

    for (case, versioning, key, address, repeats) in rows {
        // A fresh store per row, so one row's leftovers cannot decide another's.
        let router = router();
        let name = "idempotent-deletes";
        bucket(&router, name).await;
        if let Some(status) = versioning.status() {
            set_versioning(&router, name, status).await;
        }

        // The SETUP: a witness object at another key, so every row can assert the delete
        // under test took nothing else away, plus — for the rows that need one — a version of
        // `key` this bucket really held.
        let witness = put_object(&router, name, "witness.bin", b"untouched", &[]).await;
        assert_eq!(
            witness.status, 200,
            "case {case:?}: the witness write must be accepted, got {} with body {:?}",
            witness.status,
            text(&witness)
        );
        let mut minted: Option<String> = None;
        if matches!(address, Address::NullVersion | Address::AlreadyDeleted) {
            if *versioning == Versioning::Suspended {
                // A version carrying a minted id exists only if the write happened while the
                // bucket was `Enabled`; suspension is a state of the bucket, not of the
                // versions it holds.
                set_versioning(&router, name, "Enabled").await;
            }
            let written = put_object(&router, name, key, b"held for a while", &[]).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: the write of {key:?} must be accepted, got {} with body {:?}",
                written.status,
                text(&written)
            );
            minted = written.header("x-amz-version-id");
            assert!(
                minted.is_some(),
                "case {case:?}: a versioning-enabled bucket reports the id it minted"
            );
            if *versioning == Versioning::Suspended {
                set_versioning(&router, name, "Suspended").await;
            }
        }
        if *address == Address::AlreadyDeleted {
            let id = minted.clone().expect("the setup minted an id");
            let first = delete_object(&router, name, key, &format!("?versionId={id}")).await;
            assert_eq!(
                first.status, 204,
                "case {case:?}: the first delete of the stored version is 204, got {} with \
                 body {:?}",
                first.status,
                text(&first)
            );
            assert_eq!(
                first.header("x-amz-version-id").as_deref(),
                Some(id.as_str()),
                "case {case:?}: the delete that DID remove a version reports which one"
            );
        }

        let query = match address {
            Address::Key => String::new(),
            Address::NeverMinted => format!("?versionId={NEVER_MINTED}"),
            Address::NullVersion => "?versionId=null".to_string(),
            Address::AlreadyDeleted => format!(
                "?versionId={}",
                minted.clone().expect("the setup minted an id")
            ),
        };
        let before = held(&router, name, case).await;

        // The delete under test, repeated: every answer is the same no-op success, which is
        // what makes the retry a teardown performs safe.
        for attempt in 1..=*repeats {
            let repeated = format!("{case} (attempt {attempt})");
            let answer = delete_object(&router, name, key, &query).await;
            assert_no_op_success(&repeated, &answer);
        }

        // It took nothing away and laid nothing down: the bucket holds exactly what it held,
        // and the witness object still reads back.
        let after = held(&router, name, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a delete of an address holding nothing must leave every stored \
             version where it was"
        );
        let read = get_object(&router, name, "witness.bin", "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the witness object must survive, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            b"untouched",
            "case {case:?}: the witness object's bytes must be untouched"
        );
    }
}

#[tokio::test]
async fn a_bucket_emptied_by_deletes_that_repeat_themselves_is_deletable() {
    // The create-use-destroy teardown: every object and version removed, each address
    // deleted a second time the way a retrying teardown does, and then the bucket itself.
    // The rows differ along the bucket's versioning state, which decides whether a keyless
    // delete removes a version or lays a marker over it, and along how many keys and
    // revisions the history leaves to clear.
    let rows: &[(&str, Versioning, &[&str], usize)] = &[
        ("no versioning state, one key", Versioning::Never, &["only.txt"], 1),
        (
            "no versioning state, three keys, two revisions each",
            Versioning::Never,
            &["one", "deep/two.txt", "three.bin"],
            2,
        ),
        (
            "versioning enabled, three keys, three revisions each",
            Versioning::Enabled,
            &["one", "deep/two.txt", "trois.txt"],
            3,
        ),
        (
            "versioning suspended, two keys, two revisions each",
            Versioning::Suspended,
            &["one", "deep/two.txt"],
            2,
        ),
    ];

    for (case, versioning, keys, revisions) in rows {
        let router = router();
        let name = "emptied-then-deleted";
        bucket(&router, name).await;
        if let Some(status) = versioning.status() {
            set_versioning(&router, name, status).await;
        }
        for key in *keys {
            for revision in 1..=*revisions {
                let body = format!("{key} revision {revision}");
                let written = put_object(&router, name, key, body.as_bytes(), &[]).await;
                assert_eq!(
                    written.status, 200,
                    "case {case:?}: the write of {key:?} must be accepted, got {} with body {:?}",
                    written.status,
                    text(&written)
                );
            }
        }

        // The teardown: a keyless delete of each key (which, on a bucket with a versioning
        // state, lays a marker rather than removing anything), then every version the bucket
        // still holds by its own id — each of those deletes run TWICE, so the second one is a
        // delete of an address that is already gone.
        for key in *keys {
            for attempt in 1..=2 {
                let answer = delete_object(&router, name, key, "").await;
                assert_eq!(
                    answer.status, 204,
                    "case {case:?}: the keyless delete of {key:?} (attempt {attempt}) is 204, \
                     got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
            }
        }
        loop {
            let remaining = held(&router, name, case).await;
            let Some((key, version_id)) = remaining.first().cloned() else {
                break;
            };
            for attempt in 1..=2 {
                let answer =
                    delete_object(&router, name, &key, &format!("?versionId={version_id}")).await;
                assert_eq!(
                    answer.status, 204,
                    "case {case:?}: the delete of {key:?} version {version_id:?} (attempt \
                     {attempt}) is 204, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
            }
            let left = held(&router, name, case).await;
            assert!(
                left.len() < remaining.len(),
                "case {case:?}: deleting version {version_id:?} of {key:?} must take it away, \
                 still {left:?}"
            );
        }

        // Emptied by its own deletes, the bucket deletes: this is the step a client that has
        // just cleared a bucket makes, and it is answered with `DeleteBucket`'s own 204.
        let removed = delete_bucket(&router, name).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: a bucket its own deletes emptied must delete, got {} with body {:?}",
            removed.status,
            text(&removed)
        );
        assert_eq!(
            head_bucket(&router, name).await.status,
            404,
            "case {case:?}: the deleted bucket must stop existing"
        );
    }
}
