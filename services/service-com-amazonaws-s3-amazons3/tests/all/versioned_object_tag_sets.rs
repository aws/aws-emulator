// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `versionId` of the three object-tagging calls: a tag set belongs
//! to ONE stored version.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the `Tagging`
//! payload builder, the tag-set reader and the object writer are the ones
//! [`super::bucket_lifecycle`], [`super::bucket_tagging`],
//! [`super::object_and_tagging_roundtrip`], [`super::object_tagging`] and
//! [`super::object_tag_set_removal`] already own), so what is pinned is the wire behaviour.
//!
//! The rule under test: "to retrieve tags of any other version, use the versionId query
//! parameter" (`GetObjectTagging`), "to add tags to the current version, use the versionId
//! query parameter" (`PutObjectTagging`) and "to delete tags of a specific object version,
//! add the versionId query parameter in the request" (`DeleteObjectTagging`) — so the
//! parameter ADDRESSES the stored version whose tag set is read, replaced or removed, a call
//! naming none acts on the key's current version, and every other version's tag set is left
//! exactly as it was.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_tagging::{tag, tag_set_of, tagging_payload};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_tag_set_removal::delete_object_tagging;
use super::object_tagging::get_object_tagging;
use super::object_versioning::{delete_object, set_versioning};

/// The response header a write to a versioning-enabled bucket reports the version it minted
/// in: `PutObjectOutput$VersionId` is bound to `x-amz-version-id`.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// `PUT /<bucket>/<key>?tagging&versionId=<version>` — `PutObjectTagging` addressing one
/// stored version.
async fn put_version_tagging(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    version: &str,
    tags: &[(String, String)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}?tagging&versionId={version}"),
        &[],
        &tagging_payload(tags),
    )
    .await
}

/// One write into a versioning-enabled bucket, and the version id it minted.
async fn write_version(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    body: &[u8],
) -> String {
    let written = put_object(router, bucket_name, key, body, &[]).await;
    assert_eq!(
        written.status, 200,
        "the write of {key:?} must succeed, got {} with body {:?}",
        written.status,
        text(&written)
    );
    written
        .header(VERSION_ID_HEADER)
        .unwrap_or_else(|| panic!("a write to a versioned bucket must report {VERSION_ID_HEADER}"))
}

/// RULE: the `versionId` of an object-tagging call addresses the stored version whose tag set
/// is read, replaced or removed; a call naming none acts on the key's CURRENT version, and
/// every other version keeps its own set.
///
/// The rows differ along what the rule generalises over — WHICH version a call addresses: the
/// oldest stored version, a middle one, the newest (which is also the current one), and the
/// current version addressed by naming no version at all.
#[tokio::test]
async fn a_tag_set_belongs_to_the_version_the_call_addresses() {
    let router = router();
    let bucket_name = "version-scoped-tag-sets";
    let key = "scoped.bin";
    bucket(&router, bucket_name).await;
    set_versioning(&router, bucket_name, "Enabled").await;

    // Three versions of one key, each written with its own bytes so nothing but the version
    // id can tell them apart.
    let first = write_version(&router, bucket_name, key, b"first revision").await;
    let second = write_version(&router, bucket_name, key, b"second revision").await;
    let third = write_version(&router, bucket_name, key, b"third revision").await;
    for (left, right) in [(&first, &second), (&second, &third), (&first, &third)] {
        assert_ne!(
            left, right,
            "each write must mint its own version id, got {left:?} and {right:?}"
        );
    }

    // Every version is tagged with a set of its own, through the version-addressed write.
    let sets = [
        (first.clone(), vec![tag("which", "first")]),
        (second.clone(), vec![tag("which", "second"), tag("rank", "2")]),
        (third.clone(), vec![tag("which", "third")]),
    ];
    for (version, tags) in &sets {
        let written = put_version_tagging(&router, bucket_name, key, version, tags).await;
        assert_eq!(
            (written.status, written.header(VERSION_ID_HEADER)),
            (200, Some(version.clone())),
            "case {version:?}: the write must report the version it tagged, got {} with body \
             {:?}",
            written.status,
            text(&written)
        );
    }

    // (case, the versionId the read names — empty for none at all, the set expected)
    type ReadRow = (&'static str, String, Vec<(String, String)>);
    let rows: Vec<ReadRow> = vec![
        (
            "the oldest stored version",
            format!("&versionId={first}"),
            vec![tag("which", "first")],
        ),
        (
            "a noncurrent version between two others",
            format!("&versionId={second}"),
            vec![tag("which", "second"), tag("rank", "2")],
        ),
        (
            "the newest stored version",
            format!("&versionId={third}"),
            vec![tag("which", "third")],
        ),
        (
            "no version at all, which is the current one",
            String::new(),
            vec![tag("which", "third")],
        ),
    ];
    for (case, query, expected) in &rows {
        let read = get_object_tagging(&router, bucket_name, key, query).await;
        assert_eq!(
            (read.status, tag_set_of(&read)),
            (200, expected.clone()),
            "case {case:?}: expected {expected:?}, got {} with body {:?}",
            read.status,
            text(&read)
        );
    }

    // A REPLACEMENT of one version's set leaves every other version's alone.
    let replaced = put_version_tagging(
        &router,
        bucket_name,
        key,
        &second,
        &[tag("which", "second-again")],
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "replacing the middle version's set must succeed, got {} with body {:?}",
        replaced.status,
        text(&replaced)
    );
    for (case, version, expected) in [
        ("the oldest", &first, vec![tag("which", "first")]),
        ("the replaced one", &second, vec![tag("which", "second-again")]),
        ("the newest", &third, vec![tag("which", "third")]),
    ] {
        let read =
            get_object_tagging(&router, bucket_name, key, &format!("&versionId={version}")).await;
        assert_eq!(
            tag_set_of(&read),
            expected,
            "case {case:?}: a version-addressed replacement must touch no other version, got \
             {:?}",
            tag_set_of(&read)
        );
    }

    // And a REMOVAL clears only the version it addresses.
    let removed =
        delete_object_tagging(&router, bucket_name, key, &format!("&versionId={first}"), &[]).await;
    assert_eq!(
        (removed.status, removed.header(VERSION_ID_HEADER)),
        (204, Some(first.clone())),
        "the removal must report the version it cleared, got {} with body {:?}",
        removed.status,
        text(&removed)
    );
    for (case, version, expected) in [
        ("the cleared one", &first, Vec::new()),
        ("the middle one", &second, vec![tag("which", "second-again")]),
        ("the newest", &third, vec![tag("which", "third")]),
    ] {
        let read =
            get_object_tagging(&router, bucket_name, key, &format!("&versionId={version}")).await;
        assert_eq!(
            (read.status, tag_set_of(&read)),
            (200, expected.clone()),
            "case {case:?}: a version-addressed removal must clear only its own version, got \
             {} with body {:?}",
            read.status,
            text(&read)
        );
    }
}

/// RULE: a version-addressed tagging call is answered about the ADDRESS it names — a version
/// the key does not carry is that version's own 404, a delete marker carries no tag set at
/// all, and neither changes anything.
///
/// The rows differ along WHAT the address is: an id no version of the key carries, the id of a
/// version of ANOTHER key, and the id of a delete marker.
#[tokio::test]
async fn a_version_a_tagging_call_cannot_address_is_answered_about_that_version() {
    let router = router();
    let bucket_name = "version-scoped-tag-refusals";
    let key = "held.bin";
    let other_key = "other.bin";
    bucket(&router, bucket_name).await;
    set_versioning(&router, bucket_name, "Enabled").await;
    let held = write_version(&router, bucket_name, key, b"held bytes").await;
    let elsewhere = write_version(&router, bucket_name, other_key, b"other bytes").await;
    let tagged = put_version_tagging(&router, bucket_name, key, &held, &[tag("kept", "yes")]).await;
    assert_eq!(
        tagged.status, 200,
        "the setup tagging must succeed, got {} with body {:?}",
        tagged.status,
        text(&tagged)
    );

    // A plain delete of a versioned key leaves a DELETE MARKER as the current version of that
    // key, which is the third address below.
    let deleted = delete_object(&router, bucket_name, key, "").await;
    let marker = deleted
        .header(VERSION_ID_HEADER)
        .expect("a delete on a versioned bucket must report the marker's version id");

    // (case, the version id addressed, the status and code expected)
    let rows: [(&str, String, u16, &str); 3] = [
        (
            "an id no version of the key carries",
            "3HL4kqtJlcpXroDTDmjVBH40Nrjfkd".to_string(),
            404,
            "NoSuchVersion",
        ),
        (
            "a version id of another key",
            elsewhere.clone(),
            404,
            "NoSuchVersion",
        ),
        ("a delete marker's version id", marker, 405, "MethodNotAllowed"),
    ];
    for (case, version, status, code) in &rows {
        let read =
            get_object_tagging(&router, bucket_name, key, &format!("&versionId={version}")).await;
        assert_eq!(
            (read.status, read.error_code()),
            (*status, code.to_string()),
            "case {case:?} (GetObjectTagging): expected {status} {code:?}, got {} with body {:?}",
            read.status,
            text(&read)
        );
        let written =
            put_version_tagging(&router, bucket_name, key, version, &[tag("no", "write")]).await;
        assert_eq!(
            (written.status, written.error_code()),
            (*status, code.to_string()),
            "case {case:?} (PutObjectTagging): expected {status} {code:?}, got {} with body {:?}",
            written.status,
            text(&written)
        );
        let removed =
            delete_object_tagging(&router, bucket_name, key, &format!("&versionId={version}"), &[])
                .await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (*status, code.to_string()),
            "case {case:?} (DeleteObjectTagging): expected {status} {code:?}, got {} with body \
             {:?}",
            removed.status,
            text(&removed)
        );
    }

    // Nothing any of those refusals touched changed: the held version still reports the set it
    // was given.
    let read = get_object_tagging(&router, bucket_name, key, &format!("&versionId={held}")).await;
    assert_eq!(
        (read.status, tag_set_of(&read)),
        (200, vec![tag("kept", "yes")]),
        "a refused version-addressed call must leave the tag set untouched, got {} with body \
         {:?}",
        read.status,
        text(&read)
    );
}

/// The marker a delete leaves is addressed by its own version id, so the row above that names
/// one is a real stored version rather than a value this suite invented: a tagging call on it
/// is refused for WHAT it addresses and not for an id the bucket holds nothing for.
#[tokio::test]
async fn a_delete_markers_version_id_names_a_version_the_bucket_holds() {
    let router = router();
    let bucket_name = "version-scoped-tag-markers";
    let key = "marked.bin";
    bucket(&router, bucket_name).await;
    set_versioning(&router, bucket_name, "Enabled").await;
    let stored = write_version(&router, bucket_name, key, b"before the marker").await;
    let deleted = delete_object(&router, bucket_name, key, "").await;
    let marker = deleted
        .header(VERSION_ID_HEADER)
        .expect("a delete on a versioned bucket must report the marker's version id");
    assert_ne!(
        marker, stored,
        "the marker must be a version of its own, got {marker:?} for the object {stored:?}"
    );
    // The version BEHIND the marker still carries its own tag set, which is what makes the
    // marker's refusal about the marker alone.
    let tagged = put_version_tagging(&router, bucket_name, key, &stored, &[tag("under", "marker")])
        .await;
    assert_eq!(
        tagged.status, 200,
        "the version behind the marker must still take a tag set, got {} with body {:?}",
        tagged.status,
        text(&tagged)
    );
    let read = get_object_tagging(&router, bucket_name, key, &format!("&versionId={stored}")).await;
    assert_eq!(
        (read.status, tag_set_of(&read)),
        (200, vec![tag("under", "marker")]),
        "the version behind the marker must report its own tag set, got {} with body {:?}",
        read.status,
        text(&read)
    );
}
