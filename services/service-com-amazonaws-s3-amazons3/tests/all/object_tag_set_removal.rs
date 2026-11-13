// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteObjectTagging`: the removal of an OBJECT's whole tag set.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the
//! `Tagging` payload builder, the tag-set reader and the object writer are the ones
//! [`super::bucket_lifecycle`], [`super::bucket_tagging`],
//! [`super::object_and_tagging_roundtrip`] and [`super::object_tagging`] already own), so
//! what is pinned is the wire behaviour — status, payload, error code — rather than any
//! handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `DeleteObjectTagging` "removes the entire tag set from the specified object", so
//!   whatever set the object carried, afterwards it carries NONE and the object itself is
//!   untouched. The success is the modelled 204 with an empty body, and it is idempotent:
//!   an object that carried no tags is already in the asked-for state.
//! * The removal is addressed: it is the tag set of the object the request names that goes,
//!   not another key's, not the same key's in another bucket, and not the bucket's own.
//! * An address this emulator does not hold answers which part is missing —
//!   `NoSuchBucket` or `NoSuchKey`, both HTTP 404 — and removes nothing.
//! * An `x-amz-expected-bucket-owner` that is not this emulator's account is "403 Forbidden
//!   (access denied)" (`DeleteObjectTaggingRequest$ExpectedBucketOwner`) and removes
//!   nothing; the account this emulator does serve is accepted.
//! * `versionId` names ONE stored version, and a tag set belongs to that version: a removal
//!   naming a version the key does not carry is that version's own 404 (`NoSuchVersion`) and
//!   removes nothing — as it is for the reader and the writer of the same subresource.
//! * Nothing reads an object's tags through a stale copy: a tag-filtered lifecycle rule
//!   stops applying to the object as soon as its tag set is removed.

use service_com_amazonaws_s3_amazons3::state::{MAX_TAGS_PER_OBJECT, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_lifecycle_configuration::{configuration, put_lifecycle, rule};
use super::bucket_tagging::{get_bucket_tagging, put_bucket_tagging, tag, tag_set_of};
use super::object_and_tagging_roundtrip::{bucket, get_object, text};
use super::object_tagging::{get_object_tagging, object, put_object_tagging};
use super::object_versioning::head_object;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has. Asserted
/// against the emulator's own id where it is used, so it cannot silently become the owner.
const NOT_THE_OWNER: &str = "123456789012";

/// `DELETE /<bucket>/<key>?tagging` — `DeleteObjectTagging`, with whatever extra query
/// parameters and bound headers the case under test carries.
///
/// `pub(crate)`: the suite for the version an object sub-resource call reports
/// ([`super::object_subresource_version_id`]) removes its tag sets through this one helper
/// rather than spelling the subresource a second time.
pub(crate) async fn delete_object_tagging(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}/{key}?tagging{query}"),
        headers,
        "",
    )
    .await
}

/// `n` distinct tags, so a row can carry a set of any size without writing each one out.
fn tags_numbered(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
        .collect()
}

/// One row of the removal table: the case name, the key the object is stored at, and the
/// tag set the object carries before the removal.
type RemovalRow = (&'static str, &'static str, Vec<(String, String)>);

/// The table of tag sets every one of which must be gone in full after one removal.
///
/// The rows differ along what the "removes the entire tag set" rule generalises over: how
/// many tags the set carries (none at all, one, three, and the published maximum per
/// object), how long a key and a value are (one character, empty, and both published
/// character bounds exactly), and which characters they contain (ASCII, non-ASCII letters,
/// and the punctuation the published tag restrictions allow). The key each row is stored
/// under varies too — a flat name, a prefixed
/// one and a deep path — because the rule is about the object at a key, whatever its shape.
///
/// The counts and the character bounds come from the limits this service publishes for an
/// object's tag set (`MAX_TAGS_PER_OBJECT`, `MAX_TAG_KEY_CHARS`, `MAX_TAG_VALUE_CHARS`),
/// not from any one recorded request.
fn removal_rows() -> Vec<RemovalRow> {
    vec![
        ("no tags at all", "untagged-flat-key", Vec::new()),
        (
            "one tag, the shape a provisioning script sends",
            "flat-key",
            vec![tag("environment", "production")],
        ),
        (
            "three tags under a prefixed key",
            "reports/2026/summary.csv",
            vec![
                tag("team", "data"),
                tag("tier", "cold"),
                tag("owner", "analytics"),
            ],
        ),
        (
            "the published maximum number of tags per object, under a deep path",
            "a/b/c/d/e/object.bin",
            tags_numbered(MAX_TAGS_PER_OBJECT),
        ),
        (
            "an empty value and a one-character key",
            "edge-lengths",
            vec![tag("k", ""), tag("flag", "1")],
        ),
        (
            "a key and a value at the published character bounds",
            "bounded-chars",
            vec![tag(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        ),
        (
            "non-ASCII letters and every punctuation mark a tag may carry",
            "unusual/characters",
            vec![
                tag("проект", "аналитика"),
                tag("path:to/the-key", "a+b=c.d_e:f/g@h"),
                tag("spaced out key", "spaced out value"),
            ],
        ),
    ]
}

#[tokio::test]
async fn a_removal_takes_an_objects_whole_tag_set_and_leaves_the_object() {
    let router = router();
    bucket(&router, "tag-removal").await;

    for (case, key, tags) in removal_rows() {
        object(&router, "tag-removal", key).await;
        if !tags.is_empty() {
            let stored = put_object_tagging(&router, "tag-removal", key, &tags).await;
            assert_eq!(
                stored.status, 200,
                "case {case:?}: the tag set must be stored first, got {} with body {:?}",
                stored.status,
                text(&stored)
            );
            let read = get_object_tagging(&router, "tag-removal", key, "").await;
            assert_eq!(
                tag_set_of(&read),
                tags,
                "case {case:?}: the stored set must read back before the removal, got {:?}",
                tag_set_of(&read)
            );
        }

        // Twice, because "removes the entire tag set" is a destination, not a step: the
        // second call finds the object already in the asked-for state and still succeeds.
        for attempt in ["first", "second"] {
            let removed = delete_object_tagging(&router, "tag-removal", key, "", &[]).await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}/{attempt}: the modelled success is 204, got {} with body {:?}",
                removed.status,
                text(&removed)
            );
            assert!(
                removed.body.is_empty(),
                "case {case:?}/{attempt}: a 204 carries no body, got {:?}",
                text(&removed)
            );
            // The bucket here is one whose versioning was never enabled, so its current
            // object is no version of anything and has no id to report: S3 omits the
            // header there, and none may be invented. The suite for the version an object
            // sub-resource call reports ([`super::object_subresource_version_id`]) covers
            // the versioned buckets, where the id IS reported.
            assert_eq!(
                removed.header("x-amz-version-id"),
                None,
                "case {case:?}/{attempt}: no version may be reported, got {:?}",
                removed.header("x-amz-version-id")
            );

            let read = get_object_tagging(&router, "tag-removal", key, "").await;
            assert_eq!(
                read.status, 200,
                "case {case:?}/{attempt}: the untagged object still answers a tagging \
                 read, got {} with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                tag_set_of(&read),
                Vec::<(String, String)>::new(),
                "case {case:?}/{attempt}: the entire tag set must be gone, got {:?}",
                tag_set_of(&read)
            );
        }

        // The tag set went; the OBJECT did not.
        let object_read = get_object(&router, "tag-removal", key, "", &[]).await;
        assert_eq!(
            object_read.status, 200,
            "case {case:?}: removing a tag set must not remove the object, got {} with \
             body {:?}",
            object_read.status,
            text(&object_read)
        );
        assert_eq!(
            object_read.body.as_ref(),
            b"payload",
            "case {case:?}: the object's bytes must be untouched, got {:?}",
            text(&object_read)
        );

        // And a set stored after the removal is the object's whole tag set again, so the
        // removal left no residue behind.
        let restored = vec![tag("after", "removal")];
        let stored = put_object_tagging(&router, "tag-removal", key, &restored).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: an object must be taggable again after a removal, got {} with \
             body {:?}",
            stored.status,
            text(&stored)
        );
        let read = get_object_tagging(&router, "tag-removal", key, "").await;
        assert_eq!(
            tag_set_of(&read),
            restored,
            "case {case:?}: the set stored after the removal must be the whole one, got {:?}",
            tag_set_of(&read)
        );
    }
}

#[tokio::test]
async fn a_removal_is_scoped_to_the_object_it_addresses() {
    let router = router();
    bucket(&router, "removal-scope-one").await;
    bucket(&router, "removal-scope-two").await;

    // The same key in two buckets, a second key beside it, and the bucket's own tag set:
    // four tag sets, of which exactly one is addressed.
    let shared_key = vec![tag("which", "one/shared")];
    let other_key = vec![tag("which", "one/other")];
    let other_bucket = vec![tag("which", "two/shared")];
    let bucket_set = vec![tag("which", "the bucket own set")];

    for (name, key, tags) in [
        ("removal-scope-one", "shared", &shared_key),
        ("removal-scope-one", "other", &other_key),
        ("removal-scope-two", "shared", &other_bucket),
    ] {
        object(&router, name, key).await;
        let stored = put_object_tagging(&router, name, key, tags).await;
        assert_eq!(
            stored.status, 200,
            "{name}/{key} must be tagged first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }
    let stored = put_bucket_tagging(&router, "removal-scope-one", &bucket_set).await;
    assert_eq!(
        stored.status, 204,
        "the bucket's own tag set must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    let removed = delete_object_tagging(&router, "removal-scope-one", "shared", "", &[]).await;
    assert_eq!(
        removed.status, 204,
        "the addressed object's tag set must be removable, got {} with body {:?}",
        removed.status,
        text(&removed)
    );

    for (case, name, key, expected) in [
        (
            "the addressed object",
            "removal-scope-one",
            "shared",
            Vec::new(),
        ),
        (
            "another key in the same bucket",
            "removal-scope-one",
            "other",
            other_key.clone(),
        ),
        (
            "the same key in another bucket",
            "removal-scope-two",
            "shared",
            other_bucket.clone(),
        ),
    ] {
        let read = get_object_tagging(&router, name, key, "").await;
        assert_eq!(
            tag_set_of(&read),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            tag_set_of(&read)
        );
    }

    // The bucket's own tag set is a different subresource and is not touched either.
    let bucket_read = get_bucket_tagging(&router, "removal-scope-one").await;
    assert_eq!(
        tag_set_of(&bucket_read),
        bucket_set,
        "removing an object's tag set must not touch its bucket's, got {:?}",
        tag_set_of(&bucket_read)
    );
}

#[tokio::test]
async fn a_removal_naming_an_address_this_emulator_does_not_hold_answers_the_missing_part() {
    let router = router();
    bucket(&router, "held-removal-bucket").await;
    object(&router, "held-removal-bucket", "held").await;
    let held = vec![tag("stays", "put")];
    let stored = put_object_tagging(&router, "held-removal-bucket", "held", &held).await;
    assert_eq!(
        stored.status, 200,
        "the held object must be tagged first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // A bucket this emulator does not hold is named as the missing part, whatever the key
    // looks like — including one that exists in another bucket.
    for (absent_bucket, key) in [("no-such-bucket", "held"), ("another-absent", "absent-key")] {
        let answer = delete_object_tagging(&router, absent_bucket, key, "", &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {absent_bucket:?}/{key:?}: an absent bucket is a 404, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchBucket",
            "case {absent_bucket:?}/{key:?}: expected the bucket to be named as the \
             missing part, got {:?}",
            answer.error_code()
        );
    }

    // A key the held bucket does not hold is named as the missing part instead.
    for absent_key in ["absent-key", "nested/absent", "held-but-longer"] {
        let answer = delete_object_tagging(&router, "held-removal-bucket", absent_key, "", &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {absent_key:?}: an absent key is a 404, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchKey",
            "case {absent_key:?}: expected the key to be named as the missing part, got {:?}",
            answer.error_code()
        );
    }

    // None of those refusals removed anything, and none of them created a key.
    let read = get_object_tagging(&router, "held-removal-bucket", "held", "").await;
    assert_eq!(
        tag_set_of(&read),
        held,
        "a refused removal must leave the held object's tag set untouched, got {:?}",
        tag_set_of(&read)
    );
    let absent = get_object_tagging(&router, "held-removal-bucket", "absent-key", "").await;
    assert_eq!(
        absent.status, 404,
        "a refused removal must not have created the key, got {}",
        absent.status
    );
}

#[tokio::test]
async fn a_removal_parameter_this_emulator_cannot_serve_is_refused_not_guessed() {
    let router = router();
    bucket(&router, "removal-unserved").await;
    object(&router, "removal-unserved", "versioned").await;
    let stored_set = vec![tag("current", "object")];
    let stored = put_object_tagging(&router, "removal-unserved", "versioned", &stored_set).await;
    assert_eq!(
        stored.status, 200,
        "the object must be tagged first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // `versionId` names ONE version of an object, and a tag set belongs to that version: "to
    // delete tags of a specific object version, add the versionId query parameter in the
    // request" (`DeleteObjectTagging`). An id no version of the key carries is that version's
    // own 404, and nothing is removed for it.
    for version in ["3HL4kqtJlcpXroDTDmjVBH40Nrjfkd", "NoSuchVersionAtAll00000000000000"] {
        let refused = delete_object_tagging(
            &router,
            "removal-unserved",
            "versioned",
            &format!("&versionId={version}"),
            &[],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "NoSuchVersion".to_string()),
            "case {version:?}: a removal naming a version the key does not carry is that \
             version's 404, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
    }

    // An `x-amz-expected-bucket-owner` that is not this emulator's account is denied.
    assert_ne!(
        NOT_THE_OWNER,
        engine::emulator_account_id(),
        "the test's non-owner account id must not be the account this emulator serves"
    );
    let denied = delete_object_tagging(
        &router,
        "removal-unserved",
        "versioned",
        "",
        &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
    )
    .await;
    assert_eq!(
        denied.status, 403,
        "an expected bucket owner that is not the owner is denied, got {} with body {:?}",
        denied.status,
        text(&denied)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "expected the documented code, got {:?}",
        denied.error_code()
    );

    // Neither refusal removed anything.
    let read = get_object_tagging(&router, "removal-unserved", "versioned", "").await;
    assert_eq!(
        tag_set_of(&read),
        stored_set,
        "a refused removal must leave the tag set untouched, got {:?}",
        tag_set_of(&read)
    );

    // The account this emulator DOES serve is accepted, and that call removes the set.
    let allowed = delete_object_tagging(
        &router,
        "removal-unserved",
        "versioned",
        "",
        &[(
            "x-amz-expected-bucket-owner",
            engine::emulator_account_id(),
        )],
    )
    .await;
    assert_eq!(
        allowed.status, 204,
        "the owner this emulator serves must be accepted, got {} with body {:?}",
        allowed.status,
        text(&allowed)
    );
    let read = get_object_tagging(&router, "removal-unserved", "versioned", "").await;
    assert_eq!(
        tag_set_of(&read),
        Vec::<(String, String)>::new(),
        "the accepted removal must have taken the whole tag set, got {:?}",
        tag_set_of(&read)
    );
}

#[tokio::test]
async fn an_untagged_object_leaves_a_tag_filtered_lifecycle_rule() {
    let router = router();
    bucket(&router, "removal-lifecycle").await;

    // One rule, filtered on a single tag, so what it applies to follows the object's
    // CURRENT tag set and nothing else.
    let put = put_lifecycle(
        &router,
        "removal-lifecycle",
        &[],
        &configuration(&[rule(
            "<ID>archive-1</ID>\
             <Filter><Tag><Key>archive</Key><Value>yes</Value></Tag></Filter>\
             <Status>Enabled</Status><Expiration><Days>1</Days></Expiration>",
        )]),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "the configuration must be stored first, got {} with body {:?}",
        put.status,
        text(&put)
    );

    object(&router, "removal-lifecycle", "logs/retagged.txt").await;
    let head = head_object(&router, "removal-lifecycle", "logs/retagged.txt", "").await;
    assert_eq!(
        head.header("x-amz-expiration"),
        None,
        "an untagged object is outside the tag-filtered rule, got {:?}",
        head.header("x-amz-expiration")
    );

    let stored = put_object_tagging(
        &router,
        "removal-lifecycle",
        "logs/retagged.txt",
        &[tag("archive", "yes")],
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the tag that moves the object into the rule must be stored, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    let head = head_object(&router, "removal-lifecycle", "logs/retagged.txt", "").await;
    let reported = head.header("x-amz-expiration");
    assert!(
        reported
            .as_ref()
            .is_some_and(|value| value.contains("rule-id=\"archive-1\"")),
        "the tagged object must report the rule that expires it, got {reported:?}"
    );

    // Removing the tag set moves it back out: the rule no longer matches, so nothing
    // expires it.
    let removed =
        delete_object_tagging(&router, "removal-lifecycle", "logs/retagged.txt", "", &[]).await;
    assert_eq!(
        removed.status, 204,
        "the tag set must be removable, got {} with body {:?}",
        removed.status,
        text(&removed)
    );
    let head = head_object(&router, "removal-lifecycle", "logs/retagged.txt", "").await;
    assert_eq!(
        head.header("x-amz-expiration"),
        None,
        "an object whose tag set was removed is outside the tag-filtered rule, got {:?}",
        head.header("x-amz-expiration")
    );
}
